// Gemini API client with streaming support

use base64::Engine;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize)]
struct GeminiRequest {
    #[serde(rename = "systemInstruction", skip_serializing_if = "Option::is_none")]
    system_instruction: Option<Content>,
    contents: Vec<Content>,
    #[serde(rename = "generationConfig", skip_serializing_if = "Option::is_none")]
    generation_config: Option<GenerationConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Content {
    parts: Vec<Part>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum Part {
    Text {
        text: String,
    },
    InlineData {
        #[serde(rename = "inlineData")]
        inline_data: InlineData,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InlineData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

#[derive(Debug, Clone, Serialize)]
struct GenerationConfig {
    temperature: f32,
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct StreamResponse {
    candidates: Option<Vec<Candidate>>,
}

#[derive(Debug, Clone, Deserialize)]
struct Candidate {
    content: Option<CandidateContent>,
}

#[derive(Debug, Clone, Deserialize)]
struct CandidateContent {
    parts: Option<Vec<ResponsePart>>,
}

#[derive(Debug, Clone, Deserialize)]
struct ResponsePart {
    text: Option<String>,
}

/// A chunk of streaming response from Gemini
#[derive(Debug, Clone)]
pub enum GeminiChunk {
    Text(String),
    Done,
    Error(String),
}

/// A message in a multi-turn chat conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String, // "user" or "model"
    pub content: String,
}

const SMART_CASCADE: &[&str] = &[
    "gemini-3.8-flash",
    "gemini-3.6-flash",
    "gemini-3.5-flash-lite",
    "gemini-3.1-flash-lite",
];

const FAST_CASCADE: &[&str] = &[
    "gemini-3.5-flash-lite",
    "gemini-3.1-flash-lite",
];

/// Send image + multi-turn conversation history to Gemini and stream response back.
/// In "smart" mode: cascades 3.8-flash -> 3.6-flash -> 3.5-flash-lite -> 3.1-flash-lite.
/// In "fast" mode: cascades 3.5-flash-lite -> 3.1-flash-lite.
pub async fn stream_gemini_response(
    api_key: String,
    mode: String,
    image_path: String,
    history: Vec<ChatMessage>,
    tx: mpsc::UnboundedSender<GeminiChunk>,
) {
    let candidate_models: Vec<String> = if mode == "fast" {
        FAST_CASCADE.iter().map(|&s| s.to_string()).collect()
    } else {
        SMART_CASCADE.iter().map(|&s| s.to_string()).collect()
    };

    let mut last_error = String::new();
    let mut succeeded = false;

    log_info!("gemini", "Initiating stream request (mode: {}, candidates: {:?})", mode, candidate_models);

    for (attempt, current_model) in candidate_models.iter().enumerate() {
        if attempt > 0 {
            log_warn!(
                "gemini",
                "Model {} failed. Cascading to {}...",
                candidate_models[attempt - 1],
                current_model
            );
            let notice = format!(
                "_[Model {} was busy (503). Cascading to {}...]_\n\n",
                candidate_models[attempt - 1], current_model
            );
            let _ = tx.send(GeminiChunk::Text(notice));
        }

        match stream_inner(&api_key, current_model, &image_path, &history, &tx).await {
            Ok(()) => {
                log_info!("gemini", "Model {} completed stream successfully", current_model);
                succeeded = true;
                break;
            }
            Err(e) => {
                let err_str = e.to_string();
                log_warn!("gemini", "Model {} failed: {}", current_model, err_str);
                last_error = err_str.clone();

                let is_recoverable = err_str.contains("503")
                    || err_str.contains("429")
                    || err_str.contains("404")
                    || err_str.contains("UNAVAILABLE")
                    || err_str.contains("RESOURCE_EXHAUSTED")
                    || err_str.contains("NOT_FOUND");

                if is_recoverable {
                    tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;
                    continue;
                } else {
                    break;
                }
            }
        }
    }

    if !succeeded {
        log_error!("gemini", "All model candidates failed. Last error: {}", last_error);
        tx.send(GeminiChunk::Error(last_error)).ok();
    }
    tx.send(GeminiChunk::Done).ok();
}

async fn stream_inner(
    api_key: &str,
    model: &str,
    image_path: &str,
    history: &[ChatMessage],
    tx: &mpsc::UnboundedSender<GeminiChunk>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let image_data = fs::read(image_path)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&image_data);

    let mime_type = if image_path.ends_with(".jpg") || image_path.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "image/png"
    };

    let mut contents = Vec::new();
    for (i, msg) in history.iter().enumerate() {
        if i == 0 && msg.role == "user" {
            contents.push(Content {
                role: Some("user".to_string()),
                parts: vec![
                    Part::InlineData {
                        inline_data: InlineData {
                            mime_type: mime_type.to_string(),
                            data: b64.clone(),
                        },
                    },
                    Part::Text {
                        text: msg.content.clone(),
                    },
                ],
            });
        } else {
            contents.push(Content {
                role: Some(msg.role.clone()),
                parts: vec![Part::Text {
                    text: msg.content.clone(),
                }],
            });
        }
    }

    let request = GeminiRequest {
        system_instruction: Some(Content {
            role: None,
            parts: vec![Part::Text {
                text: "You are a helpful visual assistant. Format your response in clean Markdown. For mathematical formulas, equations, and matrices, use standard LaTeX notation inside display math ($$ ... $$) or inline math ($ ... $). For matrices, use standard LaTeX environments like \\begin{pmatrix} ... \\end{pmatrix} or \\begin{bmatrix} ... \\end{bmatrix} inside display math blocks.".to_string(),
            }],
        }),
        contents,
        generation_config: Some(GenerationConfig {
            temperature: 0.7,
            max_output_tokens: 4096,
        }),
    };

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent?alt=sse&key={}",
        model, api_key
    );

    let client = Client::new();
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Gemini API error {}: {}", status, body).into());
    }

    // Parse SSE stream
    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        let text = String::from_utf8_lossy(&chunk);
        buffer.push_str(&text);
        buffer = buffer.replace("\r\n", "\n");

        // Process complete SSE events in buffer
        while let Some(pos) = buffer.find("\n\n") {
            let event = buffer[..pos].to_string();
            buffer = buffer[pos + 2..].to_string();

            for line in event.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    let data = data.trim();
                    if data == "[DONE]" {
                        return Ok(());
                    }
                    if let Ok(resp) = serde_json::from_str::<StreamResponse>(data) {
                        if let Some(candidates) = resp.candidates {
                            for candidate in candidates {
                                if let Some(content) = candidate.content {
                                    if let Some(parts) = content.parts {
                                        for part in parts {
                                            if let Some(t) = part.text {
                                                if !t.is_empty() {
                                                    tx.send(GeminiChunk::Text(t)).ok();
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Flush any remaining data in buffer
    let remaining = buffer.replace("\r\n", "\n");
    for line in remaining.lines() {
        if let Some(data) = line.strip_prefix("data: ") {
            let data = data.trim();
            if data != "[DONE]" {
                if let Ok(resp) = serde_json::from_str::<StreamResponse>(data) {
                    if let Some(candidates) = resp.candidates {
                        for candidate in candidates {
                            if let Some(content) = candidate.content {
                                if let Some(parts) = content.parts {
                                    for part in parts {
                                        if let Some(t) = part.text {
                                            if !t.is_empty() {
                                                tx.send(GeminiChunk::Text(t)).ok();
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Non-streaming: send image + prompt and return the full response.
pub async fn ask_gemini(
    api_key: &str,
    model: &str,
    image_path: &str,
    prompt: &str,
) -> Result<String, String> {
    let image_data = fs::read(image_path).map_err(|e| e.to_string())?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&image_data);

    let mime_type = if image_path.ends_with(".jpg") || image_path.ends_with(".jpeg") {
        "image/jpeg"
    } else {
        "image/png"
    };

    let request = GeminiRequest {
        system_instruction: Some(Content {
            role: None,
            parts: vec![Part::Text {
                text: "You are a helpful visual assistant. Format your response in clean Markdown. For any mathematical formulas, equations, or variables, do NOT use LaTeX dollar signs ($ or $$). Instead use standard readable Unicode characters (like xᵢ, σᵢ, ∀, ∃, ≤, ≥, ≠, →, ², ³, ·) or format multiline formulas in clean code blocks.".to_string(),
            }],
        }),
        contents: vec![Content {
            parts: vec![
                Part::InlineData {
                    inline_data: InlineData {
                        mime_type: mime_type.to_string(),
                        data: b64,
                    },
                },
                Part::Text {
                    text: prompt.to_string(),
                },
            ],
            role: Some("user".to_string()),
        }],
        generation_config: Some(GenerationConfig {
            temperature: 0.7,
            max_output_tokens: 4096,
        }),
    };

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );

    let client = Client::new();
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .json(&request)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Gemini API error {}: {}", status, body));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    // Extract text from response
    let text = body["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("(No response)")
        .to_string();

    Ok(text)
}
