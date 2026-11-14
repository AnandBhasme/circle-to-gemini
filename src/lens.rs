// Google Lens integration — Direct Image Search via Public Gateway

use reqwest::Url;
use std::time::Duration;

/// Uploads image to a fast public temporary image host and returns its direct URL.
async fn upload_to_public_host(image_bytes: Vec<u8>) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .build()
        .map_err(|e| format!("Client build error: {e}"))?;

    // Attempt 1: freeimage.host API (fast, reliable, returns raw iili.io direct image)
    let form = reqwest::multipart::Form::new()
        .text("key", "6d207e02198a847aa98d0a2a901485a5")
        .text("action", "upload")
        .text("format", "json")
        .part(
            "source",
            reqwest::multipart::Part::bytes(image_bytes.clone())
                .file_name("image.png")
                .mime_str("image/png")
                .map_err(|e| format!("MIME error: {e}"))?,
        );

    if let Ok(resp) = client
        .post("https://freeimage.host/api/1/upload")
        .multipart(form)
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(url) = json.pointer("/image/url").and_then(|v| v.as_str()) {
                    if !url.is_empty() {
                        return Ok(url.to_string());
                    }
                }
            }
        }
    }

    // Attempt 2: uguu.se (open-source temporary file host, 3-hour TTL)
    let form_uguu = reqwest::multipart::Form::new().part(
        "files[]",
        reqwest::multipart::Part::bytes(image_bytes)
            .file_name("image.png")
            .mime_str("image/png")
            .map_err(|e| format!("MIME error: {e}"))?,
    );

    if let Ok(resp) = client
        .post("https://uguu.se/upload")
        .multipart(form_uguu)
        .send()
        .await
    {
        if resp.status().is_success() {
            if let Ok(json) = resp.json::<serde_json::Value>().await {
                if let Some(url) = json.pointer("/files/0/url").and_then(|v| v.as_str()) {
                    if !url.is_empty() {
                        return Ok(url.to_string());
                    }
                }
            }
        }
    }

    Err("Could not upload to temporary host".to_string())
}

/// Initiates Google Lens search for a local image.
/// First copies the image to clipboard (wl-copy) so the user can always paste.
/// Then generates an official Google Lens uploadbyurl link and opens it in the browser.
pub async fn search_with_lens(image_path: &str) -> Result<String, String> {
    // Step 1: Copy cropped image to Wayland clipboard so user can always Ctrl+V
    if let Ok(file) = std::fs::File::open(image_path) {
        let _ = std::process::Command::new("wl-copy")
            .args(["-t", "image/png"])
            .stdin(file)
            .status();
    }

    // Step 2: Read image bytes
    let image_bytes = tokio::fs::read(image_path)
        .await
        .map_err(|e| format!("Failed to read image: {e}"))?;

    // Step 3: Upload to temporary host and construct browser search URL
    match upload_to_public_host(image_bytes).await {
        Ok(public_url) => {
            log_info!("lens", "Image uploaded successfully to gateway: {}", public_url);
            let mut lens_url = Url::parse("https://lens.google.com/uploadbyurl")
                .map_err(|e| e.to_string())?;
            lens_url.query_pairs_mut().append_pair("url", &public_url);

            log_info!("lens", "Launching browser with Lens URL: {}", lens_url.as_str());
            open::that(lens_url.as_str())
                .map_err(|e| format!("Failed to open browser: {e}"))?;

            Ok("Google Lens search opened in browser.".to_string())
        }
        Err(e) => {
            log_warn!("lens", "Gateway upload failed: {}. Falling back to clipboard paste.", e);
            // Fallback: open Google Lens homepage directly with image on clipboard
            open::that("https://lens.google.com/")
                .map_err(|e| format!("Failed to open browser: {e}"))?;

            Ok("Opened Google Lens. Image copied to clipboard - press Ctrl+V to search.".to_string())
        }
    }
}
