// Core Application - iced daemon managing multi-window overlay and assistant card

use crate::config::Config;
use crate::gemini::{self, GeminiChunk};
use crate::lens;
use crate::overlay::{OverlayCanvas, OverlayMessage, OverlayState, SelectionRect};
use crate::tray::TrayCommand;

use arboard::Clipboard;
use iced::widget::canvas::Canvas;
use iced::widget::{
    button, checkbox, column, container, horizontal_space, image, markdown, row, scrollable, text,
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
    pub system_instruction_content: text_editor::Content<iced::Renderer>,
    pub debug_logging: bool,
    pub single_window_mode: bool,
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
    SystemInstructionAction(window::Id, text_editor::Action),
    ResetSystemInstruction(window::Id),
    SaveSettings(window::Id),
    OpenApiKeyUrl,
    ToggleDebugLogging(window::Id, bool),
    OpenLogFolder,
    ViewLogFile,
    ClearLogs(window::Id),
    SetMode(String),
    ToggleSingleWindowMode(window::Id, bool),
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
                            state.system_instruction_content = text_editor::Content::with_text(&self.config.system_instruction);
                            state.debug_logging = self.config.debug_logging;
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
                    system_instruction_content: text_editor::Content::with_text(&self.config.system_instruction),
                    debug_logging: self.config.debug_logging,
                    single_window_mode: self.config.single_window_mode,
                    status_message: None,
                    extracted_text: None,
                };

                self.windows
                    .insert(assistant_id, WindowState::Assistant(initial_state));

                // Close the previous assistant window if single-window mode is enabled
                let mut tasks: Vec<Task<Message>> = vec![
                    window::close(overlay_id),
                    open_task.map(move |id| Message::WindowOpened(id, WindowKindTag::Assistant)),
                ];
                if self.config.single_window_mode {
                    if let Some(old_id) = self.active_assistant_id {
                        self.windows.remove(&old_id);
                        tasks.push(window::close(old_id));
                    }
                }
                self.active_assistant_id = Some(assistant_id);

                Task::batch(tasks)
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
                    state.system_instruction_content = text_editor::Content::with_text(&self.config.system_instruction);
                    state.debug_logging = self.config.debug_logging;
                    state.single_window_mode = self.config.single_window_mode;
                }
                Task::none()
            }

            Message::ApiKeyInputChanged(id, val) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.api_key_input = val;
                }
                Task::none()
            }

            Message::SystemInstructionAction(id, action) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.system_instruction_content.perform(action);
                }
                Task::none()
            }

            Message::ResetSystemInstruction(id) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    let default_inst = crate::config::default_system_instruction();
                    state.system_instruction_content = text_editor::Content::with_text(&default_inst);
                    state.status_message = Some("System instructions reset to default. Click Save Settings to persist.".to_string());
                }
                Task::none()
            }

            Message::SaveSettings(id) => {
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    self.config.gemini_api_key = state.api_key_input.trim().to_string();
                    self.config.system_instruction = state.system_instruction_content.text();
                    self.config.save();
                    state.show_settings = false;
                    state.status_message = Some("Settings saved successfully.".to_string());
                }
                Task::none()
            }

            Message::ToggleDebugLogging(id, val) => {
                self.config.debug_logging = val;
                crate::logger::set_enabled(val);
                self.config.save();
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.debug_logging = val;
                    state.status_message = Some(if val {
                        "Debug logging enabled.".to_string()
                    } else {
                        "Debug logging disabled.".to_string()
                    });
                }
                Task::none()
            }

            Message::OpenLogFolder => {
                let _ = open::that(crate::logger::log_dir());
                Task::none()
            }

            Message::ViewLogFile => {
                let path = crate::logger::log_file_path();
                if path.exists() {
                    let _ = open::that(path);
                } else {
                    let _ = open::that(crate::logger::log_dir());
                }
                Task::none()
            }

            Message::ClearLogs(id) => {
                crate::logger::clear_logs();
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.status_message = Some("Logs cleared.".to_string());
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

            Message::ToggleSingleWindowMode(id, val) => {
                self.config.single_window_mode = val;
                self.config.save();
                if let Some(WindowState::Assistant(state)) = self.windows.get_mut(&id) {
                    state.single_window_mode = val;
                    state.status_message = Some(if val {
                        "Single-window mode enabled. Previous card closes on new capture.".to_string()
                    } else {
                        "Single-window mode disabled. Multiple cards allowed.".to_string()
                    });
                }
                Task::none()
            }

            Message::NoOp => Task::none(),
        }
    }

    fn start_capture(&self) -> Task<Message> {
        // Guard: if an overlay is already open do not stack a second one
        let overlay_already_open = self
            .windows
            .values()
            .any(|s| matches!(s, WindowState::Overlay { .. }));
        if overlay_already_open {
            return Task::none();
        }

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
        let (image_path, api_key, mode, system_instruction, history) = {
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
                    self.config.system_instruction.clone(),
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
            gemini::stream_gemini_response(api_key, mode, system_instruction, image_path, history, tx).await;
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
                    container(horizontal_space())
                        .height(1)
                        .width(Fill)
                        .style(|_theme| container::Style {
                            background: Some(iced::Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
                            ..Default::default()
                        }),
                    text("Diagnostics & Logging").size(14).color(Color::WHITE),
                    row![
                        checkbox("Enable Debug Logging", state.debug_logging)
                            .on_toggle(move |val| Message::ToggleDebugLogging(id, val)),
                    ]
                    .spacing(8),
                    text("Logs requests, streaming chunks, OCR, and errors with circular size limits (max 8 MB) to ~/.config/circle-to-gemini/logs/. Disabled by default.")
                        .size(11)
                        .color(Color::from_rgb(0.65, 0.65, 0.7)),
                    row![
                        button(text("Open Log Folder").size(11))
                            .on_press(Message::OpenLogFolder)
                            .style(button_style_subtle),
                        button(text("View Log File").size(11))
                            .on_press(Message::ViewLogFile)
                            .style(button_style_subtle),
                        button(text("Clear Logs").size(11))
                            .on_press(Message::ClearLogs(id))
                            .style(button_style_subtle),
                    ]
                    .spacing(8),
                    container(horizontal_space())
                        .height(1)
                        .width(Fill)
                        .style(|_theme| container::Style {
                            background: Some(iced::Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
                            ..Default::default()
                        }),
                    text("Window Behavior").size(14).color(Color::WHITE),
                    row![
                        checkbox(
                            "Close previous card when opening a new one",
                            state.single_window_mode
                        )
                        .on_toggle(move |val| Message::ToggleSingleWindowMode(id, val)),
                    ]
                    .spacing(8),
                    text("When enabled, triggering a new capture automatically closes any open Assistant Card. When disabled, multiple cards can remain open side by side.")
                        .size(11)
                        .color(Color::from_rgb(0.65, 0.65, 0.7)),
                    container(horizontal_space())
                        .height(1)
                        .width(Fill)
                        .style(|_theme| container::Style {
                            background: Some(iced::Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.08))),
                            ..Default::default()
                        }),
                    row![
                        text("System Instructions").size(14).color(Color::WHITE),
                        horizontal_space(),
                        button(text("Reset to Default").size(11))
                            .on_press(Message::ResetSystemInstruction(id))
                            .style(button_style_subtle),
                    ]
                    .align_y(alignment::Vertical::Center),
                    text("Prompt instructions sent to Gemini with every screenshot analysis.")
                        .size(11)
                        .color(Color::from_rgb(0.65, 0.65, 0.7)),
                    container(
                        text_editor(&state.system_instruction_content)
                            .on_action(move |action| Message::SystemInstructionAction(id, action))
                            .padding(8)
                    )
                    .height(Length::Fixed(110.0))
                    .style(editor_container_style),
                    row![
                        button(text("Save Settings").size(12))
                            .on_press(Message::SaveSettings(id)),
                    ],
                ]
                .spacing(10),
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
            _ => {
                if s.chars().count() > 1 {
                    return format!("[{}]", s);
                } else {
                    return format!("_{}", s);
                }
            }
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
            'a' => out.push('ᵃ'),
            'b' => out.push('ᵇ'),
            'c' => out.push('ᶜ'),
            'd' => out.push('ᵈ'),
            'e' => out.push('ᵉ'),
            'f' => out.push('ᶠ'),
            'g' => out.push('ᵍ'),
            'h' => out.push('ʰ'),
            'i' => out.push('ⁱ'),
            'j' => out.push('ʲ'),
            'k' => out.push('ᵏ'),
            'l' => out.push('ˡ'),
            'm' => out.push('ᵐ'),
            'n' => out.push('ⁿ'),
            'o' => out.push('ᵒ'),
            'p' => out.push('ᵖ'),
            'r' => out.push('ʳ'),
            's' => out.push('ˢ'),
            't' => out.push('ᵗ'),
            'u' => out.push('ᵘ'),
            'v' => out.push('ᵛ'),
            'w' => out.push('ʷ'),
            'x' => out.push('ˣ'),
            'y' => out.push('ʸ'),
            'z' => out.push('ᶻ'),
            'T' => out.push('ᵀ'),
            '*' => out.push('*'),
            _ => return format!("^{}", s),
        }
    }
    out
}

const MATRIX_ENVS: &[&str] = &[
    "pmatrix", "bmatrix", "Bmatrix", "vmatrix", "Vmatrix", "matrix", "smallmatrix", "array",
    "pmatrix*", "bmatrix*", "Bmatrix*", "vmatrix*", "Vmatrix*", "matrix*",
    "cases", "cases*", "dcases", "dcases*", "rcases", "rcases*",
    "aligned", "aligned*", "gathered", "gathered*", "split",
];

fn str_char_count(s: &str) -> usize {
    s.chars().count()
}

fn has_matrix_env(s: &str) -> bool {
    MATRIX_ENVS.iter().any(|env| s.contains(&format!("\\begin{{{}}}", env)))
}

fn is_table_separator(line: &str) -> bool {
    let s = line.trim();
    if !s.contains('-') {
        return false;
    }
    let trimmed = s.trim_matches('|');
    let parts: Vec<&str> = trimmed.split('|').map(|p| p.trim()).collect();
    if parts.is_empty() {
        return false;
    }
    for p in parts {
        if p.is_empty() || !p.chars().all(|c| c == '-' || c == ':' || c == ' ') || !p.contains('-') {
            return false;
        }
    }
    true
}

fn convert_table_to_box(lines: &[&str]) -> String {
    let mut rows: Vec<Vec<String>> = Vec::new();
    for (idx, l) in lines.iter().enumerate() {
        if idx == 1 && is_table_separator(l) {
            continue;
        }
        let trimmed = l.trim().trim_matches('|');
        let cells: Vec<String> = trimmed
            .split('|')
            .map(|c| clean_latex_formula(c.trim()))
            .collect();
        rows.push(cells);
    }
    if rows.is_empty() {
        return String::new();
    }
    let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(1);
    for r in &mut rows {
        while r.len() < num_cols {
            r.push(String::new());
        }
    }

    let mut col_widths = vec![0usize; num_cols];
    for r in &rows {
        for (c, cell) in r.iter().enumerate() {
            col_widths[c] = col_widths[c].max(str_char_count(cell));
        }
    }
    for w in &mut col_widths {
        *w = (*w).max(1);
    }

    let top = format!(
        "┌{}┐",
        col_widths
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┬")
    );
    let sep = format!(
        "├{}┤",
        col_widths
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┼")
    );
    let bot = format!(
        "└{}┘",
        col_widths
            .iter()
            .map(|w| "─".repeat(w + 2))
            .collect::<Vec<_>>()
            .join("┴")
    );

    let mut out = vec![top];
    for (idx, r) in rows.iter().enumerate() {
        let mut line = String::from("│");
        for (c, cell) in r.iter().enumerate() {
            let cell_len = str_char_count(cell);
            let pad = col_widths[c].saturating_sub(cell_len);
            line.push_str(&format!(" {}{} │", cell, " ".repeat(pad)));
        }
        out.push(line);
        if idx == 0 {
            out.push(sep.clone());
        }
    }
    out.push(bot);
    out.join("\n")
}

fn convert_markdown_tables(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let mut result: Vec<String> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.contains('|') && i + 1 < lines.len() && is_table_separator(lines[i + 1]) {
            let mut table_lines = vec![line, lines[i + 1]];
            let mut j = i + 2;
            while j < lines.len() && lines[j].contains('|') && !lines[j].trim().is_empty() {
                table_lines.push(lines[j]);
                j += 1;
            }
            let box_table = convert_table_to_box(&table_lines);
            if !box_table.is_empty() {
                result.push(format!("```\n{}\n```", box_table));
                i = j;
                continue;
            }
        }
        result.push(line.to_string());
        i += 1;
    }
    result.join("\n")
}

fn find_matching_end_env(input: &str, env_name: &str) -> Option<usize> {
    let begin_tag = format!("\\begin{{{}}}", env_name);
    let end_tag = format!("\\end{{{}}}", env_name);
    let mut depth = 1usize;
    let mut idx = 0;

    while idx < input.len() {
        let rem = &input[idx..];
        let next_b = rem.find(&begin_tag);
        let next_e = rem.find(&end_tag);

        match (next_b, next_e) {
            (Some(b), Some(e)) if b < e => {
                depth += 1;
                idx += b + begin_tag.len();
            }
            (_, Some(e)) => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx + e);
                }
                idx += e + end_tag.len();
            }
            _ => break,
        }
    }
    None
}

fn split_latex_rows(body: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut i = 0;
    let chars: Vec<char> = body.chars().collect();

    while i < chars.len() {
        let rem: String = chars[i..].iter().collect();
        if rem.starts_with("\\begin{") {
            depth += 1;
            current.push_str("\\begin{");
            i += 7;
            continue;
        }
        if rem.starts_with("\\end{") {
            if depth > 0 {
                depth -= 1;
            }
            current.push_str("\\end{");
            i += 5;
            continue;
        }
        if depth == 0 && rem.starts_with("\\\\") {
            let row = current.trim().to_string();
            if !row.is_empty() {
                rows.push(row);
            }
            current.clear();
            i += 2;
            continue;
        }
        current.push(chars[i]);
        i += 1;
    }
    let row = current.trim().to_string();
    if !row.is_empty() {
        rows.push(row);
    }
    rows
}

fn split_latex_cells(row: &str) -> Vec<String> {
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let chars: Vec<char> = row.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let rem: String = chars[i..].iter().collect();
        if rem.starts_with("\\begin{") {
            depth += 1;
            current.push_str("\\begin{");
            i += 7;
            continue;
        }
        if rem.starts_with("\\end{") {
            if depth > 0 {
                depth -= 1;
            }
            current.push_str("\\end{");
            i += 5;
            continue;
        }
        if depth == 0 && chars[i] == '&' {
            cells.push(current.trim().to_string());
            current.clear();
            i += 1;
            continue;
        }
        current.push(chars[i]);
        i += 1;
    }
    cells.push(current.trim().to_string());
    cells
}

fn parse_matrix_body(env_type: &str, content: &str) -> Vec<String> {
    let mut body = content.trim();

    // Strip optional array specifiers: {ccc} or [t] or multiple {..}
    while body.starts_with('{') || body.starts_with('[') {
        if body.starts_with('{') {
            if let Some(close) = body.find('}') {
                body = body[close + 1..].trim();
            } else {
                break;
            }
        } else if body.starts_with('[') {
            if let Some(close) = body.find(']') {
                body = body[close + 1..].trim();
            } else {
                break;
            }
        }
    }

    let raw_rows = split_latex_rows(body);
    let mut parsed_rows: Vec<Vec<Vec<String>>> = Vec::new();

    for raw in raw_rows {
        let row_clean = raw.replace("\\hline", "");
        let r = row_clean.trim();
        if r.is_empty() {
            continue;
        }

        let raw_cells = split_latex_cells(r);
        let mut row_cells: Vec<Vec<String>> = Vec::new();
        for cell in raw_cells {
            let trimmed = cell.trim();
            if has_matrix_env(trimmed) {
                let formatted = format_equation_with_matrices(trimmed);
                let cell_lines: Vec<String> = formatted.lines().map(|l| l.to_string()).collect();
                row_cells.push(if cell_lines.is_empty() { vec![String::new()] } else { cell_lines });
            } else {
                let cleaned = clean_latex_formula(trimmed);
                row_cells.push(vec![cleaned]);
            }
        }
        parsed_rows.push(row_cells);
    }

    if parsed_rows.is_empty() {
        return vec!["[]".to_string()];
    }

    let num_cols = parsed_rows.iter().map(|r| r.len()).max().unwrap_or(1);
    for r in &mut parsed_rows {
        while r.len() < num_cols {
            r.push(vec![String::new()]);
        }
    }

    // Determine column widths
    let mut col_widths = vec![0usize; num_cols];
    for r in &parsed_rows {
        for (j, cell_lines) in r.iter().enumerate() {
            let max_w = cell_lines.iter().map(|l| str_char_count(l)).max().unwrap_or(0);
            col_widths[j] = col_widths[j].max(max_w);
        }
    }

    let mut result_lines = Vec::new();

    for row in &parsed_rows {
        let row_h = row.iter().map(|c| c.len()).max().unwrap_or(1);
        let mut aligned_row_lines = vec![String::new(); row_h];

        for (j, cell_lines) in row.iter().enumerate() {
            let w = col_widths[j];
            let cell_h = cell_lines.len();
            let top_pad = (row_h - cell_h) / 2;
            let bot_pad = row_h - cell_h - top_pad;

            let is_left_aligned = env_type.starts_with("case")
                || env_type.starts_with("rcase")
                || env_type.starts_with("dcase")
                || env_type.starts_with("align")
                || env_type.starts_with("gather")
                || env_type == "split";

            let mut padded_cell = Vec::new();
            for _ in 0..top_pad {
                padded_cell.push(" ".repeat(w));
            }
            for line in cell_lines {
                let len = str_char_count(line);
                let total_pad = w.saturating_sub(len);
                let (left_pad, right_pad) = if is_left_aligned {
                    (0, total_pad)
                } else {
                    let lp = total_pad / 2;
                    (lp, total_pad - lp)
                };
                padded_cell.push(format!("{}{}{}", " ".repeat(left_pad), line, " ".repeat(right_pad)));
            }
            for _ in 0..bot_pad {
                padded_cell.push(" ".repeat(w));
            }

            for (line_idx, line_str) in padded_cell.iter().enumerate() {
                if j > 0 {
                    aligned_row_lines[line_idx].push_str("   ");
                }
                aligned_row_lines[line_idx].push_str(line_str);
            }
        }

        result_lines.extend(aligned_row_lines);
    }

    let num_lines = result_lines.len();
    let mut final_lines = Vec::new();

    for (i, inner) in result_lines.iter().enumerate() {
        let (left, right) = match env_type {
            "pmatrix" | "pmatrix*" => {
                if num_lines == 1 {
                    ("(", ")")
                } else if num_lines == 2 {
                    if i == 0 { ("⎛ ", " ⎞") } else { ("⎝ ", " ⎠") }
                } else if i == 0 {
                    ("⎛ ", " ⎞")
                } else if i == num_lines - 1 {
                    ("⎝ ", " ⎠")
                } else {
                    ("⎜ ", " ⎟")
                }
            }
            "vmatrix" | "vmatrix*" => ("│ ", " │"),
            "Vmatrix" | "Vmatrix*" => ("║ ", " ║"),
            "Bmatrix" | "Bmatrix*" => {
                if num_lines == 1 {
                    ("{", "}")
                } else if num_lines == 2 {
                    if i == 0 { ("⎧ ", " ⎫") } else { ("⎩ ", " ⎭") }
                } else if i == 0 {
                    ("⎧ ", " ⎫")
                } else if i == num_lines - 1 {
                    ("⎩ ", " ⎭")
                } else if i == num_lines / 2 {
                    ("⎨ ", " ⎬")
                } else {
                    ("⎪ ", " ⎪")
                }
            }
            "cases" | "cases*" | "dcases" | "dcases*" => {
                if num_lines == 1 {
                    ("{ ", "")
                } else if num_lines == 2 {
                    if i == 0 { ("⎧ ", "") } else { ("⎩ ", "") }
                } else if i == 0 {
                    ("⎧ ", "")
                } else if i == num_lines - 1 {
                    ("⎩ ", "")
                } else if i == num_lines / 2 {
                    ("⎨ ", "")
                } else {
                    ("⎪ ", "")
                }
            }
            "rcases" | "rcases*" => {
                if num_lines == 1 {
                    ("", " }")
                } else if num_lines == 2 {
                    if i == 0 { ("", " ⎫") } else { ("", " ⎭") }
                } else if i == 0 {
                    ("", " ⎫")
                } else if i == num_lines - 1 {
                    ("", " ⎭")
                } else if i == num_lines / 2 {
                    ("", " ⎬")
                } else {
                    ("", " ⎪")
                }
            }
            "aligned" | "aligned*" | "gathered" | "gathered*" | "split" | "array" | "matrix" | "smallmatrix" => {
                ("", "")
            }
            _ => {
                // bmatrix, etc.
                if num_lines == 1 {
                    ("[", "]")
                } else if num_lines == 2 {
                    if i == 0 { ("⎡ ", " ⎤") } else { ("⎣ ", " ⎦") }
                } else if i == 0 {
                    ("⎡ ", " ⎤")
                } else if i == num_lines - 1 {
                    ("⎣ ", " ⎦")
                } else {
                    ("⎢ ", " ⎥")
                }
            }
        };

        if left.is_empty() && right.is_empty() {
            final_lines.push(inner.clone());
        } else {
            final_lines.push(format!("{}{}{}", left, inner, right));
        }
    }

    final_lines
}

fn format_equation_with_matrices(input: &str) -> String {
    let mut tokens: Vec<Vec<String>> = Vec::new();
    let mut remainder = input;

    while let Some(start) = remainder.find("\\begin{") {
        let after_begin = &remainder[start + 7..];
        if let Some(close_brace) = after_begin.find('}') {
            let env_name = &after_begin[..close_brace];
            if MATRIX_ENVS.contains(&env_name) {
                // Found matrix environment
                if start > 0 {
                    let pre = &remainder[..start];
                    let cleaned = clean_latex_formula(pre);
                    if !cleaned.is_empty() {
                        tokens.push(vec![cleaned]);
                    }
                }

                let body_start = start + 7 + close_brace + 1;
                if let Some(end_pos) = find_matching_end_env(&remainder[body_start..], env_name) {
                    let matrix_content = &remainder[body_start..body_start + end_pos];
                    let matrix_lines = parse_matrix_body(env_name, matrix_content);
                    tokens.push(matrix_lines);
                    let end_tag_len = format!("\\end{{{}}}", env_name).len();
                    remainder = &remainder[body_start + end_pos + end_tag_len..];
                } else {
                    // Unclosed matrix (e.g. streaming)
                    let matrix_content = &remainder[body_start..];
                    let matrix_lines = parse_matrix_body(env_name, matrix_content);
                    tokens.push(matrix_lines);
                    remainder = "";
                    break;
                }
                continue;
            }
        }
        let next_start = start + 7;
        let pre = &remainder[..next_start];
        let cleaned = clean_latex_formula(pre);
        if !cleaned.is_empty() {
            tokens.push(vec![cleaned]);
        }
        remainder = &remainder[next_start..];
    }

    if !remainder.is_empty() {
        let cleaned = clean_latex_formula(remainder);
        if !cleaned.is_empty() {
            tokens.push(vec![cleaned]);
        }
    }

    if tokens.is_empty() {
        return String::new();
    }

    let max_height = tokens.iter().map(|tok| tok.len()).max().unwrap_or(1);
    let token_widths: Vec<usize> = tokens
        .iter()
        .map(|tok| tok.iter().map(|l| str_char_count(l)).max().unwrap_or(0))
        .collect();

    let mut padded_tokens: Vec<Vec<String>> = Vec::new();
    for (tok_idx, tok) in tokens.iter().enumerate() {
        let w = token_widths[tok_idx];
        let h = tok.len();
        let top_pad = (max_height - h) / 2;
        let bot_pad = max_height - h - top_pad;

        let mut lines = Vec::new();
        for _ in 0..top_pad {
            lines.push(" ".repeat(w));
        }
        for line in tok {
            let line_w = str_char_count(line);
            let pad = w.saturating_sub(line_w);
            lines.push(format!("{}{}", line, " ".repeat(pad)));
        }
        for _ in 0..bot_pad {
            lines.push(" ".repeat(w));
        }
        padded_tokens.push(lines);
    }

    // Determine separators between tokens
    let mut separators = vec![""; tokens.len()];
    for i in 1..tokens.len() {
        separators[i] = " ";
    }

    let mut result_lines = Vec::new();
    for row in 0..max_height {
        let mut row_str = String::new();
        for (tok_idx, tok_lines) in padded_tokens.iter().enumerate() {
            if tok_idx > 0 {
                row_str.push_str(separators[tok_idx]);
            }
            row_str.push_str(&tok_lines[row]);
        }
        result_lines.push(row_str.trim_end().to_string());
    }

    result_lines.join("\n")
}

fn format_display_math(input: &str) -> String {
    if has_matrix_env(input) {
        format_equation_with_matrices(input)
    } else {
        clean_latex_formula(input)
    }
}

fn sanitize_markdown_math(input: &str) -> String {
    // Preserve existing markdown code blocks
    let code_blocks: Vec<&str> = input.split("```").collect();
    if code_blocks.len() > 1 {
        let mut result = String::new();
        for (i, block) in code_blocks.iter().enumerate() {
            if i % 2 == 1 {
                result.push_str("```");
                result.push_str(block);
                if i < code_blocks.len() - 1 || input.ends_with("```") {
                    result.push_str("```");
                }
            } else {
                let converted_tables = convert_markdown_tables(block);
                result.push_str(&sanitize_math_text(&converted_tables));
            }
        }
        result
    } else {
        let converted_tables = convert_markdown_tables(input);
        sanitize_math_text(&converted_tables)
    }
}

fn sanitize_math_text(input: &str) -> String {
    // Normalize LaTeX delimiters
    let mut normalized = input.to_string();

    normalized = normalized.replace("\\[", "\n$$\n");
    normalized = normalized.replace("\\]", "\n$$\n");
    normalized = normalized.replace("\\begin{equation}", "\n$$\n");
    normalized = normalized.replace("\\end{equation}", "\n$$\n");
    normalized = normalized.replace("\\begin{equation*}", "\n$$\n");
    normalized = normalized.replace("\\end{equation*}", "\n$$\n");
    normalized = normalized.replace("\\begin{align}", "\n$$\n\\begin{aligned}\n");
    normalized = normalized.replace("\\end{align}", "\n\\end{aligned}\n$$\n");
    normalized = normalized.replace("\\begin{align*}", "\n$$\n\\begin{aligned}\n");
    normalized = normalized.replace("\\end{align*}", "\n\\end{aligned}\n$$\n");
    normalized = normalized.replace("\\begin{gather}", "\n$$\n\\begin{gathered}\n");
    normalized = normalized.replace("\\end{gather}", "\n\\end{gathered}\n$$\n");
    normalized = normalized.replace("\\begin{gather*}", "\n$$\n\\begin{gathered}\n");
    normalized = normalized.replace("\\end{gather*}", "\n\\end{gathered}\n$$\n");
    normalized = normalized.replace("\\(", "$");
    normalized = normalized.replace("\\)", "$");

    // Wrap standalone matrix environments outside $$ in $$
    for env in MATRIX_ENVS {
        let start_tag = format!("\\begin{{{}}}", env);
        let end_tag = format!("\\end{{{}}}", env);
        if normalized.contains(&start_tag) {
            let mut search_idx = 0;
            while let Some(start) = normalized[search_idx..].find(&start_tag) {
                let actual_start = search_idx + start;
                let before = &normalized[..actual_start];
                let dollar_count = before.matches("$$").count();
                if dollar_count % 2 == 0 {
                    // Outside $$, wrap it
                    if let Some(end) = normalized[actual_start..].find(&end_tag) {
                        let actual_end = actual_start + end + end_tag.len();
                        let matrix_chunk = &normalized[actual_start..actual_end];
                        let wrapped = format!("\n$$\n{}\n$$\n", matrix_chunk);
                        normalized = format!("{}{}{}", &normalized[..actual_start], wrapped, &normalized[actual_end..]);
                        search_idx = actual_start + wrapped.len();
                        continue;
                    }
                }
                search_idx = actual_start + start_tag.len();
            }
        }
    }

    let parts: Vec<&str> = normalized.split("$$").collect();
    if parts.len() > 1 {
        let mut result = String::new();
        for (i, part) in parts.iter().enumerate() {
            if i % 2 == 1 {
                let formatted = format_display_math(part);
                result.push_str(&format!("\n```\n{}\n```\n", formatted.trim()));
            } else {
                result.push_str(&sanitize_inline_math(part));
            }
        }
        result
    } else {
        sanitize_inline_math(&normalized)
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
                if has_matrix_env(&buf) {
                    let formatted = format_equation_with_matrices(&buf);
                    if formatted.contains('\n') {
                        result.push_str(&format!("\n```\n{}\n```\n", formatted.trim()));
                    } else {
                        result.push_str(&formatted);
                    }
                } else {
                    result.push_str(&clean_latex_formula(&buf));
                }
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

fn convert_blackboard(input: &str) -> String {
    let mut s = input.to_string();
    while let Some(start) = s.find("\\mathbb{") {
        let prefix_len = 8;
        if let Some(close) = s[start + prefix_len..].find('}') {
            let inner = &s[start + prefix_len..start + prefix_len + close];
            let converted: String = inner
                .chars()
                .map(|c| match c {
                    'C' => 'ℂ',
                    'H' => 'ℍ',
                    'N' => 'ℕ',
                    'P' => 'ℙ',
                    'Q' => 'ℚ',
                    'R' => 'ℝ',
                    'Z' => 'ℤ',
                    '0' => '𝟘',
                    '1' => '𝟙',
                    '2' => '𝟚',
                    '3' => '𝟛',
                    '4' => '𝟜',
                    '5' => '𝟝',
                    '6' => '𝟞',
                    '7' => '𝟟',
                    '8' => '𝟠',
                    '9' => '𝟡',
                    'A'..='Z' => {
                        let offset = c as u32 - 'A' as u32;
                        char::from_u32(0x1D538 + offset).unwrap_or(c)
                    }
                    'a'..='z' => {
                        let offset = c as u32 - 'a' as u32;
                        char::from_u32(0x1D552 + offset).unwrap_or(c)
                    }
                    _ => c,
                })
                .collect();
            s = format!("{}{}{}", &s[..start], converted, &s[start + prefix_len + close + 1..]);
        } else {
            break;
        }
    }
    s
}

fn convert_script(input: &str) -> String {
    let mut s = input.to_string();
    for prefix in &["\\mathcal{", "\\mathscr{"] {
        while let Some(start) = s.find(prefix) {
            let prefix_len = prefix.len();
            if let Some(close) = s[start + prefix_len..].find('}') {
                let inner = &s[start + prefix_len..start + prefix_len + close];
                let converted: String = inner
                    .chars()
                    .map(|c| match c {
                        'B' => 'ℬ',
                        'E' => 'ℰ',
                        'F' => 'ℱ',
                        'H' => 'ℋ',
                        'I' => 'ℐ',
                        'L' => 'ℒ',
                        'M' => 'ℳ',
                        'R' => 'ℛ',
                        'A'..='Z' => {
                            let offset = c as u32 - 'A' as u32;
                            char::from_u32(0x1D49C + offset).unwrap_or(c)
                        }
                        _ => c,
                    })
                    .collect();
                s = format!("{}{}{}", &s[..start], converted, &s[start + prefix_len + close + 1..]);
            } else {
                break;
            }
        }
    }
    s
}

fn convert_operator_limits_depth(input: &str, depth: usize) -> String {
    const OPS: &[(&str, &str)] = &[
        ("\\bigoplus", "⨁"),
        ("\\bigotimes", "⨂"),
        ("\\bigcup", "⋃"),
        ("\\bigcap", "⋂"),
        ("\\bigvee", "⋁"),
        ("\\bigwedge", "⋀"),
        ("\\coprod", "∐"),
        ("\\iiint", "∭"),
        ("\\iint", "∬"),
        ("\\oint", "∮"),
        ("\\sum", "∑"),
        ("\\prod", "∏"),
        ("\\int", "∫"),
    ];

    let mut out = String::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;

    let extract_limit = |pos: usize| -> (Option<String>, usize) {
        let mut idx = pos;
        while idx < chars.len() && chars[idx].is_whitespace() {
            idx += 1;
        }
        if idx >= chars.len() {
            return (None, pos);
        }
        if chars[idx] == '{' {
            let mut d = 1usize;
            let start = idx + 1;
            idx += 1;
            while idx < chars.len() && d > 0 {
                if chars[idx] == '{' {
                    d += 1;
                } else if chars[idx] == '}' {
                    d -= 1;
                }
                idx += 1;
            }
            if d == 0 {
                let inner: String = chars[start..idx - 1].iter().collect();
                (Some(inner), idx)
            } else {
                let inner: String = chars[start..].iter().collect();
                (Some(inner), chars.len())
            }
        } else if chars[idx] == '\\' {
            let start = idx;
            idx += 1;
            while idx < chars.len() && chars[idx].is_ascii_alphabetic() {
                idx += 1;
            }
            let inner: String = chars[start..idx].iter().collect();
            (Some(inner), idx)
        } else {
            let inner = chars[idx].to_string();
            (Some(inner), idx + 1)
        }
    };

    while i < chars.len() {
        let mut matched_op: Option<(&str, &str)> = None;
        let rem: String = chars[i..].iter().collect();

        for &(op_tex, op_sym) in OPS {
            if rem.starts_with(op_tex) {
                let op_char_len = op_tex.chars().count();
                let after = i + op_char_len;
                if after < chars.len() && chars[after].is_ascii_alphabetic() {
                    continue;
                }
                matched_op = Some((op_tex, op_sym));
                break;
            }
        }

        if let Some((op_tex, op_sym)) = matched_op {
            let op_char_len = op_tex.chars().count();
            let mut cur = i + op_char_len;
            let mut sub: Option<String> = None;
            let mut sup: Option<String> = None;

            for _ in 0..2 {
                let mut temp = cur;
                while temp < chars.len() && chars[temp].is_whitespace() {
                    temp += 1;
                }
                if temp < chars.len() {
                    if chars[temp] == '_' && sub.is_none() {
                        let (lim, next_pos) = extract_limit(temp + 1);
                        if let Some(l) = lim {
                            sub = Some(l);
                            cur = next_pos;
                            continue;
                        }
                    } else if chars[temp] == '^' && sup.is_none() {
                        let (lim, next_pos) = extract_limit(temp + 1);
                        if let Some(l) = lim {
                            sup = Some(l);
                            cur = next_pos;
                            continue;
                        }
                    }
                }
                break;
            }

            match (sub, sup) {
                (Some(b), Some(p)) => {
                    let cleaned_b = clean_latex_formula_depth(&b, depth);
                    let cleaned_p = clean_latex_formula_depth(&p, depth);
                    out.push_str(&format!("{}[{}..{}]", op_sym, cleaned_b, cleaned_p));
                }
                (Some(b), None) => {
                    let cleaned_b = clean_latex_formula_depth(&b, depth);
                    out.push_str(&format!("{}[{}]", op_sym, cleaned_b));
                }
                (None, Some(p)) => {
                    let cleaned_p = clean_latex_formula_depth(&p, depth);
                    out.push_str(&format!("{}[..{}]", op_sym, cleaned_p));
                }
                (None, None) => {
                    out.push_str(op_sym);
                }
            }
            i = cur;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }

    out
}

fn replace_latex_command(mut s: String, from_cmd: &str, to_sym: &str) -> String {
    let check_alpha = from_cmd.starts_with('\\')
        && from_cmd
            .chars()
            .last()
            .map(|c| c.is_ascii_alphabetic())
            .unwrap_or(false);

    let mut idx = 0;
    while let Some(pos) = s[idx..].find(from_cmd) {
        let actual_pos = idx + pos;
        let after = actual_pos + from_cmd.len();
        if check_alpha && after < s.len() {
            let next_char = s[after..].chars().next().unwrap();
            if next_char.is_ascii_alphabetic() {
                idx = actual_pos + 1;
                continue;
            }
        }
        s = format!("{}{}{}", &s[..actual_pos], to_sym, &s[after..]);
        idx = actual_pos + to_sym.len();
    }
    s
}

fn clean_latex_formula(input: &str) -> String {
    clean_latex_formula_depth(input, 0)
}

fn clean_latex_formula_depth(input: &str, depth: usize) -> String {
    let mut s = input.to_string();

    // 1. Strip font/style wrappers
    for prefix in &[
        "\\mathbf{", "\\mathit{", "\\mathrm{", "\\textrm{", "\\textbf{", "\\textit{", "\\text{",
        "\\bm{", "\\boldsymbol{", "\\symbf{", "\\mathsf{", "\\mathtt{",
        "\\operatorname*{", "\\operatorname{",
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

    // Collapse multiple spaces caused by unwrapping (e.g. \text{if } j = 0)
    while s.contains("  ") {
        s = s.replace("  ", " ");
    }

    // 2. Blackboard bold & script
    s = convert_blackboard(&s);
    s = convert_script(&s);

    // 3. Strip backslash from math function names
    let math_funcs = [
        ("\\cos", "cos"), ("\\sin", "sin"), ("\\tan", "tan"),
        ("\\sec", "sec"), ("\\csc", "csc"), ("\\cot", "cot"),
        ("\\arcsin", "arcsin"), ("\\arccos", "arccos"), ("\\arctan", "arctan"),
        ("\\cosh", "cosh"), ("\\sinh", "sinh"), ("\\tanh", "tanh"),
        ("\\ln", "ln"), ("\\log", "log"), ("\\exp", "exp"),
        ("\\det", "det"), ("\\dim", "dim"), ("\\ker", "ker"),
        ("\\deg", "deg"), ("\\gcd", "gcd"),
        ("\\min", "min"), ("\\max", "max"), ("\\sup", "sup"),
        ("\\inf", "inf"), ("\\lim", "lim"),
        ("\\Pr", "Pr"), ("\\var", "Var"), ("\\Var", "Var"),
        ("\\cov", "Cov"), ("\\Cov", "Cov"),
        ("\\argmax", "argmax"), ("\\argmin", "argmin"), ("\\arg", "arg"),
    ];
    for (from, to) in math_funcs {
        while let Some(start) = s.find(from) {
            let after = start + from.len();
            if after < s.len() {
                let next_char = s[after..].chars().next().unwrap();
                if next_char.is_ascii_alphabetic() {
                    break;
                }
                let replacement = if next_char == '(' || next_char == ' ' || next_char == '^' || next_char == '_' {
                    to.to_string()
                } else {
                    format!("{} ", to)
                };
                s = format!("{}{}{}", &s[..start], replacement, &s[after..]);
            } else {
                s = format!("{}{}", &s[..start], to);
            }
        }
    }

    // 4. Fractions
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

    // 5. Roots
    while let Some(start) = s.find("\\sqrt{") {
        if let Some(close) = s[start + 6..].find('}') {
            let inner = &s[start + 6..start + 6 + close];
            s = format!("{}√({}){}", &s[..start], inner, &s[start + 6 + close + 1..]);
        } else {
            break;
        }
    }

    // 6. Big operators with limits (convert \sum_{d=0}^{u} -> ∑[d=0..u])
    if depth < 2 {
        s = convert_operator_limits_depth(&s, depth + 1);
    }

    // 7. Accents
    let accents = [
        ("\\hat{", "\u{0302}"),
        ("\\bar{", "\u{0304}"),
        ("\\vec{", "\u{20D7}"),
        ("\\dot{", "\u{0307}"),
        ("\\ddot{", "\u{0308}"),
        ("\\tilde{", "\u{0303}"),
    ];
    for &(prefix, mark) in &accents {
        while let Some(start) = s.find(prefix) {
            let len = prefix.len();
            if let Some(close) = s[start + len..].find('}') {
                let inner = &s[start + len..start + len + close];
                let accented = if inner.chars().count() == 1 {
                    format!("{}{}", inner, mark)
                } else {
                    inner.to_string()
                };
                s = format!("{}{}{}", &s[..start], accented, &s[start + len + close + 1..]);
            } else {
                break;
            }
        }
    }

    // 8. Delimiters, symbols, arrows, greek
    let replacements = [
        ("\\left(", "("),
        ("\\right)", ")"),
        ("\\left[", "["),
        ("\\right]", "]"),
        ("\\left\\{", "{"),
        ("\\right\\}", "}"),
        ("\\left|", "|"),
        ("\\right|", "|"),
        ("\\left.", ""),
        ("\\right.", ""),
        ("\\{", "{"),
        ("\\}", "}"),
        ("\\langle", "⟨"),
        ("\\rangle", "⟩"),
        ("\\lceil", "⌈"),
        ("\\rceil", "⌉"),
        ("\\lfloor", "⌊"),
        ("\\rfloor", "⌋"),
        ("\\Vert", "‖"),
        ("\\|", "‖"),
        ("\\middle|", "|"),
        ("\\forall", "∀"),
        ("\\exists", "∃"),
        ("\\nexists", "∄"),
        ("\\in", "∈"),
        ("\\notin", "∉"),
        ("\\ni", "∋"),
        ("\\owns", "∋"),
        ("\\subseteq", "⊆"),
        ("\\subset", "⊂"),
        ("\\supseteq", "⊇"),
        ("\\supset", "⊃"),
        ("\\emptyset", "∅"),
        ("\\varnothing", "∅"),
        ("\\cap", "∩"),
        ("\\cup", "∪"),
        ("\\setminus", "\\"),
        ("\\leq", "≤"),
        ("\\le", "≤"),
        ("\\geq", "≥"),
        ("\\ge", "≥"),
        ("\\neq", "≠"),
        ("\\ne", "≠"),
        ("\\approx", "≈"),
        ("\\sim", "∼"),
        ("\\equiv", "≡"),
        ("\\propto", "∝"),
        ("\\times", "×"),
        ("\\cdot", "·"),
        ("\\div", "÷"),
        ("\\pm", "±"),
        ("\\mp", "∓"),
        ("\\infty", "∞"),
        ("\\partial", "∂"),
        ("\\nabla", "∇"),
        ("\\sum", "∑"),
        ("\\prod", "∏"),
        ("\\coprod", "∐"),
        ("\\int", "∫"),
        ("\\iint", "∬"),
        ("\\iiint", "∭"),
        ("\\oint", "∮"),
        ("\\sqrt", "√"),
        ("\\implies", "⇒"),
        ("\\impliedby", "⇐"),
        ("\\iff", "⇔"),
        ("\\land", "∧"),
        ("\\lor", "∨"),
        ("\\neg", "¬"),
        ("\\lnot", "¬"),
        ("\\rightarrow", "→"),
        ("\\to", "→"),
        ("\\leftarrow", "←"),
        ("\\Rightarrow", "⇒"),
        ("\\Leftarrow", "⇐"),
        ("\\Leftrightarrow", "⇔"),
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
        ("\\vartheta", "ϑ"),
        ("\\Theta", "Θ"),
        ("\\iota", "ι"),
        ("\\kappa", "κ"),
        ("\\lambda", "λ"),
        ("\\Lambda", "Λ"),
        ("\\mu", "μ"),
        ("\\nu", "ν"),
        ("\\xi", "ξ"),
        ("\\Xi", "Ξ"),
        ("\\pi", "π"),
        ("\\varpi", "ϖ"),
        ("\\Pi", "Π"),
        ("\\rho", "ρ"),
        ("\\varrho", "ϱ"),
        ("\\sigma", "σ"),
        ("\\varsigma", "ς"),
        ("\\Sigma", "Σ"),
        ("\\tau", "τ"),
        ("\\upsilon", "υ"),
        ("\\Upsilon", "Υ"),
        ("\\phi", "φ"),
        ("\\varphi", "ϕ"),
        ("\\Phi", "Φ"),
        ("\\chi", "χ"),
        ("\\psi", "ψ"),
        ("\\Psi", "Ψ"),
        ("\\omega", "ω"),
        ("\\Omega", "Ω"),
        ("\\quad", "  "),
        ("\\qquad", "    "),
        ("\\,", " "),
        ("\\;", " "),
        ("\\:", " "),
        ("\\!", ""),
        ("\\cdots", "..."),
        ("\\ldots", "..."),
        ("\\dots", "..."),
        ("\\ddots", "⋱"),
        ("\\vdots", "⋮"),
        ("\\dagger", "†"),
        ("\\ddagger", "‡"),
        ("\\star", "⋆"),
        ("\\ast", "*"),
        ("\\oplus", "⊕"),
        ("\\otimes", "⊗"),
        ("\\odot", "⊙"),
        ("\\circ", "∘"),
        ("\\bullet", "•"),
        ("\\perp", "⟂"),
        ("\\parallel", "∥"),
        ("~", " "),
    ];

    for (from, to) in replacements {
        s = replace_latex_command(s, from, to);
    }

    // 9. Braced subscripts and superscripts
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

    // 10. Single character subscripts and superscripts
    let single_sub = [
        ("_0", "₀"), ("_1", "₁"), ("_2", "₂"), ("_3", "₃"), ("_4", "₄"),
        ("_5", "₅"), ("_6", "₆"), ("_7", "₇"), ("_8", "₈"), ("_9", "₉"),
        ("_+", "₊"), ("_-", "₋"), ("_=", "₌"),
        ("_a", "ₐ"), ("_e", "ₑ"), ("_h", "ₕ"),
        ("_i", "ᵢ"), ("_j", "ⱼ"), ("_k", "ₖ"),
        ("_l", "ₗ"), ("_m", "ₘ"), ("_n", "ₙ"),
        ("_o", "ₒ"), ("_p", "ₚ"), ("_r", "ᵣ"),
        ("_s", "ₛ"), ("_t", "ₜ"), ("_u", "ᵤ"),
        ("_v", "ᵥ"), ("_x", "ₓ"),
    ];
    for (from, to) in single_sub {
        s = s.replace(from, to);
    }

    let single_sup = [
        ("^0", "⁰"), ("^1", "¹"), ("^2", "²"), ("^3", "³"), ("^4", "⁴"),
        ("^5", "⁵"), ("^6", "⁶"), ("^7", "⁷"), ("^8", "⁸"), ("^9", "⁹"),
        ("^+", "⁺"), ("^-", "⁻"), ("^=", "⁼"),
        ("^a", "ᵃ"), ("^b", "ᵇ"), ("^c", "ᶜ"), ("^d", "ᵈ"), ("^e", "ᵉ"),
        ("^f", "ᶠ"), ("^g", "ᵍ"), ("^h", "ʰ"), ("^i", "ⁱ"), ("^j", "ʲ"),
        ("^k", "ᵏ"), ("^l", "ˡ"), ("^m", "ᵐ"), ("^n", "ⁿ"), ("^o", "ᵒ"),
        ("^p", "ᵖ"), ("^r", "ʳ"), ("^s", "ˢ"), ("^t", "ᵗ"), ("^u", "ᵘ"),
        ("^v", "ᵛ"), ("^w", "ʷ"), ("^x", "ˣ"), ("^y", "ʸ"), ("^z", "ᶻ"),
        ("^T", "ᵀ"), ("^*", "*"),
    ];
    for (from, to) in single_sup {
        s = s.replace(from, to);
    }

    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_array_matrix_output() {
        let prompt1 = r#"$$\begin{array}{c|c|c|} 
& y_1 & y_2 \\ \hline
x_1 & (5, 1) & (0, 0) \\ \hline
x_2 & (4, 4) & (1, 5) \\ \hline
\end{array}$$"#;
        let out1 = sanitize_markdown_math(prompt1);
        assert!(out1.contains("y₁"));
        assert!(out1.contains("y₂"));
        assert!(out1.contains("(5, 1)"));
        assert!(out1.contains("(4, 4)"));

        let prompt3 = r#"$$\begin{array}{cc} & \begin{array}{cc} \text{Col 1} & \text{Col 2} \end{array} \\ \begin{array}{cc} \text{Row 1} \\ \text{Row 2} \end{array} & \begin{pmatrix} (2, 1) & (0, 0) \\ (0, 0) & (1, 2) \end{pmatrix} \end{array}$$"#;
        let out3 = sanitize_markdown_math(prompt3);
        assert!(out3.contains("Col 1"));
        assert!(out3.contains("Col 2"));
        assert!(out3.contains("Row 1"));
        assert!(out3.contains("Row 2"));
        assert!(out3.contains("⎛ (2, 1)"));
        assert!(out3.contains("⎝ (0, 0)"));
    }

    #[test]
    fn test_markdown_table_to_box_drawing() {
        let table_md = "Here is the payoff matrix:\n\n| | Left | Right |\n|---|---|---|\n| Top | (2, 1) | (0, 0) |\n| Bottom | (0, 0) | (1, 2) |\n\nDone.";
        let sanitized = sanitize_markdown_math(table_md);
        assert!(sanitized.contains("```"));
        assert!(sanitized.contains("┌"));
        assert!(sanitized.contains("Left"));
        assert!(sanitized.contains("Right"));
        assert!(sanitized.contains("Top"));
        assert!(sanitized.contains("(2, 1)"));
        assert!(sanitized.contains("Bottom"));
        assert!(sanitized.contains("┘"));

        // Verify iced markdown::parse actually parses it into items (as a CodeBlock) rather than dropping it
        let items: Vec<markdown::Item> = markdown::parse(&sanitized).collect();
        assert!(!items.is_empty());
        let has_code_block = items.iter().any(|item| matches!(item, markdown::Item::CodeBlock(_)));
        assert!(has_code_block, "Markdown table must be converted to a CodeBlock so iced renders it");
    }

    #[test]
    fn test_rotation_matrix_formatting() {
        let input = r"$$R(\theta) = \begin{bmatrix} \cos\theta & -\sin\theta \\ \sin\theta & \cos\theta \end{bmatrix}$$";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("R(θ) = ⎡"));
        assert!(sanitized.contains("cos θ"));
        assert!(sanitized.contains("-sin θ"));
        assert!(sanitized.contains("⎣"));
    }

    #[test]
    fn test_determinant_expansion() {
        let input = r"$$\det(A) = 1 \cdot \begin{vmatrix} 4 & 5 \\ 0 & 6 \end{vmatrix}$$";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("det(A) = 1 · │"));
        assert!(sanitized.contains("│ 4   5 │"));
        assert!(sanitized.contains("│ 0   6 │"));
    }

    #[test]
    fn test_standalone_matrix_without_dollar() {
        let input = "The matrix is:\n\\begin{pmatrix} 1 & 2 \\\\ 3 & 4 \\end{pmatrix}\nEnd";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("```"));
        assert!(sanitized.contains("⎛"));
        assert!(sanitized.contains("⎝"));
    }

    #[test]
    fn test_math_functions_and_symbols() {
        let input = r"$$\cos^2\theta + \sin^2\theta = 1$$";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("cos²θ + sin²θ = 1"));
    }

    #[test]
    fn test_3x3_matrix_formatting() {
        let input = r"$$\begin{pmatrix} 1 & 2 & 3 \\ 0 & 4 & 5 \\ 1 & 0 & 6 \end{pmatrix}$$";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("⎛ 1   2   3 ⎞"));
        assert!(sanitized.contains("⎜ 0   4   5 ⎟"));
        assert!(sanitized.contains("⎝ 1   0   6 ⎠"));
    }

    #[test]
    fn test_multiple_matrices_multiplication() {
        let input = r"$$\begin{pmatrix} x' \\ y' \end{pmatrix} = \begin{pmatrix} a & b \\ c & d \end{pmatrix} \begin{pmatrix} x \\ y \end{pmatrix}$$";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("⎛ x' ⎞ = ⎛ a   b ⎞ ⎛ x ⎞"));
        assert!(sanitized.contains("⎝ y' ⎠   ⎝ c   d ⎠ ⎝ y ⎠"));
    }

    #[test]
    fn test_inline_matrix_in_bullet_point() {
        let input = "* $\\begin{vmatrix} 4 & 5 \\\\ 0 & 6 \\end{vmatrix}$ = 24";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("```"));
        assert!(sanitized.contains("│ 4   5 │"));
        assert!(sanitized.contains("│ 0   6 │"));
        assert!(sanitized.contains("= 24"));
    }

    #[test]
    fn test_config_system_instruction_persistence() {
        let mut cfg = Config::default();
        assert!(!cfg.system_instruction.is_empty());
        cfg.system_instruction = "Custom instruction for testing".to_string();
        let serialized = serde_json::to_string(&cfg).unwrap();
        let deserialized: Config = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.system_instruction, "Custom instruction for testing");
    }

    #[test]
    fn test_code_block_preservation() {
        let input = "Here is Python code:\n```python\nval = \"$$\\begin{matrix} 1 & 2 \\end{matrix}$$\"\n```\nAnd math: $x^2$";
        let sanitized = sanitize_markdown_math(input);
        // Code block interior should remain untouched
        assert!(sanitized.contains("val = \"$$\\begin{matrix} 1 & 2 \\end{matrix}$$\""));
        // Math outside should be converted
        assert!(sanitized.contains("x²"));
    }

    #[test]
    fn test_circular_logger() {
        let temp_dir = std::env::temp_dir().join(format!("ctg_test_log_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).ok();
        crate::logger::set_custom_log_dir(Some(temp_dir.clone()));

        crate::logger::init(true);
        assert!(crate::logger::is_enabled());
        log_info!("test", "Testing log entry");
        let path = crate::logger::log_file_path();
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Testing log entry"));
        crate::logger::set_enabled(false);
        assert!(!crate::logger::is_enabled());
        crate::logger::clear_logs();

        crate::logger::set_custom_log_dir(None);
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_log_rotation_limit() {
        let temp_dir = std::env::temp_dir().join(format!("ctg_test_rot_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).ok();
        crate::logger::set_custom_log_dir(Some(temp_dir.clone()));

        crate::logger::init(true);
        crate::logger::clear_logs();

        // Write ~6.5 MB of data to trigger multiple rotations
        let chunk = "X".repeat(512 * 1024); // 512 KB
        for i in 0..14 {
            log_info!("test_rotation", "Iteration {}: {}", i, chunk);
        }

        let dir = crate::logger::log_dir();
        // Check that app.log exists
        assert!(dir.join("app.log").exists());
        // Check rotated backups exist
        assert!(dir.join("app.log.1").exists());
        assert!(dir.join("app.log.2").exists());
        assert!(dir.join("app.log.3").exists());
        // Check app.log.4 does NOT exist (strictly capped at MAX_BACKUPS = 3)
        assert!(!dir.join("app.log.4").exists());

        crate::logger::set_enabled(false);
        crate::logger::clear_logs();

        crate::logger::set_custom_log_dir(None);
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[test]
    fn test_gemini_expectation_and_summation_limits() {
        let input = r"$$F(u) = \mathbb{E}[h(u - D)^+] = \sum_{d=0}^{u} (u - d) p_d$$";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("F(u) = 𝔼[h(u - D)⁺] = ∑[d=0..u] (u - d) p_d"));
    }

    #[test]
    fn test_gemini_indicator_and_inline_math() {
        let input = r"1. The ordering cost: $K \cdot \mathbb{I}(a > 0) + c \cdot a = 2\mathbb{I}(a > 0) + 1 \cdot a$";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("K · 𝕀(a > 0) + c · a = 2𝕀(a > 0) + 1 · a"));
    }

    #[test]
    fn test_gemini_piecewise_cases_rendering() {
        let input = r"$$f(j) = \begin{cases} 0 & \text{if } j = 0 \\ K + c \cdot j = 2 + 1 \cdot j & \text{if } j > 0 \end{cases}$$";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("```"));
        assert!(sanitized.contains("f(j) = ⎧ 0"));
        assert!(sanitized.contains("if j = 0"));
        assert!(sanitized.contains("⎩ K + c · j = 2 + 1 · j"));
        assert!(sanitized.contains("if j > 0"));
    }

    #[test]
    fn test_script_and_delimiters() {
        let input = r"$$\mathcal{S} = \{0, 1, 2\}, \quad \mathcal{A} = \{0, 1, 2\}$$";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("𝒮 = {0, 1, 2}"));
        assert!(sanitized.contains("𝒜 = {0, 1, 2}"));
    }

    #[test]
    fn test_integrals_and_infinite_limits() {
        let input = r"$$\int_{0}^{\infty} e^{-x} dx = 1$$";
        let sanitized = sanitize_markdown_math(input);
        assert!(sanitized.contains("∫[0..∞] e⁻ˣ dx = 1"));
    }

    #[test]
    fn test_mdp_inventory_response_end_to_end() {
        let sample = r#"
* **Inventory State Space:** $S = \{0, 1, 2\}$
* **Demand Probabilities:**
  $$P(D = 0) = 0.5, \quad P(D = 1) = 0.3, \quad P(D = 2) = 0.2$$
* **Ordering Cost:**
  $$f(j) = \begin{cases} 0 & \text{if } j = 0 \\ K + c \cdot j = 2 + 1 \cdot j & \text{if } j > 0 \end{cases}$$
* **Holding Cost:**
  $$F(u) = \mathbb{E}[h(u - D)^+] = \sum_{d=0}^{u} (u - d) p_d$$
* **Step C:**
  1. The ordering cost: $K \cdot \mathbb{I}(a > 0) + c \cdot a = 2\mathbb{I}(a > 0) + 1 \cdot a$
  2. The expected holding cost:
  $$r(s, a) = \text{Ordering Cost}(a) + F(s + a)$$

| State ($s$) | Action ($a$) | Cost $r(s, a)$ |
| :---: | :---: | :---: |
| $0$ | $0$ | **$0.0$** |
"#;
        let sanitized = sanitize_markdown_math(sample);
        assert!(sanitized.contains("S = {0, 1, 2}"));
        assert!(sanitized.contains("P(D = 0) = 0.5,    P(D = 1) = 0.3,    P(D = 2) = 0.2"));
        assert!(sanitized.contains("f(j) = ⎧ 0"));
        assert!(sanitized.contains("⎩ K + c · j = 2 + 1 · j"));
        assert!(sanitized.contains("F(u) = 𝔼[h(u - D)⁺] = ∑[d=0..u] (u - d) p_d"));
        assert!(sanitized.contains("2𝕀(a > 0) + 1 · a"));
        assert!(sanitized.contains("r(s, a) = Ordering Cost(a) + F(s + a)"));
        assert!(sanitized.contains("┌"));
        assert!(sanitized.contains("State (s)"));
        assert!(sanitized.contains("┘"));
    }
}
