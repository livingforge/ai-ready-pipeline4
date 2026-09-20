use arp4_cli::skills::{self, Agent, Asset};
use std::{fs, process::Command};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_arp4"))
}

#[test]
fn installs_embedded_skills_with_empty_path() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("日本語 project");
    fs::create_dir(&root).unwrap();
    let result = binary()
        .env("PATH", "")
        .args(["skills", "install", "--root"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for asset in skills::bundled() {
        assert_eq!(fs::read(root.join(asset.path)).unwrap(), asset.body);
    }
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join(".arp/installed-skills.json")).unwrap())
            .unwrap();
    assert_eq!(record.as_object().unwrap().len(), 4);
    assert!(!root.join(".arp/runtime").exists());
}

#[test]
fn selection_and_repeat_install_preserve_other_agent() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(skills::install(temp.path(), Agent::Github).unwrap(), 2);
    assert!(!temp.path().join(".claude").exists());
    let before = fs::read(temp.path().join(".arp/installed-skills.json")).unwrap();
    skills::install(temp.path(), Agent::Github).unwrap();
    assert_eq!(
        before,
        fs::read(temp.path().join(".arp/installed-skills.json")).unwrap()
    );
    skills::install(temp.path(), Agent::Claude).unwrap();
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(temp.path().join(".arp/installed-skills.json")).unwrap())
            .unwrap();
    assert_eq!(record.as_object().unwrap().len(), 4);
}

#[test]
fn none_does_not_create_files() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(skills::install(temp.path(), Agent::None).unwrap(), 0);
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn owned_old_version_updates_and_crlf_is_not_a_local_edit() {
    let temp = tempfile::tempdir().unwrap();
    let path = ".github/skills/arp4/SKILL.md";
    skills::install_assets(
        temp.path(),
        &[Asset {
            path,
            body: b"old\nversion\n",
        }],
    )
    .unwrap();
    fs::write(temp.path().join(path), b"old\r\nversion\r\n").unwrap();
    skills::install(temp.path(), Agent::Github).unwrap();
    assert_eq!(
        fs::read(temp.path().join(path)).unwrap(),
        skills::bundled()[2].body
    );
}

#[test]
fn a_late_conflict_prevents_all_updates() {
    let temp = tempfile::tempdir().unwrap();
    let assets = skills::bundled();
    let old: Vec<_> = assets
        .iter()
        .map(|a| Asset {
            path: a.path,
            body: b"old version".as_slice(),
        })
        .collect();
    skills::install_assets(temp.path(), &old).unwrap();
    fs::write(temp.path().join(assets[3].path), b"user edit").unwrap();
    let marker = fs::read(temp.path().join(".arp/installed-skills.json")).unwrap();
    assert!(
        skills::install(temp.path(), Agent::All)
            .unwrap_err()
            .to_string()
            .contains("local changes")
    );
    assert_eq!(
        fs::read(temp.path().join(assets[0].path)).unwrap(),
        b"old version"
    );
    assert_eq!(
        fs::read(temp.path().join(assets[3].path)).unwrap(),
        b"user edit"
    );
    assert_eq!(
        fs::read(temp.path().join(".arp/installed-skills.json")).unwrap(),
        marker
    );
    assert!(!temp.path().join(".arp/rust-skills-install.lock").exists());
}

#[test]
fn unowned_files_and_invalid_ownership_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join(".github/skills/arp4/SKILL.md");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"personal skill").unwrap();
    assert!(skills::install(temp.path(), Agent::Github).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"personal skill");
    fs::write(temp.path().join(".arp/installed-skills.json"), b"not JSON").unwrap();
    assert!(skills::install(temp.path(), Agent::Claude).is_err());
    assert!(!temp.path().join(".claude").exists());
}

#[test]
fn invalid_paths_and_duplicate_assets_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    assert!(
        skills::install_assets(
            temp.path(),
            &[Asset {
                path: "../escaped",
                body: b"x"
            }]
        )
        .is_err()
    );
    let path = ".github/skills/arp4/SKILL.md";
    assert!(
        skills::install_assets(
            temp.path(),
            &[Asset { path, body: b"a" }, Asset { path, body: b"b" }]
        )
        .is_err()
    );
    assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[test]
fn active_lock_prevents_another_installation() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join(".arp")).unwrap();
    let lock = temp.path().join(".arp/rust-skills-install.lock");
    fs::write(&lock, b"another installer").unwrap();
    assert!(skills::install(temp.path(), Agent::All).is_err());
    assert_eq!(fs::read(lock).unwrap(), b"another installer");
    assert!(!temp.path().join(".github").exists());
}

#[test]
fn doctor_and_schemas_work_with_empty_path() {
    let result = binary()
        .env("PATH", "")
        .args(["doctor", "--format", "json"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["release_ready"], false);
    assert_eq!(report["capabilities"]["excel_import"], true);
    assert_eq!(report["capabilities"]["excel_structural_writeback"], false);
    let result = binary()
        .env("PATH", "")
        .args(["documents", "schema", "mappings"])
        .output()
        .unwrap();
    assert!(result.status.success());
    let schema: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(schema["properties"]["schema_version"]["const"], "2");
}

#[test]
fn unsupported_commands_and_unknown_schemas_fail() {
    for args in [
        vec!["documents", "import", "missing.xlsx"],
        vec!["documents", "schema", "unknown"],
        vec!["skills", "install", "--agent", "unknown"],
    ] {
        assert_eq!(binary().args(args).output().unwrap().status.code(), Some(2));
    }
}

#[cfg(windows)]
#[test]
fn marker_write_failure_rolls_back_changed_skills() {
    use std::os::windows::fs::OpenOptionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = ".github/skills/arp4/SKILL.md";
    skills::install_assets(
        temp.path(),
        &[Asset {
            path,
            body: b"old version",
        }],
    )
    .unwrap();
    let marker = temp.path().join(".arp/installed-skills.json");
    let before = fs::read(&marker).unwrap();
    let _guard = fs::OpenOptions::new()
        .read(true)
        .share_mode(1)
        .open(&marker)
        .unwrap();
    assert!(skills::install(temp.path(), Agent::Github).is_err());
    assert_eq!(fs::read(temp.path().join(path)).unwrap(), b"old version");
    assert!(
        !temp
            .path()
            .join(".github/skills/arp4-setup/SKILL.md")
            .exists()
    );
    assert_eq!(fs::read(&marker).unwrap(), before);
}

#[cfg(windows)]
#[test]
fn directory_junction_cannot_redirect_installation() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let junction = temp.path().join(".github");
    let status = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&junction)
        .arg(outside.path())
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let result = skills::install(temp.path(), Agent::Github);
    fs::remove_dir(&junction).unwrap();
    assert!(result.unwrap_err().to_string().contains("link"));
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}
