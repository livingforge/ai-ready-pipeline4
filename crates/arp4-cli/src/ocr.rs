//! Automatic OCR for imported image assets on Windows.
use crate::data::read;
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};

pub fn recognize(name: &str, bytes: &[u8]) -> Value {
    match run(name, bytes) {
        Ok((text, language)) => json!({
            "status":"available",
            "text":text,
            "reason":format!("Windows OCR ({language}) executed on imported image")
        }),
        Err(error) => json!({
            "status":"unavailable",
            "text":"",
            "reason":format!("Windows OCR could not process imported image: {error:#}")
        }),
    }
}

fn run(name: &str, bytes: &[u8]) -> Result<(String, String)> {
    ensure!(cfg!(windows), "Windows OCR requires Windows");
    let stage = tempfile::tempdir()?;
    let extension = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| value.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("bin");
    let image = stage.path().join(format!("image.{extension}"));
    let script = stage.path().join("ocr.ps1");
    let result = stage.path().join("result.json");
    let log = stage.path().join("stderr.txt");
    fs::write(&image, bytes)?;
    fs::write(&script, include_bytes!("ocr.ps1"))?;
    let mut command = std::process::Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .arg("-ImagePath")
        .arg(&image)
        .arg("-ResultPath")
        .arg(&result)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(fs::File::create(&log)?));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().context("cannot start Windows OCR")?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("OCR timed out after 60 seconds");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    ensure!(
        status.success(),
        "{}",
        String::from_utf8_lossy(&fs::read(log)?).trim()
    );
    let value = read(&result, None)?;
    let text = value["text"]
        .as_str()
        .context("OCR text missing")?
        .to_owned();
    let language = value["language"]
        .as_str()
        .context("OCR language missing")?
        .to_owned();
    Ok((text, language))
}
