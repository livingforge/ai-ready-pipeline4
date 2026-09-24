//! Native Excel rendering, isolated from the scalar/structural writeback engine.
use crate::data::{encoded, hash, read};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{fs, path::Path};

pub fn render(
    source: &Path,
    sheet: &str,
    range: Option<&str>,
    timeout: u64,
) -> Result<(Vec<u8>, Value)> {
    ensure!(
        cfg!(windows),
        "Excel rendering requires Windows and desktop Microsoft Excel"
    );
    ensure!(
        (1..=600).contains(&timeout),
        "render timeout must be 1..600 seconds"
    );
    let original = fs::read(source)?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(&original))?;
    // These can execute or refresh on open independently of VBA event suppression.
    for index in 0..archive.len() {
        let entry = archive.by_index(index)?;
        let name = entry.name().to_ascii_lowercase();
        ensure!(
            !name.starts_with("xl/macrosheets/") && name != "xl/connections.xml",
            "rendering workbooks with Excel 4.0 macros or data connections is unsupported"
        );
    }
    let stage = tempfile::tempdir()?;
    let request_path = stage.path().join("request.json");
    let result_path = stage.path().join("result.json");
    let output = stage.path().join("render.png");
    let script = stage.path().join("render.ps1");
    fs::write(&script, include_bytes!("render.ps1"))?;
    fs::write(
        &request_path,
        encoded(
            &json!({"source":source,"sheet":sheet,"range":range,"output":output,"result":result_path}),
        ),
    )?;
    let log = stage.path().join("stderr.txt");
    let mut command = std::process::Command::new("powershell.exe");
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-STA",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script)
        .arg("-Request")
        .arg(&request_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(fs::File::create(&log)?));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command
        .spawn()
        .context("cannot start Windows PowerShell Excel renderer")?;
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= std::time::Duration::from_secs(timeout) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("Excel render timed out; no image or region was adopted");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    ensure!(
        hash(&fs::read(source)?) == hash(&original),
        "original changed during rendering; output discarded"
    );
    ensure!(
        status.success(),
        "Excel render failed: {}",
        String::from_utf8_lossy(&fs::read(log)?)
    );
    let result = read(&result_path, None)?;
    ensure!(
        result["engine"] == "excel-com" && result["sheet"] == sheet,
        "unexpected renderer result"
    );
    let bytes = fs::read(output)?;
    super::validate_image_asset("assets/render.png", &bytes)?;
    ensure!(
        bytes.len() >= 33 && &bytes[12..16] == b"IHDR",
        "invalid rendered PNG"
    );
    let width = u32::from_be_bytes(bytes[16..20].try_into()?);
    let height = u32::from_be_bytes(bytes[20..24].try_into()?);
    ensure!(
        result["width"] == width && result["height"] == height && width > 1 && height > 1,
        "render dimensions mismatch"
    );
    Ok((bytes, result))
}
