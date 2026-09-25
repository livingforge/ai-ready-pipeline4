//! Automatic OCR for imported image assets on Windows.
use anyhow::Result;
use serde_json::{Value, json};

pub fn recognize(_name: &str, bytes: &[u8]) -> Value {
    match run(bytes) {
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

#[cfg(not(windows))]
fn run(_bytes: &[u8]) -> Result<(String, String)> {
    anyhow::bail!("Windows OCR requires Windows")
}

#[cfg(windows)]
fn run(bytes: &[u8]) -> Result<(String, String)> {
    use std::{sync::mpsc, time::Duration};
    let bytes = bytes.to_vec();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(recognize_winrt(&bytes).map_err(anyhow::Error::from));
    });
    receiver
        .recv_timeout(Duration::from_secs(60))
        .map_err(|_| anyhow::anyhow!("OCR timed out after 60 seconds"))?
}

#[cfg(windows)]
fn recognize_winrt(bytes: &[u8]) -> windows::core::Result<(String, String)> {
    use windows::{
        Graphics::Imaging::BitmapDecoder,
        Media::Ocr::OcrEngine,
        Storage::Streams::{DataWriter, InMemoryRandomAccessStream},
        core::{Error, HRESULT},
    };
    let engine = OcrEngine::TryCreateFromUserProfileLanguages().map_err(|_| {
        Error::new(
            HRESULT(0x80004005u32 as i32),
            "No Windows OCR language matches the user profile.",
        )
    })?;
    let stream = InMemoryRandomAccessStream::new()?;
    let writer = DataWriter::CreateDataWriter(&stream)?;
    writer.WriteBytes(bytes)?;
    writer.StoreAsync()?.join()?;
    writer.FlushAsync()?.join()?;
    writer.DetachStream()?;
    stream.Seek(0)?;
    let decoder = BitmapDecoder::CreateAsync(&stream)?.join()?;
    let bitmap = decoder.GetSoftwareBitmapAsync()?.join()?;
    let result = engine.RecognizeAsync(&bitmap)?.join()?;
    let text = result.Text()?.to_string();
    let language = engine.RecognizerLanguage()?.LanguageTag()?.to_string();
    bitmap.Close()?;
    Ok((text, language))
}
