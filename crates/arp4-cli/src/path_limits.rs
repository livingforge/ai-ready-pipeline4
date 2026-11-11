//! How long the path of a file a project keeps may be where it is checked out.
//! ARP itself reads and writes paths of any length (see `data::verbatim`), but
//! Git for Windows without `core.longpaths` does not: measured with Git
//! 2.45, `git add` fails on a file of 259 characters and a checkout cannot
//! make a folder of 248. A longer file would stay out of commits, or out of
//! the checkout, with no more than a warning.
//!
//! Lengths are of the absolute path as Windows counts them (UTF-16 units from
//! the drive letter), where this clone is: a clone in a longer folder holds
//! less, unless it enables long paths.
use anyhow::{Result, bail};
use std::path::Path;

/// The longest file and folder paths Git for Windows takes without long paths.
const GIT_FILE: usize = 258;
const GIT_FOLDER: usize = 247;

/// The limits for files of the project at `root`. Git is asked whether it
/// takes long paths only when a path is longer than it takes without them.
pub struct PathLimits<'a> {
    root: &'a Path,
    git_short: Option<bool>,
}

impl<'a> PathLimits<'a> {
    pub fn new(root: &'a Path) -> Self {
        Self {
            root,
            git_short: None,
        }
    }

    /// Fails when Git cannot keep the file `path` of the project, or its folder.
    pub fn check(&mut self, path: &Path) -> Result<()> {
        let file = length(path);
        let folder = path.parent().map_or(0, length);
        if file <= GIT_FILE && folder <= GIT_FOLDER {
            return Ok(());
        }
        let root = self.root;
        if !*self.git_short.get_or_insert_with(|| git_short(root)) {
            return Ok(());
        }
        let shown = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let (what, length, limit) = if file > GIT_FILE {
            ("path", file, GIT_FILE)
        } else {
            ("folder", folder, GIT_FOLDER)
        };
        bail!(
            "document {what} too long for Git on Windows without long paths ({length} > {limit} characters from the drive in this clone): {shown}; run `git config core.longpaths true` in every Windows clone, check out the project in a shorter folder, or shorten the folder or file names of the original"
        )
    }
}

/// Whether Git keeps the project at `root` to short paths: on Windows, in a
/// Git work tree, unless Git is set up for long paths. A Git that cannot be
/// asked is taken to keep to them.
fn git_short(root: &Path) -> bool {
    cfg!(windows)
        && root.ancestors().any(|folder| folder.join(".git").exists())
        && !std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["config", "--type=bool", "--get", "core.longpaths"])
            .output()
            .is_ok_and(|output| output.status.success() && output.stdout.trim_ascii() == b"true")
}

/// The length of `path` as Windows counts it, without a `\\?\` prefix.
fn length(path: &Path) -> usize {
    let text = path.to_string_lossy();
    let units = text.encode_utf16().count();
    if text.starts_with(r"\\?\UNC\") {
        // \\?\UNC\server is \\server
        units - r"\\?\UNC\".len() + r"\\".len()
    } else if text.starts_with(r"\\?\") {
        units - r"\\?\".len()
    } else {
        units
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_are_counted_without_the_long_path_prefix() {
        assert_eq!(length(Path::new(r"C:\a\b")), 6);
        assert_eq!(length(Path::new(r"\\?\C:\a\b")), 6);
        assert_eq!(length(Path::new(r"\\server\share\a")), 16);
        assert_eq!(length(Path::new(r"\\?\UNC\server\share\a")), 16);
        assert_eq!(length(Path::new(r"C:\資料\𠮷")), 8);
    }

    #[test]
    fn paths_git_takes_without_long_paths_are_not_checked() {
        // Git is not asked: the root is not a work tree and no path is long.
        let root = Path::new(r"C:\project");
        let mut limits = PathLimits::new(root);
        let folder = format!(r"C:\{}", "a".repeat(GIT_FOLDER - 3));
        limits.check(&Path::new(&folder).join("b")).unwrap();
        limits
            .check(&Path::new(r"C:\").join("f".repeat(GIT_FILE - 3)))
            .unwrap();
        assert_eq!(limits.git_short, None);
    }

    #[test]
    fn files_and_folders_longer_than_git_takes_are_rejected() {
        let root = Path::new(r"C:\project");
        let mut limits = PathLimits {
            root,
            git_short: Some(true),
        };
        let file = |size: usize| root.join("f".repeat(size - length(root) - 1));
        limits.check(&file(GIT_FILE)).unwrap();
        let error = limits.check(&file(GIT_FILE + 1)).unwrap_err().to_string();
        assert!(error.contains("document path too long"), "{error}");
        assert!(error.contains("(259 > 258"), "{error}");
        assert!(error.contains(": ffff"), "{error}");

        let folder = |size: usize| root.join("d".repeat(size - length(root) - 1));
        limits.check(&folder(GIT_FOLDER).join("a")).unwrap();
        let error = limits
            .check(&folder(GIT_FOLDER + 1).join("a"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("document folder too long"), "{error}");
        assert!(error.contains("(248 > 247"), "{error}");

        // A Git set up for long paths, or no Git, takes them.
        limits.git_short = Some(false);
        limits.check(&file(GIT_FILE + 1)).unwrap();
    }
}
