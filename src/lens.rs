// Google Lens integration - Direct image upload via multipart POST

use std::time::Duration;

/// Performs a Google Lens visual search by uploading the image directly to
/// lens.google.com/v3/upload. This endpoint accepts the raw image bytes,
/// processes them server-side, and responds with a 303 redirect to the
/// search results page. We capture that redirect URL and open it in the
/// default browser. This avoids all third-party hosting and hotlink issues.
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

    log_info!("lens", "Uploading {} bytes directly to lens.google.com/v3/upload", image_bytes.len());

    // Step 3: Build HTTP client - do NOT follow redirects; we need the Location header
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("Client build error: {e}"))?;

    // Step 4: Multipart upload - same field name the Lens web UI uses
    let part = reqwest::multipart::Part::bytes(image_bytes)
        .file_name("image.png")
        .mime_str("image/png")
        .map_err(|e| format!("MIME error: {e}"))?;

    let form = reqwest::multipart::Form::new().part("encoded_image", part);

    let response = client
        .post("https://lens.google.com/v3/upload")
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Upload request failed: {e}"))?;

    let status = response.status();
    log_info!("lens", "Lens upload response status: {}", status.as_u16());

    // Step 5: Expect 303 See Other with Location pointing to google.com/search?vsrid=...
    if status.as_u16() == 303 {
        if let Some(location) = response.headers().get("location") {
            if let Ok(url) = location.to_str() {
                log_info!("lens", "Lens redirect URL obtained successfully");
                open::that(url).map_err(|e| format!("Failed to open browser: {e}"))?;
                return Ok("Google Lens search opened in browser.".to_string());
            }
        }
    }

    // Fallback: open Google Lens homepage - user can paste from clipboard
    log_warn!(
        "lens",
        "Unexpected status {}; falling back to clipboard paste.",
        status.as_u16()
    );
    open::that("https://lens.google.com/")
        .map_err(|e| format!("Failed to open browser: {e}"))?;

    Ok("Opened Google Lens. Image copied to clipboard - press Ctrl+V to search.".to_string())
}
