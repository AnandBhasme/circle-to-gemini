// Core Application - iced daemon managing multi-window overlay and assistant card

use crate::config::Config;
use crate::gemini::{self, GeminiChunk};
use crate::lens;
use crate::overlay::{OverlayCanvas, OverlayMessage, OverlayState, SelectionRect};
use crate::tray::TrayCommand;

use arboard::Clipboard;
use iced::widget::canvas::Canvas;
use iced::widget::{
    button, column, container, horizontal_space, image, markdown, row, scrollable, text,
    text_editor, text_input, Column,
};
use iced::window;
use iced::{
    alignment, Color, Element, Fill, Length, Point, Size, Subscription, Task, Theme,
};
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};

/// Global trigger channel for IPC or tray activation
pub type TriggerSender = mpsc::UnboundedSender<TrayCommand>;
pub type TriggerReceiver = mpsc::UnboundedReceiver<TrayCommand>;

pub struct App {
    /// Window registry mapping each window::Id to its specific UI state
    windows: BTreeMap<window::Id, WindowState>,
    /// Persistent configuration
    config: Config,
    /// Shared tray receiver stream
    tray_rx: Arc<Mutex<Option<TriggerReceiver>>>,
    /// Currently active assistant window ID (if open)
    active_assistant_id: Option<window::Id>,
}

#[derive(Debug)]
pub enum WindowState {
    Overlay {
        screenshot_path: String,
        drawing: OverlayState,
    },
    Assistant(AssistantState),
}

#[derive(Debug, Clone)]
pub struct ChatMessageItem {
    pub role: ChatRole,
    pub content: String,
    pub md_items: Vec<markdown::Item>,
}

impl ChatMessageItem {
    pub fn new(role: ChatRole, content: String) -> Self {
        let md_items = if role == ChatRole::Model {
            let sanitized = sanitize_markdown_math(&content);
            markdown::parse(&sanitized).collect()
        } else {
            Vec::new()
        };
        Self {
            role,
            content,
            md_items,
        }
    }

    pub fn push_str(&mut self, s: &str) {
        self.content.push_str(s);
        if self.role == ChatRole::Model {
            let sanitized = sanitize_markdown_math(&self.content);
            self.md_items = markdown::parse(&sanitized).collect();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Model,
}

#[derive(Debug)]
pub struct AssistantState {
    pub image_path: String,
    pub prompt_input: String,
    pub messages: Vec<ChatMessageItem>,
    pub is_loading: bool,
    pub show_settings: bool,
    pub api_key_input: String,
    pub status_message: Option<String>,
    pub extracted_text: Option<text_editor::Content<iced::Renderer>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    // Tray / Trigger events
    Tray(TrayCommand),
    TriggerCapture,
    ScreenCaptured(Result<String, String>),

    // Window management
    WindowOpened(window::Id, WindowKindTag),
    WindowClosed(window::Id),

    // Overlay events
    OverlayAction(window::Id, OverlayMessage),
    FinishSelection(window::Id, SelectionRect),
    CancelOverlay(window::Id),

    // Assistant card actions
    PromptChanged(window::Id, String),
    SendPrompt(window::Id),
    ActionExplain(window::Id),
    ActionCopyText(window::Id),
    ActionLens(window::Id),
    ActionContinueInWeb(window::Id),
    AutoCopiedTranscript(window::Id, String),
    CopyResponse(window::Id),
    CopyMessage(window::Id, usize),
    CopyAllChat(window::Id),
    CopyPrompt(window::Id),

    // Extracted OCR text interaction
    EditorAction(window::Id, text_editor::Action),
    CopyExtractedSelection(window::Id),
    CopyExtractedAll(window::Id),
    InsertExtractedToPrompt(window::Id),
    CloseExtractedText(window::Id),
    UpdateStatus(window::Id, String),

    // Gemini streaming
    GeminiStreamChunk(window::Id, GeminiChunk),
    OpenMarkdownUrl(String),
    NoOp,

    // Settings
    ToggleSettings(window::Id),
    ApiKeyInputChanged(window::Id, String),
    SaveSettings(window::Id),
    OpenApiKeyUrl,
    SetMode(String),
    WindowEvent(window::Id, window::Event),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKindTag {
    Overlay,
    Assistant,
}

impl App {
    pub fn new(tray_rx: TriggerReceiver) -> (Self, Task<Message>) {
        let config = Config::load();
        let app = Self {
            windows: BTreeMap::new(),
            config,
            tray_rx: Arc::new(Mutex::new(Some(tray_rx))),
            active_assistant_id: None,
        };

        (app, Task::none())
    }

    pub fn title(&self, window_id: window::Id) -> String {
        match self.windows.get(&window_id) {
            Some(WindowState::Overlay { .. }) => "Circle to Gemini Selection".to_string(),
            Some(WindowState::Assistant(_)) => "Gemini Screen Assistant".to_string(),
            None => "Circle to Gemini".to_string(),
        }
    }

    pub fn theme(&self, _window_id: window::Id) -> Theme {
        Theme::Dark
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let tray_rx = Arc::clone(&self.tray_rx);

        let tray_sub = Subscription::run_with_id(
            "tray_events",
            iced::stream::channel(100, move |mut output| async move {
                use iced::futures::SinkExt;
                let mut guard = tray_rx.lock().await;
                if let Some(mut rx) = guard.take() {
                    drop(guard);
                    while let Some(cmd) = rx.recv().await {
                        let _ = output.send(Message::Tray(cmd)).await;
                    }
                }
            }),
        );

        let window_events_sub = window::events().map(|(id, event)| Message::WindowEvent(id, event));
        let window_close_sub = window::close_events().map(Message::WindowClosed);

        Subscription::batch(vec![tray_sub, window_events_sub, window_close_sub])
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tray(cmd) => match cmd {
                TrayCommand::Activate => self.start_capture(),
                TrayCommand::OpenSettings => {
                    if let Some(id) = self.active_assistant_id {
                        if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                            state.show_settings = true;
                            state.api_key_input = self.config.gemini_api_key.clone();
                        }
                    } else {
                        return self.start_capture();
                    }
                    Task::none()
                }
                TrayCommand::Quit => iced::exit(),
            },

            Message::TriggerCapture => self.start_capture(),

            Message::ScreenCaptured(result) => match result {
                Ok(path) => {
                    let mut settings = window::Settings {
                        size: Size::new(1920.0, 1080.0),
                        decorations: false,
                        transparent: false,
                        level: window::Level::AlwaysOnTop,
                        ..Default::default()
                    };
                    settings.platform_specific.application_id = "circle-to-gemini-overlay".to_string();
                    if let Ok(icon) = load_window_icon() {
                        settings.icon = Some(icon);
                    }

                    let (id, open_task) = window::open(settings);

                    self.windows.insert(
                        id,
                        WindowState::Overlay {
                            screenshot_path: path,
                            drawing: OverlayState::default(),
                        },
                    );

                    open_task.map(move |id| Message::WindowOpened(id, WindowKindTag::Overlay))
                }
                Err(err) => {
                    eprintln!("Capture error: {err}");
                    Task::none()
                }
            },

            Message::WindowOpened(id, kind) => {
                match kind {
                    WindowKindTag::Overlay => {
                        window::change_mode(id, window::Mode::Fullscreen)
                    }
                    WindowKindTag::Assistant => {
                        self.active_assistant_id = Some(id);
                        Task::none()
                    }
                }
            }

            Message::WindowClosed(id) => {
                self.windows.remove(&id);
                if self.active_assistant_id == Some(id) {
                    self.active_assistant_id = None;
                }
                Task::none()
            }

            Message::WindowEvent(id, event) => {
                if self.active_assistant_id == Some(id) {
                    match event {
                        window::Event::Moved(point) => {
                            self.config.window_x = Some(point.x);
                            self.config.window_y = Some(point.y);
                            self.config.save();
                        }
                        window::Event::Resized(size) => {
                            self.config.window_width = Some(size.width);
                            self.config.window_height = Some(size.height);
                            self.config.save();
                        }
                        _ => {}
                    }
                }
                Task::none()
            }

            Message::CancelOverlay(id) => {
                self.windows.remove(&id);
                window::close(id)
            }

            Message::OverlayAction(id, action) => {
                if let Some(WindowState::Overlay { drawing, .. }) = self.windows.get_mut(&id) {
                    match action {
                        OverlayMessage::StartDraw(pos, w, h) => {
                            drawing.canvas_width = w;
                            drawing.canvas_height = h;
                            drawing.is_drawing = true;
                            drawing.rect_start = Some(pos);
                            drawing.rect_current = Some(pos);
                            drawing.points.clear();
                            drawing.points.push(pos);
                        }
                        OverlayMessage::Drawing(pos, w, h) => {
                            drawing.canvas_width = w;
                            drawing.canvas_height = h;
                            if drawing.is_drawing {
                                drawing.rect_current = Some(pos);
                                drawing.points.push(pos);
                            }
                        }
                        OverlayMessage::EndDraw(pos, w, h) => {
                            drawing.canvas_width = w;
                            drawing.canvas_height = h;
                            drawing.is_drawing = false;
                            drawing.rect_current = Some(pos);
                            drawing.points.push(pos);
                            drawing.selection_complete = true;

                            if let Some(rect) = drawing.selection_bounds() {
                                return Task::done(Message::FinishSelection(id, rect));
                            }
                        }
                        OverlayMessage::Cancel => {
                            return Task::done(Message::CancelOverlay(id));
                        }
                    }
                }
                Task::none()
            }

            Message::FinishSelection(overlay_id, rect) => {
                let screenshot_path = if let Some(WindowState::Overlay { screenshot_path, .. }) =
                    self.windows.get(&overlay_id)
                {
                    screenshot_path.clone()
                } else {
                    return Task::none();
                };

                let cropped_path = match crop_image(&screenshot_path, rect) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("Failed to crop image: {e}");
                        return window::close(overlay_id);
                    }
                };

                self.windows.remove(&overlay_id);

                let width = self.config.window_width.unwrap_or(560.0);
                let height = self.config.window_height.unwrap_or(720.0);

                let mut settings = window::Settings {
                    size: Size::new(width, height),
                    decorations: true,
                    ..Default::default()
                };
                settings.platform_specific.application_id = "circle-to-gemini-assistant".to_string();
                if let (Some(x), Some(y)) = (self.config.window_x, self.config.window_y) {
                    settings.position = window::Position::Specific(Point::new(x, y));
                }
                if let Ok(icon) = load_window_icon() {
                    settings.icon = Some(icon);
                }

                let (assistant_id, open_task) = window::open(settings);

                let initial_state = AssistantState {
                    image_path: cropped_path,
                    prompt_input: String::new(),
                    messages: Vec::new(),
                    is_loading: false,
                    show_settings: false,
                    api_key_input: self.config.gemini_api_key.clone(),
                    status_message: None,
                    extracted_text: None,
                };

                self.windows
                    .insert(assistant_id, WindowState::Assistant(initial_state));
                self.active_assistant_id = Some(assistant_id);

                Task::batch(vec![
                    window::close(overlay_id),
                    open_task.map(move |id| Message::WindowOpened(id, WindowKindTag::Assistant)),
                ])
            }

            Message::PromptChanged(id, val) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.prompt_input = val;
                }
                Task::none()
            }

            Message::SendPrompt(id) => {
                let prompt = if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    let p = state.prompt_input.trim().to_string();
                    state.prompt_input.clear();
                    p
                } else {
                    return Task::none();
                };

                if prompt.is_empty() {
                    return Task::none();
                }

                self.query_gemini(id, prompt)
            }

            Message::ActionExplain(id) => {
                self.query_gemini(
                    id,
                    "Identify and explain what is shown in this image in detail.".to_string(),
                )
            }

            Message::ActionCopyText(id) => {
                let image_path = if let Some(WindowState::Assistant(state)) = self.windows.get(&id)
                {
                    state.image_path.clone()
                } else {
                    return Task::none();
                };

                let ocr_result = std::process::Command::new("tesseract")
                    .args([&image_path, "stdout", "-l", "eng"])
                    .output();

                match ocr_result {
                    Ok(output) if output.status.success() => {
                        let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if !text.is_empty() {
                            if let Ok(mut cb) = Clipboard::new() {
                                let _ = cb.set_text(&text);
                            }
                            let _ = std::process::Command::new("wl-copy").arg(&text).status();
                            if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                                state.extracted_text = Some(text_editor::Content::with_text(&text));
                                state.status_message =
                                    Some("Text extracted. Select specific text to copy, or use Copy Selection / Copy All.".to_string());
                            }
                        } else if let Some(WindowState::Assistant(state)) =
                            self.windows.get_mut(&id)
                        {
                            state.status_message =
                                Some("No readable text detected in selection.".to_string());
                        }
                    }
                    _ => {
                        return self.query_gemini(
                            id,
                            "Extract all readable text from this image exactly as written."
                                .to_string(),
                        );
                    }
                }

                Task::none()
            }

            Message::EditorAction(id, action) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    if let Some(content) = &mut state.extracted_text {
                        content.perform(action);
                    }
                }
                Task::none()
            }

            Message::CopyExtractedSelection(id) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    if let Some(content) = &state.extracted_text {
                        let to_copy = content.selection().unwrap_or_else(|| content.text());
                        if !to_copy.is_empty() {
                            if let Ok(mut cb) = Clipboard::new() {
                                let _ = cb.set_text(&to_copy);
                            }
                            let _ = std::process::Command::new("wl-copy").arg(&to_copy).status();
                            let is_partial = content.selection().is_some();
                            state.status_message = Some(if is_partial {
                                "Selected text copied to clipboard.".to_string()
                            } else {
                                "All text copied to clipboard.".to_string()
                            });
                        }
                    }
                }
                Task::none()
            }

            Message::CopyExtractedAll(id) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    if let Some(content) = &state.extracted_text {
                        let text = content.text();
                        if !text.is_empty() {
                            if let Ok(mut cb) = Clipboard::new() {
                                let _ = cb.set_text(&text);
                            }
                            let _ = std::process::Command::new("wl-copy").arg(&text).status();
                            state.status_message = Some("All text copied to clipboard.".to_string());
                        }
                    }
                }
                Task::none()
            }

            Message::InsertExtractedToPrompt(id) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    if let Some(content) = &state.extracted_text {
                        let to_insert = content.selection().unwrap_or_else(|| content.text());
                        if !to_insert.is_empty() {
                            if state.prompt_input.is_empty() {
                                state.prompt_input = to_insert;
                            } else {
                                state.prompt_input.push_str(&format!(" {}", to_insert));
                            }
                            state.status_message = Some("Inserted text into prompt bar.".to_string());
                        }
                    }
                }
                Task::none()
            }

            Message::CloseExtractedText(id) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.extracted_text = None;
                }
                Task::none()
            }

            Message::UpdateStatus(id, msg) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.status_message = Some(msg);
                }
                Task::none()
            }

            Message::ActionLens(id) => {
                let image_path = if let Some(WindowState::Assistant(state)) = self.windows.get(&id)
                {
                    state.image_path.clone()
                } else {
                    return Task::none();
                };

                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.status_message = Some("Opening Google Lens search...".to_string());
                }

                Task::perform(
                    async move {
                        lens::search_with_lens(&image_path).await
                    },
                    move |res| match res {
                        Ok(msg) => Message::UpdateStatus(id, msg),
                        Err(err) => Message::UpdateStatus(id, format!("Lens error: {err}")),
                    },
                )
            }

            Message::ActionContinueInWeb(id) => {
                let (image_path, transcript) = if let Some(WindowState::Assistant(state)) = self.windows.get(&id) {
                    let mut transcript = String::new();
                    for msg in &state.messages {
                        let speaker = match msg.role {
                            ChatRole::User => "User",
                            ChatRole::Model => "Gemini",
                        };
                        transcript.push_str(&format!("{}:\n{}\n\n", speaker, msg.content));
                    }
                    if transcript.is_empty() && !state.prompt_input.is_empty() {
                        transcript = state.prompt_input.clone();
                    }
                    (state.image_path.clone(), transcript.trim().to_string())
                } else {
                    return Task::none();
                };

                // Step 1: Copy screenshot to Wayland clipboard using wl-copy
                if let Ok(file) = std::fs::File::open(&image_path) {
                    let _ = std::process::Command::new("wl-copy")
                        .args(["-t", "image/png"])
                        .stdin(file)
                        .status();
                }

                // Open Gemini in default browser
                let _ = open::that("https://gemini.google.com/app");

                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    if transcript.is_empty() {
                        state.status_message = Some(
                            "Step 1: Image copied! Press Ctrl+V in Gemini to attach image.".to_string(),
                        );
                    } else {
                        state.status_message = Some(
                            "Step 1: Image copied! Press Ctrl+V in Gemini to attach. (Transcript copies in 4s...)".to_string(),
                        );
                    }
                }

                if !transcript.is_empty() {
                    // Step 2: Wait 4.5 seconds for the user to switch to browser and paste image,
                    // then automatically put the chat transcript on the clipboard!
                    Task::perform(
                        async move {
                            tokio::time::sleep(tokio::time::Duration::from_millis(4500)).await;
                            transcript
                        },
                        move |text| Message::AutoCopiedTranscript(id, text),
                    )
                } else {
                    Task::none()
                }
            }

            Message::AutoCopiedTranscript(id, text) => {
                if let Ok(mut cb) = Clipboard::new() {
                    let _ = cb.set_text(&text);
                }
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.status_message = Some(
                        "Step 2: Transcript copied! Press Ctrl+V in Gemini to paste your question/chat.".to_string(),
                    );
                }
                Task::none()
            }

            Message::CopyMessage(id, idx) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get(&id) {
                    if let Some(msg) = state.messages.get(idx) {
                        if !msg.content.is_empty() {
                            if let Ok(mut cb) = Clipboard::new() {
                                let _ = cb.set_text(&msg.content);
                            }
                        }
                    }
                }
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.status_message = Some("Message copied to clipboard.".to_string());
                }
                Task::none()
            }

            Message::CopyAllChat(id) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get(&id) {
                    let mut transcript = String::new();
                    for msg in &state.messages {
                        let speaker = match msg.role {
                            ChatRole::User => "User",
                            ChatRole::Model => "Gemini",
                        };
                        transcript.push_str(&format!("{}:\n{}\n\n", speaker, msg.content));
                    }
                    if !transcript.is_empty() {
                        if let Ok(mut cb) = Clipboard::new() {
                            let _ = cb.set_text(transcript.trim());
                        }
                    }
                }
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.status_message = Some("Full chat transcript copied to clipboard.".to_string());
                }
                Task::none()
            }

            Message::CopyPrompt(id) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get(&id) {
                    if !state.prompt_input.is_empty() {
                        if let Ok(mut cb) = Clipboard::new() {
                            let _ = cb.set_text(&state.prompt_input);
                        }
                    }
                }
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.status_message = Some("Prompt copied to clipboard.".to_string());
                }
                Task::none()
            }

            Message::CopyResponse(id) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get(&id) {
                    if let Some(last_msg) = state.messages.iter().rev().find(|m| m.role == ChatRole::Model) {
                        if !last_msg.content.is_empty() {
                            if let Ok(mut cb) = Clipboard::new() {
                                let _ = cb.set_text(&last_msg.content);
                            }
                        }
                    }
                }
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.status_message = Some("Response copied to clipboard.".to_string());
                }
                Task::none()
            }

            Message::GeminiStreamChunk(id, chunk) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    match chunk {
                        GeminiChunk::Text(piece) => {
                            state.is_loading = false;
                            if let Some(last_msg) = state.messages.last_mut() {
                                if last_msg.role == ChatRole::Model {
                                    last_msg.push_str(&piece);
                                }
                            }
                        }
                        GeminiChunk::Done => {
                            state.is_loading = false;
                        }
                        GeminiChunk::Error(err) => {
                            state.is_loading = false;
                            if let Some(last_msg) = state.messages.last_mut() {
                                if last_msg.role == ChatRole::Model {
                                    last_msg.push_str(&format!("\n\n**Error:** {}", err));
                                }
                            }
                        }
                    }
                }
                Task::none()
            }

            Message::ToggleSettings(id) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.show_settings = !state.show_settings;
                    state.api_key_input = self.config.gemini_api_key.clone();
                }
                Task::none()
            }

            Message::ApiKeyInputChanged(id, val) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.api_key_input = val;
                }
                Task::none()
            }

            Message::SaveSettings(id) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    self.config.gemini_api_key = state.api_key_input.trim().to_string();
                    self.config.save();
                    state.show_settings = false;
                    state.status_message = Some("API key saved.".to_string());
                }
                Task::none()
            }

            Message::OpenApiKeyUrl => {
                let _ = open::that("https://aistudio.google.com/app/apikey");
                Task::none()
            }

            Message::OpenMarkdownUrl(url) => {
                let _ = open::that(&url);
                Task::none()
            }

            Message::SetMode(mode) => {
                self.config.mode = mode;
                self.config.save();
                Task::none()
            }

            Message::NoOp => Task::none(),
        }
    }

    fn start_capture(&self) -> Task<Message> {
        Task::perform(
            async {
                tokio::task::spawn_blocking(crate::capture::capture_current_monitor)
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
            },
            Message::ScreenCaptured,
        )
    }

    fn query_gemini(&mut self, window_id: window::Id, prompt: String) -> Task<Message> {
        let (image_path, api_key, mode, history) = {
            if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&window_id) {
                state.is_loading = true;
                state.status_message = None;

                // Add user message
                state.messages.push(ChatMessageItem::new(
                    ChatRole::User,
                    prompt.clone(),
                ));

                // Prepare placeholder for model streaming response
                state.messages.push(ChatMessageItem::new(
                    ChatRole::Model,
                    String::new(),
                ));

                // Convert history to gemini format (exclude the trailing empty model message)
                let history_msgs: Vec<gemini::ChatMessage> = state
                    .messages[..state.messages.len() - 1]
                    .iter()
                    .map(|m| gemini::ChatMessage {
                        role: match m.role {
                            ChatRole::User => "user".to_string(),
                            ChatRole::Model => "model".to_string(),
                        },
                        content: m.content.clone(),
                    })
                    .collect();

                (
                    state.image_path.clone(),
                    self.config.gemini_api_key.clone(),
                    self.config.mode.clone(),
                    history_msgs,
                )
            } else {
                return Task::none();
            }
        };

        if api_key.trim().is_empty() {
            if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&window_id) {
                state.is_loading = false;
                state.show_settings = true;
                if let Some(last_msg) = state.messages.last_mut() {
                    last_msg.content =
                        "Gemini API key not found. Please enter your free Gemini API key below (get one at aistudio.google.com), or use the Google Lens button for instant search without a key."
                            .to_string();
                    let sanitized = sanitize_markdown_math(&last_msg.content);
                    last_msg.md_items = markdown::parse(&sanitized).collect();
                }
            }
            return Task::none();
        }

        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            gemini::stream_gemini_response(api_key, mode, image_path, history, tx).await;
        });

        Task::run(
            iced::stream::channel(100, move |mut output| async move {
                use iced::futures::SinkExt;
                while let Some(chunk) = rx.recv().await {
                    let _ = output.send(Message::GeminiStreamChunk(window_id, chunk)).await;
                }
            }),
            |msg| msg,
        )
    }

    pub fn view(&self, window_id: window::Id) -> Element<'_, Message> {
        match self.windows.get(&window_id) {
            Some(WindowState::Overlay { screenshot_path, drawing }) => {
                let overlay_canvas = Canvas::new(OverlayCanvas {
                    state: drawing,
                })
                .width(Fill)
                .height(Fill);

                let bg_image = image(image::Handle::from_path(screenshot_path))
                    .width(Fill)
                    .height(Fill);

                let hint = container(
                    row![
                        text("Circle or draw around anything on your screen")
                            .size(15)
                            .color(Color::WHITE),
                        text(" - Esc or Right-click to cancel")
                            .size(13)
                            .color(Color::from_rgb(0.7, 0.7, 0.7)),
                    ]
                    .align_y(alignment::Vertical::Center)
                    .spacing(10),
                )
                .padding([8, 16])
                .style(|_| container::Style {
                    background: Some(Color::from_rgba(0.08, 0.08, 0.1, 0.85).into()),
                    border: iced::border::rounded(20),
                    ..Default::default()
                });

                iced::widget::stack![
                    bg_image,
                    Element::from(overlay_canvas).map(move |m| Message::OverlayAction(window_id, m)),
                    container(hint)
                        .width(Fill)
                        .height(Fill)
                        .align_x(alignment::Horizontal::Center)
                        .align_y(alignment::Vertical::Top)
                        .padding(20)
                ]
                .into()
            }

            Some(WindowState::Assistant(state)) => {
                self.view_assistant(window_id, state)
            }

            None => container(text("Loading...")).into(),
        }
    }

    fn view_assistant<'a>(&'a self, id: window::Id, state: &'a AssistantState) -> Element<'a, Message> {
        // --- Top Bar ---
        let is_fast = self.config.mode == "fast";
        let mode_toggle = row![
            button(text("Smart").size(11))
                .on_press(Message::SetMode("smart".to_string()))
                .style(mode_button_style(!is_fast))
                .padding([3, 10]),
            button(text("Fast").size(11))
                .on_press(Message::SetMode("fast".to_string()))
                .style(mode_button_style(is_fast))
                .padding([3, 10]),
        ]
        .spacing(4);

        let header = row![
            text("Gemini Screen Assistant")
                .size(16)
                .color(Color::from_rgb(0.55, 0.75, 1.0)),
            horizontal_space(),
            mode_toggle,
            button(text("Settings").size(12))
                .on_press(Message::ToggleSettings(id))
                .style(button_style_subtle),
            button(text("Close").size(12))
                .on_press(Message::WindowClosed(id))
                .style(button_style_subtle),
        ]
        .align_y(alignment::Vertical::Center)
        .spacing(8);

        // --- Settings Section (collapsible) ---
        let settings_box = if state.show_settings {
            container(
                column![
                    text("Gemini API Configuration").size(14).color(Color::WHITE),
                    row![
                        text_input("Enter Gemini API Key...", &state.api_key_input)
                            .on_input(move |v| Message::ApiKeyInputChanged(id, v))
                            .secure(true)
                            .padding(8),
                        button("Save").on_press(Message::SaveSettings(id)),
                    ]
                    .spacing(8),
                    row![
                        button(text("Get free API key from Google AI Studio").size(12))
                            .on_press(Message::OpenApiKeyUrl)
                            .style(button_style_link),
                    ],
                ]
                .spacing(8),
            )
            .padding(12)
            .style(settings_card_style)
        } else {
            container(column![])
        };

        // --- Snippet Image Preview ---
        let preview = container(
            image(image::Handle::from_path(&state.image_path))
                .height(Length::Fixed(130.0))
                .content_fit(iced::ContentFit::Contain),
        )
        .width(Fill)
        .align_x(alignment::Horizontal::Center)
        .padding(6)
        .style(preview_container_style);

        // --- Quick Action Pills ---
        let action_chips = row![
            button("Explain").on_press(Message::ActionExplain(id)).style(chip_style),
            button("Copy Text").on_press(Message::ActionCopyText(id)).style(chip_style),
            button("Google Lens").on_press(Message::ActionLens(id)).style(chip_style),
            button("Open in Gemini Web").on_press(Message::ActionContinueInWeb(id)).style(chip_style),
        ]
        .spacing(8)
        .wrap();

        // --- Prompt Input Bar ---
        let input_row = row![
            text_input("Ask follow-up or any question...", &state.prompt_input)
                .on_input(move |val| Message::PromptChanged(id, val))
                .on_submit(Message::SendPrompt(id))
                .padding(10),
            button(text("Ask").size(14))
                .on_press(Message::SendPrompt(id))
                .padding([10, 16])
                .style(send_button_style),
        ]
        .spacing(8)
        .align_y(alignment::Vertical::Center);

        // --- Chat Messages Area ---
        let mut chat_column = Column::new().spacing(10);

        if state.messages.is_empty() {
            chat_column = chat_column.push(
                text("Select a quick action above or type any question below to chat with Gemini about this screenshot.")
                    .size(13)
                    .color(Color::from_rgb(0.5, 0.5, 0.6)),
            );
        } else {
            // Chat header with Copy All and Continue in Web
            chat_column = chat_column.push(
                row![
                    text("Conversation").size(12).color(Color::from_rgb(0.5, 0.5, 0.6)),
                    horizontal_space(),
                    button(text("Open in Gemini Web").size(11))
                        .on_press(Message::ActionContinueInWeb(id))
                        .style(button_style_subtle),
                    button(text("Copy All").size(11))
                        .on_press(Message::CopyAllChat(id))
                        .style(button_style_subtle),
                ]
                .align_y(alignment::Vertical::Center)
                .spacing(8),
            );

            for (idx, msg) in state.messages.iter().enumerate() {
                match msg.role {
                    ChatRole::User => {
                        let user_bubble = container(
                            column![
                                text("You").size(11).color(Color::from_rgb(0.6, 0.8, 1.0)),
                                text(&msg.content).size(14).color(Color::WHITE),
                            ]
                            .spacing(3),
                        )
                        .padding([8, 12])
                        .style(|_| container::Style {
                            background: Some(Color::from_rgba(0.2, 0.35, 0.6, 0.35).into()),
                            border: iced::border::rounded(8),
                            ..Default::default()
                        });
                        chat_column = chat_column.push(user_bubble);
                    }
                    ChatRole::Model => {
                        let is_last = idx == state.messages.len() - 1;
                        let is_empty_and_thinking = msg.content.is_empty() && is_last && state.is_loading;

                        let content_widget: Element<'a, Message> = if is_empty_and_thinking {
                            text("Gemini is thinking...")
                                .size(13)
                                .color(Color::from_rgb(0.6, 0.6, 0.85))
                                .into()
                        } else {
                            markdown::view(
                                &msg.md_items,
                                markdown::Settings::default(),
                                markdown::Style::from_palette(iced::theme::Palette::DARK),
                            )
                            .map(|url| Message::OpenMarkdownUrl(url.to_string()))
                            .into()
                        };

                        let model_bubble = container(
                            column![
                                row![
                                    text("Gemini").size(11).color(Color::from_rgb(0.7, 0.7, 0.85)),
                                    horizontal_space(),
                                    button(text("Copy").size(11))
                                        .on_press(Message::CopyMessage(id, idx))
                                        .style(button_style_subtle),
                                ]
                                .align_y(alignment::Vertical::Center),
                                content_widget,
                            ]
                            .spacing(5),
                        )
                        .padding([10, 12])
                        .style(|_| container::Style {
                            background: Some(Color::from_rgba(0.12, 0.13, 0.18, 0.85).into()),
                            border: iced::border::rounded(8),
                            ..Default::default()
                        });
                        chat_column = chat_column.push(model_bubble);
                    }
                }
            }
        }

        let response_box = container(
            scrollable(chat_column)
                .height(Length::Fill),
        )
        .width(Fill)
        .height(Fill)
        .padding(10)
        .style(response_container_style);

        // --- Status Banner (if any) ---
        let status_bar: Element<'_, Message> = if let Some(msg) = &state.status_message {
            container(text(msg).size(12).color(Color::from_rgb(0.4, 0.9, 0.5)))
                .padding([4, 8])
                .into()
        } else {
            container(column![]).into()
        };

        // --- Extracted Text Panel (Interactive OCR Selection) ---
        let extracted_box: Element<'a, Message> = if let Some(content) = &state.extracted_text {
            let has_sel = content.selection().is_some();
            let copy_sel_label = if has_sel {
                "Copy Selection"
            } else {
                "Copy All"
            };

            container(
                column![
                    row![
                        text("Extracted Text (Selectable)").size(13).color(Color::from_rgb(0.7, 0.85, 1.0)),
                        horizontal_space(),
                        button(text(copy_sel_label).size(11))
                            .on_press(Message::CopyExtractedSelection(id))
                            .style(chip_style)
                            .padding([3, 8]),
                        button(text("Copy All").size(11))
                            .on_press(Message::CopyExtractedAll(id))
                            .style(button_style_subtle)
                            .padding([3, 8]),
                        button(text("Insert in Prompt").size(11))
                            .on_press(Message::InsertExtractedToPrompt(id))
                            .style(button_style_subtle)
                            .padding([3, 8]),
                        button(text("Close").size(11))
                            .on_press(Message::CloseExtractedText(id))
                            .style(button_style_subtle)
                            .padding([3, 8]),
                    ]
                    .align_y(alignment::Vertical::Center)
                    .spacing(6),
                    text("Click and drag mouse to select specific text to copy:")
                        .size(11)
                        .color(Color::from_rgb(0.6, 0.6, 0.7)),
                    container(
                        text_editor(content)
                            .on_action(move |action| Message::EditorAction(id, action))
                            .padding(8)
                    )
                    .height(Length::Fixed(120.0))
                    .style(editor_container_style),
                ]
                .spacing(6),
            )
            .padding(10)
            .style(extracted_card_style)
            .into()
        } else {
            container(column![]).into()
        };

        // --- Main Layout ---
        container(
            column![
                header,
                if state.show_settings {
                    Element::from(settings_box)
                } else {
                    horizontal_space().into()
                },
                preview,
                action_chips,
                if state.extracted_text.is_some() {
                    extracted_box
                } else {
                    horizontal_space().into()
                },
                input_row,
                response_box,
                status_bar,
            ]
            .spacing(10),
        )
        .padding(16)
        .width(Fill)
        .height(Fill)
        .into()
    }
}

/// Helper function to load 32x32 window icon
fn load_window_icon() -> Result<window::Icon, Box<dyn std::error::Error>> {
    let img_bytes = include_bytes!("../assets/gemini-icon-32.png");
    let img = ::image::load_from_memory(img_bytes)?.to_rgba8();
    let (width, height) = img.dimensions();
    Ok(window::icon::from_rgba(img.into_raw(), width, height)?)
}

/// Helper function to crop the selection from screenshot
fn crop_image(screenshot_path: &str, rect: SelectionRect) -> Result<String, String> {
    let mut img = ::image::open(screenshot_path).map_err(|e| e.to_string())?;
    let img_w = img.width();
    let img_h = img.height();

    let (scale_x, scale_y) = if rect.canvas_width > 10.0 && rect.canvas_height > 10.0 {
        (img_w as f32 / rect.canvas_width, img_h as f32 / rect.canvas_height)
    } else {
        (1.0, 1.0)
    };

    let crop_x = ((rect.x as f32 * scale_x).round() as u32).min(img_w.saturating_sub(1));
    let crop_y = ((rect.y as f32 * scale_y).round() as u32).min(img_h.saturating_sub(1));
    let crop_w = ((rect.width as f32 * scale_x).round() as u32).max(1).min(img_w - crop_x);
    let crop_h = ((rect.height as f32 * scale_y).round() as u32).max(1).min(img_h - crop_y);

    let cropped = ::image::imageops::crop(
        &mut img,
        crop_x,
        crop_y,
        crop_w,
        crop_h,
    )
    .to_image();

    let tmp = tempfile::NamedTempFile::new().map_err(|e| e.to_string())?;
    let path = format!("{}_crop.png", tmp.path().display());
    cropped.save(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

// --- Visual Styles ---

fn chip_style(_theme: &Theme, status: button::Status) -> button::Style {
    let base = Color::from_rgb(0.18, 0.20, 0.25);
    let border_color = Color::from_rgb(0.35, 0.50, 0.85);

    match status {
        button::Status::Hovered => button::Style {
            background: Some(Color::from_rgb(0.25, 0.30, 0.40).into()),
            text_color: Color::WHITE,
            border: iced::border::rounded(14).color(border_color).width(1.0),
            ..Default::default()
        },
        _ => button::Style {
            background: Some(base.into()),
            text_color: Color::from_rgb(0.85, 0.88, 0.95),
            border: iced::border::rounded(14).color(Color::from_rgb(0.28, 0.30, 0.38)).width(1.0),
            ..Default::default()
        },
    }
}

fn send_button_style(_theme: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered => button::Style {
            background: Some(Color::from_rgb(0.35, 0.60, 1.0).into()),
            text_color: Color::WHITE,
            border: iced::border::rounded(8),
            ..Default::default()
        },
        _ => button::Style {
            background: Some(Color::from_rgb(0.25, 0.48, 0.95).into()),
            text_color: Color::WHITE,
            border: iced::border::rounded(8),
            ..Default::default()
        },
    }
}

fn button_style_subtle(_theme: &Theme, status: button::Status) -> button::Style {
    match status {
        button::Status::Hovered => button::Style {
            background: Some(Color::from_rgba(1.0, 1.0, 1.0, 0.1).into()),
            text_color: Color::WHITE,
            border: iced::border::rounded(6),
            ..Default::default()
        },
        _ => button::Style {
            background: None,
            text_color: Color::from_rgb(0.7, 0.7, 0.8),
            ..Default::default()
        },
    }
}

fn button_style_link(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: Color::from_rgb(0.4, 0.7, 1.0),
        ..Default::default()
    }
}

fn preview_container_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb(0.12, 0.13, 0.16).into()),
        border: iced::border::rounded(8).color(Color::from_rgb(0.22, 0.24, 0.30)).width(1.0),
        ..Default::default()
    }
}

fn response_container_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb(0.12, 0.13, 0.17).into()),
        border: iced::border::rounded(8).color(Color::from_rgb(0.22, 0.25, 0.32)).width(1.0),
        ..Default::default()
    }
}

fn settings_card_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb(0.14, 0.16, 0.21).into()),
        border: iced::border::rounded(8).color(Color::from_rgb(0.35, 0.50, 0.85)).width(1.0),
        ..Default::default()
    }
}

fn editor_container_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb(0.10, 0.11, 0.15).into()),
        border: iced::border::rounded(6).color(Color::from_rgb(0.25, 0.30, 0.45)).width(1.0),
        ..Default::default()
    }
}

fn extracted_card_style(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Color::from_rgb(0.14, 0.16, 0.22).into()),
        border: iced::border::rounded(8).color(Color::from_rgb(0.30, 0.45, 0.70)).width(1.0),
        ..Default::default()
    }
}

fn mode_button_style(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        if active {
            button::Style {
                background: Some(Color::from_rgb(0.22, 0.48, 0.95).into()),
                text_color: Color::WHITE,
                border: iced::border::rounded(12),
                ..Default::default()
            }
        } else {
            let bg = match status {
                button::Status::Hovered => Color::from_rgba(1.0, 1.0, 1.0, 0.12),
                _ => Color::from_rgba(1.0, 1.0, 1.0, 0.05),
            };
            button::Style {
                background: Some(bg.into()),
                text_color: Color::from_rgb(0.65, 0.68, 0.75),
                border: iced::border::rounded(12),
                ..Default::default()
            }
        }
    }
}

fn format_subscript(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '0' => out.push('₀'),
            '1' => out.push('₁'),
            '2' => out.push('₂'),
            '3' => out.push('₃'),
            '4' => out.push('₄'),
            '5' => out.push('₅'),
            '6' => out.push('₆'),
            '7' => out.push('₇'),
            '8' => out.push('₈'),
            '9' => out.push('₉'),
            '+' => out.push('₊'),
            '-' => out.push('₋'),
            '=' => out.push('₌'),
            '(' => out.push('₍'),
            ')' => out.push('₎'),
            'a' => out.push('ₐ'),
            'e' => out.push('ₑ'),
            'h' => out.push('ₕ'),
            'i' => out.push('ᵢ'),
            'j' => out.push('ⱼ'),
            'k' => out.push('ₖ'),
            'l' => out.push('ₗ'),
            'm' => out.push('ₘ'),
            'n' => out.push('ₙ'),
            'o' => out.push('ₒ'),
            'p' => out.push('ₚ'),
            'r' => out.push('ᵣ'),
            's' => out.push('ₛ'),
            't' => out.push('ₜ'),
            'u' => out.push('ᵤ'),
            'v' => out.push('ᵥ'),
            'x' => out.push('ₓ'),
            _ => return format!("_{}", s),
        }
    }
    out
}

fn format_superscript(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '0' => out.push('⁰'),
            '1' => out.push('¹'),
            '2' => out.push('²'),
            '3' => out.push('³'),
            '4' => out.push('⁴'),
            '5' => out.push('⁵'),
            '6' => out.push('⁶'),
            '7' => out.push('⁷'),
            '8' => out.push('⁸'),
            '9' => out.push('⁹'),
            '+' => out.push('⁺'),
            '-' => out.push('⁻'),
            '=' => out.push('⁼'),
            '(' => out.push('⁽'),
            ')' => out.push('⁾'),
            'n' => out.push('ⁿ'),
            'i' => out.push('ⁱ'),
            'T' => out.push('ᵀ'),
            '*' => out.push('*'),
            _ => return format!("^{}", s),
        }
    }
    out
}

fn sanitize_markdown_math(input: &str) -> String {
    let parts: Vec<&str> = input.split("$$").collect();
    if parts.len() > 1 {
        let mut result = String::new();
        for (i, part) in parts.iter().enumerate() {
            if i % 2 == 1 {
                if i == parts.len() - 1 {
                    // Streaming in progress: unclosed $$
                    result.push_str("$$");
                    result.push_str(part);
                } else {
                    let clean = clean_latex_formula(part);
                    result.push_str(&format!("\n```\n{}\n```\n", clean.trim()));
                }
            } else {
                result.push_str(&sanitize_inline_math(part));
            }
        }
        result
    } else {
        sanitize_inline_math(input)
    }
}

fn sanitize_inline_math(input: &str) -> String {
    let mut result = String::new();
    let mut in_dollar = false;
    let mut buf = String::new();

    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && (i == 0 || chars[i - 1] != '\\') {
            if in_dollar {
                result.push_str(&clean_latex_formula(&buf));
                buf.clear();
                in_dollar = false;
            } else {
                in_dollar = true;
            }
            i += 1;
            continue;
        }

        if in_dollar {
            buf.push(chars[i]);
        } else {
            result.push(chars[i]);
        }
        i += 1;
    }

    if in_dollar {
        result.push('$');
        result.push_str(&buf);
    }

    result
}

fn clean_latex_formula(input: &str) -> String {
    let mut s = input.to_string();

    for prefix in &[
        "\\mathbf{", "\\mathit{", "\\mathrm{", "\\textrm{", "\\textbf{", "\\textit{", "\\text{",
    ] {
        while let Some(start) = s.find(prefix) {
            let len = prefix.len();
            if let Some(close) = s[start + len..].find('}') {
                let inner = &s[start + len..start + len + close];
                s = format!("{}{}{}", &s[..start], inner, &s[start + len + close + 1..]);
            } else {
                break;
            }
        }
    }

    while let Some(start) = s.find("\\frac{") {
        if let Some(first_close) = s[start + 6..].find('}') {
            let num = &s[start + 6..start + 6 + first_close];
            let rest = &s[start + 6 + first_close + 1..];
            if rest.starts_with('{') {
                if let Some(second_close) = rest[1..].find('}') {
                    let den = &rest[1..1 + second_close];
                    s = format!("{}{}/{}{}", &s[..start], num, den, &rest[1 + second_close + 1..]);
                    continue;
                }
            }
        }
        break;
    }

    while let Some(start) = s.find("\\sqrt{") {
        if let Some(close) = s[start + 6..].find('}') {
            let inner = &s[start + 6..start + 6 + close];
            s = format!("{}√({}){}", &s[..start], inner, &s[start + 6 + close + 1..]);
        } else {
            break;
        }
    }

    let replacements = [
        ("\\forall", "∀"),
        ("\\exists", "∃"),
        ("\\nexists", "∄"),
        ("\\in", "∈"),
        ("\\notin", "∉"),
        ("\\ni", "∋"),
        ("\\subseteq", "⊆"),
        ("\\subset", "⊂"),
        ("\\supseteq", "⊇"),
        ("\\supset", "⊃"),
        ("\\emptyset", "∅"),
        ("\\cap", "∩"),
        ("\\cup", "∪"),
        ("\\leq", "≤"),
        ("\\le", "≤"),
        ("\\geq", "≥"),
        ("\\ge", "≥"),
        ("\\neq", "≠"),
        ("\\ne", "≠"),
        ("\\approx", "≈"),
        ("\\times", "×"),
        ("\\cdot", "·"),
        ("\\div", "÷"),
        ("\\pm", "±"),
        ("\\infty", "∞"),
        ("\\partial", "∂"),
        ("\\nabla", "∇"),
        ("\\sum", "∑"),
        ("\\prod", "∏"),
        ("\\int", "∫"),
        ("\\sqrt", "√"),
        ("\\rightarrow", "→"),
        ("\\to", "→"),
        ("\\leftarrow", "←"),
        ("\\Rightarrow", "⇒"),
        ("\\Leftarrow", "⇐"),
        ("\\Leftrightarrow", "⇔"),
        ("\\iff", "⇔"),
        ("\\alpha", "α"),
        ("\\beta", "β"),
        ("\\gamma", "γ"),
        ("\\Gamma", "Γ"),
        ("\\delta", "δ"),
        ("\\Delta", "Δ"),
        ("\\epsilon", "ε"),
        ("\\varepsilon", "ε"),
        ("\\zeta", "ζ"),
        ("\\eta", "η"),
        ("\\theta", "θ"),
        ("\\Theta", "Θ"),
        ("\\iota", "ι"),
        ("\\kappa", "κ"),
        ("\\lambda", "λ"),
        ("\\Lambda", "Λ"),
        ("\\mu", "μ"),
        ("\\nu", "ν"),
        ("\\xi", "ξ"),
        ("\\pi", "π"),
        ("\\Pi", "Π"),
        ("\\rho", "ρ"),
        ("\\sigma", "σ"),
        ("\\Sigma", "Σ"),
        ("\\tau", "τ"),
        ("\\phi", "φ"),
        ("\\Phi", "Φ"),
        ("\\chi", "χ"),
        ("\\psi", "ψ"),
        ("\\Psi", "Ψ"),
        ("\\omega", "ω"),
        ("\\Omega", "Ω"),
        ("\\mathbb{R}", "ℝ"),
        ("\\mathbb{N}", "ℕ"),
        ("\\mathbb{Z}", "ℤ"),
        ("\\mathbb{C}", "ℂ"),
        ("\\mathbb{Q}", "ℚ"),
        ("\\quad", "  "),
        ("\\qquad", "    "),
        ("\\,", " "),
        ("\\;", " "),
        ("\\:", " "),
    ];

    for (from, to) in replacements {
        s = s.replace(from, to);
    }

    while let Some(start) = s.find("_{") {
        if let Some(close) = s[start + 2..].find('}') {
            let inner = &s[start + 2..start + 2 + close];
            let sub = format_subscript(inner);
            s = format!("{}{}{}", &s[..start], sub, &s[start + 2 + close + 1..]);
        } else {
            break;
        }
    }

    while let Some(start) = s.find("^{") {
        if let Some(close) = s[start + 2..].find('}') {
            let inner = &s[start + 2..start + 2 + close];
            let sup = format_superscript(inner);
            s = format!("{}{}{}", &s[..start], sup, &s[start + 2 + close + 1..]);
        } else {
            break;
        }
    }

    let single_sub = [
        ("_0", "₀"), ("_1", "₁"), ("_2", "₂"), ("_3", "₃"), ("_4", "₄"),
        ("_5", "₅"), ("_6", "₆"), ("_7", "₇"), ("_8", "₈"), ("_9", "₉"),
        ("_i", "ᵢ"), ("_j", "ⱼ"), ("_k", "ₖ"), ("_n", "ₙ"), ("_m", "ₘ"),
        ("_t", "ₜ"), ("_x", "ₓ"), ("_y", "ᵧ"),
    ];
    for (from, to) in single_sub {
        s = s.replace(from, to);
    }

    let single_sup = [
        ("^0", "⁰"), ("^1", "¹"), ("^2", "²"), ("^3", "³"), ("^4", "⁴"),
        ("^5", "⁵"), ("^6", "⁶"), ("^7", "⁷"), ("^8", "⁸"), ("^9", "⁹"),
        ("^n", "ⁿ"), ("^i", "ⁱ"), ("^T", "ᵀ"), ("^*", "*"),
    ];
    for (from, to) in single_sup {
        s = s.replace(from, to);
    }

    s.trim().to_string()
}
