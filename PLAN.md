# Circle to Gemini — Build Plan

> **Project:** A lightweight Rust desktop tool for Linux (KDE Plasma 6 / Wayland) that lets you select any region on screen and query Gemini AI about it — like Android's "Circle to Search" + Gemini overlay, but for desktop.

---

## 1. System Environment (Verified)

| Item | Value |
|------|-------|
| **OS** | CachyOS (Arch-based), rolling release |
| **Desktop** | KDE Plasma 6 |
| **Display Server** | Wayland (`wayland-1`), X11 fallback (`:0`) |
| **Screens** | 2× 1920×1080 (eDP-1 + Virtual-hp), scale 1.0 |
| **Rust** | 1.99.1 (system package, no rustup) |
| **Cargo** | 1.99.1 |
| **AUR Helper** | `paru` |
| **Screenshot Tool** | `spectacle` 6.8.2 (KDE native, Wayland-safe) |
| **OCR** | `tesseract` installed, `pytesseract` NOT installed |
| **Browser Open** | `xdg-open` available |
| **System Tray** | KDE uses StatusNotifierItem (SNI) via D-Bus |
| **D-Bus** | 1.16.4 |
| **Key System Libs** | wayland 1.26, wayland-protocols 1.50, libxkbcommon 1.13, vulkan-icd-loader 1.4, mesa 26.2, fontconfig 2.18, freetype2 2.15, openssl 4.1, libappindicator 12.10 |

### Verified Working
- [x] `spectacle -m -b -n -o /path.png` captures current monitor silently (1920×1080)
- [x] `spectacle -f -b -n -o /path.png` captures all screens (3840×1080)
- [x] Google Lens upload via `POST https://lens.google.com/v3/upload` returns 303 redirect with search URL
- [x] `agy -p "prompt"` works for CLI Gemini queries (auth handled by agy)
- [x] iced 0.13.1 compiles successfully (resolved from `0.13` spec; `0.14` not yet on crates.io)
- [x] PyQt6 fullscreen window works on Wayland (but we're using Rust/iced instead)
- [x] `tray-icon` crate with `ksni` feature is the correct approach for KDE Wayland tray

---

## 2. What Exists on the Internet (Research Summary)

| Tool | What it does | Why it doesn't satisfy our need |
|------|-------------|-------------------------------|
| `jaslrobinson/circle-to-search` (AUR) | Draw shape → Google Lens in browser | No AI assistant card, no Gemini, just opens browser tab |
| `shashwathv/KenXSearch` | Freehand circle → Google Lens | Same — browser-only, no conversational AI |
| `SIGMazer/circle2search` | X11 area select → Google Translate/Lens | X11-only, no Wayland, no Gemini |
| `AmerAlreyahi/OpenCircle` (Rust) | Circle/snip → translate/OCR/AI | Windows-focused (`windows-rs`), Linux experimental, no integrated floating card |
| **Google Gemini Desktop App** | Native "Ask about this screen" | **Windows & macOS only — no Linux version exists** |

**Gap:** Nothing provides the Android-style experience of: shortcut → circle/select → floating AI card with streaming Gemini response, on Linux.

---

## 3. Architecture

### 3.1 UX Flow

```
User presses shortcut (Meta+G) OR clicks tray icon
  │
  ▼
Screen freezes (spectacle captures current monitor)
  │
  ▼
Fullscreen overlay appears showing captured screenshot
  ├── Dimmed with 45% dark overlay
  ├── Instruction text: "Draw a rectangle to select an area (Esc to cancel)"
  └── Crosshair cursor
  │
  ▼
User draws rectangle (click+drag) or freehand lasso
  ├── Glowing Gemini-blue selection border with corner handles
  └── Right-click or Esc cancels
  │
  ▼
Selection made → overlay closes → cropped image saved to temp file
  │
  ▼
Floating Assistant Card appears (new window, ~500×600)
  ┌────────────────────────────────────────────────────────────┐
  │   Circle to Gemini                          []  []   │
  │  ┌──────────────────────────────────────────────────────┐  │
  │  │          [ Cropped Image Preview ]                   │  │
  │  └──────────────────────────────────────────────────────┘  │
  │                                                            │
  │  [ Explain] [ Copy Text] [ Google Lens] [ Debug]  │
  │                                                            │
  │  ┌──────────────────────────────────────────────┐  [Send] │
  │  │  Ask anything about this...                   │         │
  │  └──────────────────────────────────────────────┘         │
  │                                                            │
  │  ┌──────────────────────────────────────────────────────┐  │
  │  │   Streaming response area                          │  │
  │  │  (scrollable, monospace for code)                    │  │
  │  └──────────────────────────────────────────────────────┘  │
  │                                                            │
  │  API Key: [not set — click  to configure]               │
  └────────────────────────────────────────────────────────────┘
```

### 3.2 Component Diagram

```
┌─────────────────────────────────────────────────────────┐
│                    main.rs                               │
│  - Spawns tray icon thread                              │
│  - Starts iced::daemon (no initial window)              │
│  - Receives tray click / shortcut via channel           │
└──────────────────────┬──────────────────────────────────┘
                       │
        ┌──────────────┼──────────────┐
        ▼              ▼              ▼
   ┌─────────┐  ┌───────────┐  ┌──────────┐
   │ tray.rs │  │  app.rs   │  │config.rs │
   │         │  │ (iced     │  │(API key, │
   │ SNI via │  │  daemon)  │  │ model,   │
   │ ksni    │  │           │  │ persist) │
   └────┬────┘  └─────┬─────┘  └──────────┘
        │              │
        │   ┌──────────┴──────────┐
        │   ▼                     ▼
        │ ┌──────────────┐  ┌──────────────┐
        └→│ overlay.rs   │  │ assistant    │
          │ (Window 1:   │  │ card (inside │
          │  fullscreen  │  │  app.rs      │
          │  canvas +    │  │  Window 2)   │
          │  drawing)    │  └──────┬───────┘
          └──────────────┘         │
                            ┌──────┴───────┐
                            ▼              ▼
                      ┌──────────┐  ┌──────────┐
                      │gemini.rs │  │ lens.rs  │
                      │(API call │  │(upload   │
                      │streaming)│  │to Lens)  │
                      └──────────┘  └──────────┘
```

### 3.3 Tech Stack

| Component | Choice | Reason |
|-----------|--------|--------|
| **Language** | Rust | User requirement — lightweight, fast, single binary |
| **GUI Framework** | `iced` 0.13 | Pure Rust, Wayland-native, multi-window via daemon mode, canvas widget for drawing |
| **System Tray** | `tray-icon` with `ksni` feature | Native KDE StatusNotifierItem via D-Bus, no GTK dependency |
| **Screenshot** | `spectacle` CLI | Already installed, Wayland-native on KDE, reliable |
| **AI Backend** | Gemini API (`generativelanguage.googleapis.com`) | Streaming SSE, multimodal (image+text) |
| **Visual Search** | Google Lens (`lens.google.com/v3/upload`) | Direct HTTP upload, opens results in browser |
| **OCR** | `tesseract` CLI | Already installed, offline, fast |
| **HTTP Client** | `reqwest` with `stream` feature | Async, streaming support for SSE |
| **Config Storage** | `directories` + `serde_json` | XDG-compliant config path |
| **Clipboard** | `arboard` | Cross-platform clipboard (Wayland via wl-copy) |

---

## 4. File Structure

```
~/circle-to-gemini/
├── Cargo.toml              #  Created & tuned (release LTO, stripped binary)
├── assets/
│   └── circle-to-gemini.svg#  Created (SVG app & tray icon)
├── src/
│   ├── main.rs             #  Created (IPC trigger, single-instance socket, tray spawn)
│   ├── app.rs              #  Created (Core iced daemon, multi-window, streaming card)
│   ├── capture.rs          #  Created (KDE Spectacle Wayland capture)
│   ├── config.rs           #  Created (API key, model, persistence)
│   ├── gemini.rs           #  Created (streaming SSE API client)
│   ├── lens.rs             #  Created (Google Lens upload + browser open)
│   ├── overlay.rs          #  Created (canvas drawing program with Gemini-glow)
│   └── tray.rs             #  Created (KDE StatusNotifierItem via ksni D-Bus)
├── circle-to-gemini.desktop #  Created (Desktop entry with actions)
└── install.sh               #  Created (Build + install + shortcut setup)
```

---

## 5. Detailed Module Specs

### 5.1 `app.rs` — Core Application (TODO) ⬜

This is the central iced daemon that orchestrates everything.

**State:**
```rust
struct App {
    windows: BTreeMap<window::Id, WindowKind>,
    overlay_state: OverlayState,
    screenshot_path: Option<String>,
    cropped_path: Option<String>,
    config: Config,
    // Assistant card state
    prompt_input: String,
    response_text: String,
    is_loading: bool,
    // Channel from tray
    tray_rx: Option<mpsc::UnboundedReceiver<TrayEvent>>,
}

enum WindowKind {
    Overlay,
    Assistant,
}
```

**Message enum:**
```rust
enum Message {
    // Tray / activation
    TrayActivate,
    // Capture
    ScreenCaptured(Result<String, String>),
    // Overlay drawing
    OverlayMsg(OverlayMessage),
    SelectionDone(SelectionRect),
    CancelOverlay,
    // Assistant card
    ActionExplain,
    ActionCopyText,
    ActionLens,
    ActionDebug,
    PromptChanged(String),
    SendPrompt,
    // Gemini streaming
    GeminiChunk(GeminiChunk),
    // Config
    OpenSettings,
    ApiKeyChanged(String),
    SaveSettings,
    // Window management
    WindowClosed(window::Id),
    // Clipboard
    CopyResponse,
}
```

**Flow:**
1. `new()` → no windows, just start subscription listening for tray events
2. `TrayActivate` → spawn tokio task to run `capture::capture_current_monitor()`, send `ScreenCaptured`
3. `ScreenCaptured(Ok(path))` → store path, open fullscreen overlay window
4. Overlay canvas draws on screenshot, sends `OverlayMsg` → update `overlay_state`
5. `SelectionDone` → crop screenshot image to selection bounds, save to temp file, close overlay window, open assistant card window
6. Assistant card shows cropped preview + action buttons + prompt input + response area
7. Action buttons / Send → spawn Gemini streaming task, receive `GeminiChunk` messages → append to `response_text`
8. `WindowClosed` → clean up, return to tray-only mode

### 5.2 `tray.rs` — System Tray (TODO) ⬜

**Approach:** Spawn a background thread that creates a `tray-icon` TrayIcon with the `ksni` backend. Use an `mpsc` channel to send `TrayActivate` events to the iced app.

**Menu items:**
- "Capture Screen" → sends TrayActivate
- "Settings" → sends OpenSettings
- "Quit" → sends Quit

**Icon:** A simple 32×32 RGBA icon (Gemini sparkle  or a circle-with-crosshair).

### 5.3 `overlay.rs` — Drawing Canvas (DONE )

Already implemented with:
- Rectangle selection (default)
- Freehand lasso mode
- Glowing Gemini-blue selection borders
- Corner handles for rectangle
- Dimmed overlay background
- Crosshair cursor
- Esc/Right-click to cancel

### 5.4 `capture.rs` — Screenshot (DONE )

- `capture_current_monitor()` → `spectacle -m -b -n -o`
- `capture_full_desktop()` → `spectacle -f -b -n -o`

### 5.5 `gemini.rs` — Gemini API (DONE )

- `stream_gemini_response()` — SSE streaming with channel
- `ask_gemini()` — non-streaming fallback
- Supports image + text multimodal prompts
- Configurable model name

### 5.6 `lens.rs` — Google Lens (DONE )

- `upload_to_lens()` — POST multipart to `lens.google.com/v3/upload`
- `search_with_lens()` — upload + `xdg-open` the result URL

### 5.7 `config.rs` — Configuration (DONE )

- XDG-compliant path: `~/.config/ctg/circle-to-gemini/config.json`
- Stores: `gemini_api_key`, `model` (default: `gemini-3.8-flash`)
- Load/save/has_api_key

---

## 6. Build & Install Plan

### 6.1 Build
```bash
cd ~/circle-to-gemini
cargo build --release
```

### 6.2 Install
```bash
# Binary
cp target/release/circle-to-gemini ~/.local/bin/

# Desktop entry
cp circle-to-gemini.desktop ~/.local/share/applications/

# KDE global shortcut (Meta+G)
# Use kwriteconfig6 or KDE System Settings
```

### 6.3 Desktop Entry (`circle-to-gemini.desktop`)
```ini
[Desktop Entry]
Name=Circle to Gemini
Comment=Select anything on screen and ask Gemini AI about it
Exec=circle-to-gemini
Icon=circle-to-gemini
Type=Application
Categories=Utility;
Keywords=gemini;ai;screenshot;search;circle;
StartupNotify=false
```

### 6.4 KDE Shortcut Setup
```bash
# Register Meta+S as a global shortcut to launch the capture
kwriteconfig6 --file kglobalshortcutsrc \
  --group "circle-to-gemini.desktop" \
  --key "_launch" "Meta+S,none,Circle to Gemini"
```

---

## 7. Status & Verification

| # | Task | Status | Details |
|---|------|--------|---------|
| 1 | **`tray.rs`** |  DONE | Native KDE StatusNotifierItem via pure D-Bus with transparent 4-pointed Gemini star icon |
| 2 | **`app.rs`** |  DONE | Multi-window daemon, overlay canvas, continuous multi-turn chat, native Markdown rendering |
| 3 | **`main.rs`** |  DONE | Unix IPC listener (`/tmp/circle-to-gemini.sock`), single-instance trigger, background tray daemon |
| 4 | **App Icon** |  DONE | Transparent Gemini sparkle star in SVG and full PNG mipmaps (16×16 to 256×256) |
| 5 | **Compile & Build** |  DONE | Built release binary (`15MB`, stripped, optimized thin LTO) |
| 6 | **`install.sh`** |  DONE | Installs binary to `~/.local/bin/`, desktop launcher, autostart, and shortcut `Meta+S` |
| 7 | **`.desktop` file** |  DONE | `~/.local/share/applications/circle-to-gemini.desktop` with `StartupWMClass=circle-to-gemini` |
| 8 | **Lens & OCR** |  DONE | Zero-API-key Google Lens visual search + offline Tesseract OCR |
| 9 | **Gemini Assistant** |  DONE | Multi-turn continuous chat, native Markdown rendering, and model cascade |
| 10 | **Model Hierarchy** |  DONE | Starts with `gemini-3.8-flash` -> auto-fallback to `3.6-flash` -> `3.5-flash-lite` -> `3.1-flash-lite` on 503/429 |
| 11 | **Continue in Web** |  DONE | Two-step clipboard workflow: copies image for browser paste, then transcript for follow-up query |

---

## 8. Key Technical Decisions & Notes

1. **iced 0.13:** Native Wayland client with `iced::daemon` and built-in `markdown` feature (`pulldown-cmark`).
2. **KDE Plasma 6 Wayland Screenshot:** Uses `spectacle -m -b -n -o` for silent screen capture without permission dialogs.
3. **Tray without GTK:** Pure D-Bus implementation via `ksni` StatusNotifierItem with embedded transparent RGBA icon.
4. **Single-Instance IPC:** When `circle-to-gemini --trigger` is invoked by a hotkey, it communicates with the running daemon over `/tmp/circle-to-gemini.sock` in <5ms.
5. **No Emojis in UI:** Clean text labels to prevent font fallback rectangle ("tofu") boxes on Linux.
6. **Continuous Multi-Turn Chat:** Maintains conversation memory in RAM for the session; previous turns and image context sent with follow-ups. No conversations written to disk.
7. **Model Resilience:** Dynamic error-catching for 503 (high demand) and 429 (rate limits) cascading from frontier models down to high-throughput flash-lite models.
8. **No API key requirement for search/OCR:** Google Lens and local OCR work immediately without any setup.

---

## 9. Revision Log

| Date | Change |
|------|--------|
| 2026-11-13 | Initial architecture plan created. |
| 2026-11-14 | All modules implemented in Rust, release binary compiled (15MB), installed, tested, and integrated with KDE Plasma 6. |
| 2026-11-14 | Updated global shortcut to `Meta+S`. Replaced tray and window icons with official transparent Gemini star. Removed emojis to fix tofu rendering bugs. |
| 2026-11-14 | Implemented full multi-turn chat history. Added native Markdown parsing and rendering via `iced::widget::markdown`. |
| 2026-11-14 | Implemented model cascading order (`gemini-3.8-flash` -> `3.6-flash` -> `3.5-flash-lite` -> `3.1-flash-lite`). |
| 2026-11-14 | Added 2-step clipboard workflow for continuing in Gemini Web. Streamlined quick action pills (removed Debug/Translate). |
