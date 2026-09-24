use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, ValueEnum)]
pub enum Agent {
    All,
    Claude,
    Github,
    Codex,
    None,
}

pub struct Asset<'a> {
    pub path: &'a str,
    pub body: &'a [u8],
}

pub fn bundled() -> Vec<Asset<'static>> {
    include!(concat!(env!("OUT_DIR"), "/bundled_assets.rs"))
}

pub fn skill_hash(bytes: &[u8]) -> String {
    let normalized: Vec<u8> = bytes
        .iter()
        .enumerate()
        .filter(|(i, b)| !(**b == b'\r' && bytes.get(i + 1) == Some(&b'\n')))
        .map(|(_, b)| *b)
        .collect();
    format!("{:x}", Sha256::digest(normalized))
}

// Reject junctions as well as symbolic links, including links which point inside root.
fn is_link(meta: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        meta.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        meta.file_type().is_symlink()
    }
}

fn checked(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(name) = component else {
            bail!("invalid relative path: {relative}")
        };
        path.push(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if is_link(&meta) => bail!("skill path contains a link: {}", path.display()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path)
}

fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("missing parent")?;
    fs::create_dir_all(parent)?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(bytes)?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|e| e.error)?;
    Ok(())
}

struct Lock {
    path: PathBuf,
    file: Option<fs::File>,
}
impl Drop for Lock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

pub fn install(root: &Path, agent: Agent) -> Result<usize> {
    let assets = bundled();
    let selected: Vec<_> = assets
        .into_iter()
        .filter(|asset| match agent {
            Agent::All => true,
            Agent::Claude => asset.path.starts_with(".claude/"),
            Agent::Github => asset.path.starts_with(".github/"),
            Agent::Codex => asset.path.starts_with(".codex/") || asset.path.starts_with(".agents/"),
            Agent::None => false,
        })
        .collect();
    install_assets(root, &selected)
}

pub fn install_assets(root: &Path, assets: &[Asset<'_>]) -> Result<usize> {
    if assets.is_empty() {
        return Ok(0);
    }
    let allowed = bundled();
    let mut names = std::collections::BTreeSet::new();
    for asset in assets {
        if !allowed.iter().any(|known| known.path == asset.path) || !names.insert(asset.path) {
            bail!("invalid or duplicate skill path: {}", asset.path);
        }
    }
    fs::create_dir_all(root)
        .with_context(|| format!("cannot create project directory: {}", root.display()))?;
    let root = root.canonicalize().context("cannot resolve project root")?;
    let marker = checked(&root, ".arp/installed-skills.json")?;
    let lock_path = checked(&root, ".arp/rust-skills-install.lock")?;
    fs::create_dir_all(lock_path.parent().unwrap())?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .context("cannot acquire skill installation lock; another installation may be running")?;
    let _lock = Lock {
        path: lock_path,
        file: Some(file),
    };
    let mut prior: BTreeMap<String, String> = if marker.exists() {
        serde_json::from_slice(&fs::read(&marker)?).context("invalid skill ownership record")?
    } else {
        BTreeMap::new()
    };
    let mut changes = Vec::new();
    // Preflight every file before any replacement, including the ownership record.
    for asset in assets {
        let target = checked(&root, asset.path)?;
        let existing = if target.exists() {
            Some(fs::read(&target).with_context(|| format!("cannot read {}", target.display()))?)
        } else {
            None
        };
        if let Some(bytes) = &existing {
            let hash = skill_hash(bytes);
            if hash != skill_hash(asset.body) && prior.get(asset.path) != Some(&hash) {
                bail!(
                    "skill has local changes; preserve/reconcile it: {}",
                    target.display()
                );
            }
        }
        prior.insert(asset.path.to_owned(), skill_hash(asset.body));
        changes.push((target, existing, asset.body.to_vec()));
    }
    let mut record = serde_json::to_vec_pretty(&prior)?;
    record.push(b'\n');
    let old_record = if marker.exists() {
        Some(fs::read(&marker)?)
    } else {
        None
    };
    changes.push((marker, old_record, record));
    for (index, (path, _, body)) in changes.iter().enumerate() {
        if let Err(error) = replace(path, body) {
            let mut rollback_errors = Vec::new();
            for (previous, original, _) in changes[..index].iter().rev() {
                let restored = match original {
                    Some(bytes) => replace(previous, bytes),
                    None => fs::remove_file(previous).map_err(anyhow::Error::from),
                };
                if let Err(restore_error) = restored {
                    rollback_errors.push(format!("{}: {restore_error}", previous.display()));
                }
            }
            bail!("installation failed: {error}; rollback errors: {rollback_errors:?}");
        }
    }
    Ok(assets.len())
}
