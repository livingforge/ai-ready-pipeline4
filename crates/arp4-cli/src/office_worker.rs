//! Disposable worker processes of this executable that drive desktop Office,
//! so a stuck COM call is ended by killing the worker and its Office process.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::Path,
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

/// Runs this executable with `flag` and `request`, writing its standard error to
/// `log`. Returns `None` when it does not finish within `timeout` seconds; the
/// worker is then killed.
pub(crate) fn run(
    flag: &str,
    request: &Path,
    log: &Path,
    timeout: u64,
) -> Result<Option<ExitStatus>> {
    let current = std::env::current_exe()?;
    // Test binaries live in target/<profile>/deps, next to the CLI's folder.
    let executable = if current.parent().and_then(|path| path.file_name()) == Some("deps".as_ref())
    {
        current
            .parent()
            .context("worker executable directory")?
            .parent()
            .context("worker build directory")?
            .join("arp4.exe")
    } else {
        current
    };
    ensure!(
        executable.is_file(),
        "Rust Office worker executable was not found: {}",
        executable.display()
    );
    let mut command = Command::new(&executable);
    command
        .arg(flag)
        .arg(request)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(fs::File::create(log)?));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().context("cannot start Rust Office worker")?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if started.elapsed() >= Duration::from_secs(timeout) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
