// Google Lens integration — Chromium upload protocol + clipboard fallback

use std::time::{SystemTime, UNIX_EPOCH};

/// Uploads an image to Google Lens and returns the results URL.
pub async fn upload_to_lens(image_path: &str) -> Result<String, String> {
    let image_data = tokio::fs::read(image_path)
        .await
        .map_err(|e| format!("Failed to read image: {e}"))?;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let upload_url = format!("https://lens.google.com/upload?ep=ccm&s=&st={now_ms}");

    let form = reqwest::multipart::Form::new().part(
        "encoded_image",
        reqwest::multipart::Part::bytes(image_data)
            .file_name("image.png")
            .mime_str("image/png")
            .map_err(|e| format!("MIME error: {e}"))?,
    );

    let resp = client
        .post(&upload_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36",
        )
        .header(
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8",
        )
        .header("Origin", "https://lens.google.com")
        .header("Referer", "https://lens.google.com/")
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Lens upload failed: {e}"))?;

    if let Some(location) = resp.headers().get("location") {
        if let Ok(url) = location.to_str() {
            if !url.is_empty() {
                let full_url = if url.starts_with('/') {
                    format!("https://lens.google.com{url}")
                } else {
                    url.to_string()
                };
                return Ok(full_url);
            }
        }
    }

    if resp.status().is_success() || resp.status().is_redirection() {
        return Ok(resp.url().to_string());
    }

    Err(format!("Lens upload returned status: {}", resp.status()))
}

/// Copies image to clipboard and opens Google Lens in browser.
/// Uses direct upload if available, otherwise opens Google Lens home for instant paste.
pub async fn search_with_lens(image_path: &str) -> Result<String, String> {
    // Step 1: Copy cropped image to Wayland clipboard so user can always Ctrl+V
    if let Ok(file) = std::fs::File::open(image_path) {
        let _ = std::process::Command::new("wl-copy")
            .args(["-t", "image/png"])
            .stdin(file)
            .status();
    }

    // Step 2: Try the official Lens upload endpoint
    match upload_to_lens(image_path).await {
        Ok(url) => {
            open::that(&url).map_err(|e| format!("Failed to open browser: {e}"))?;
            Ok("Google Lens search opened in browser.".to_string())
        }
        Err(_) => {
            // Step 3: Fallback - open Lens homepage where image is already on clipboard
            open::that("https://lens.google.com/").map_err(|e| format!("Failed to open browser: {e}"))?;
            Ok("Opened Google Lens. Image copied to clipboard - press Ctrl+V to search.".to_string())
        }
    }
}
