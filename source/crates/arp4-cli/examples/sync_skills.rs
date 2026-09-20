use anyhow::{Result, ensure};
use std::{env, fs, path::Path};

fn main() -> Result<()> {
    let check = env::args().skip(1).any(|arg| arg == "--check");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for asset in arp4_cli::skills::bundled() {
        let path = root.join(asset.path);
        if check {
            let actual = fs::read_to_string(&path)?.replace("\r\n", "\n");
            ensure!(
                actual.as_bytes() == asset.body,
                "Stale skill: {}",
                path.display()
            );
        } else {
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(path, asset.body)?;
        }
    }
    Ok(())
}
