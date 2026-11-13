// Google Lens integration — async upload + browser open

/// Uploads an image to Google Lens and returns the results URL.
pub async fn upload_to_lens(image_path: &str) -> Result<String, String> {
    let image_data = tokio::fs::read(image_path)
        .await
        .map_err(|e| format!("Failed to read image: {e}"))?;

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let form = reqwest::multipart::Form::new().part(
        "encoded_image",
        reqwest::multipart::Part::bytes(image_data)
            .file_name("image.png")
            .mime_str("image/png")
            .unwrap(),
    );

    let resp = client
        .post("https://lens.google.com/v3/upload")
        .header(
            "User-Agent",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36",
        )
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("Lens upload failed: {e}"))?;

    // Google Lens returns 303 with a Location header pointing to the results page
    if let Some(location) = resp.headers().get("location") {
        if let Ok(url) = location.to_str() {
            if !url.is_empty() {
                return Ok(url.to_string());
            }
        }
    }

    // Fallback: use the final URL
    Ok(resp.url().to_string())
}

/// Upload image to Google Lens and open the results in the default browser.
pub async fn search_with_lens(image_path: &str) -> Result<(), String> {
    let url = upload_to_lens(image_path).await?;
    open::that(&url).map_err(|e| format!("Failed to open browser: {e}"))?;
    Ok(())
}
