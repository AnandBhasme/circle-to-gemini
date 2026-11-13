# Circle to Gemini 🌟

> **A native, blazingly fast Linux desktop equivalent of Android's "Circle to Search" and Google Gemini screen assistant.**
> Built for KDE Plasma 6 (Wayland) using **Rust**, `iced 0.13`, and the Google Gemini Multimodal API.

[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform: Linux](https://img.shields.io/badge/platform-Linux%20(KDE%20Plasma%206%20%2F%20Wayland)-blueviolet.svg)]()

---

## ✨ Features

- ⚡ **Instant Invocation (<5ms):** Runs as a lightweight resident background daemon in Rust (~15MB compiled). Summon anytime via **`Meta + S`** (`Windows Key + S`) or the transparent Gemini star icon in your KDE System Tray.
- 📸 **Wayland Freeze-Frame Capture:** Silently captures your active screen using native Wayland tools and displays a frozen desktop canvas so you can comfortably select moving content, video frames, or dynamic UIs.
- ✏️ **Interactive Screen Canvas:** Drag a glowing neon rectangle or lasso over any text, error message, UI design, chart, or image on your screen. Press `Esc` at any time to cancel.
- 🤖 **Multimodal Gemini AI:** Stream responses in real-time using Google's latest multimodal models with multi-turn chat memory directly inside a floating Assistant Card.
- 🎚️ **Smart vs. Fast Mode:**
  - **Smart Mode:** Deep reasoning cascade (`gemini-3.8-flash` → `gemini-3.6-flash` → `gemini-3.5-flash-lite` → `gemini-3.1-flash-lite`) with automated recovery from `503 Service Unavailable` demand spikes.
  - **Fast Mode:** Low-latency quick queries (`gemini-3.5-flash-lite` → `gemini-3.1-flash-lite`).
  - Remembers your selected mode across reboots.
- 📐 **Clean Math & Formula Rendering:** Built-in LaTeX-to-Unicode sanitizer converts formulas and variables (e.g. `$x_i$`, `$\sigma_i$`, `\forall`, `\le`) into clean Unicode characters (`xᵢ`, `σᵢ`, `∀`, `≤`) and formats display equations (`$$...$$`) into code blocks.
- 🪟 **Wayland Window Position Memory:** Uses a native KWin compositor script and window rules so your Assistant Card always remembers where you placed it and re-opens at that exact screen position.
- 🔍 **Google Lens Integration:** Instant 1-click visual reverse image search in your default web browser (works without an API key).
- 📝 **Offline OCR:** Extract readable text from your selection directly to your clipboard using local `tesseract` (works completely offline with no network or API key).
- 🌐 **Continue in Gemini Web:** Automatic 2-step Wayland clipboard sync:
  1. Copies the screenshot to your clipboard (`image/png`) and opens `gemini.google.com` (press `Ctrl+V` to attach).
  2. Automatically copies your chat transcript to the clipboard after 4 seconds (press `Ctrl+V` again to paste your prompt).
- 🔒 **Privacy First:** Conversations exist strictly in volatile RAM for the duration of the assistant card. When you close the card or restart the app, your history is completely wiped. Nothing is logged or saved to disk.

---

## 📋 Prerequisites

Before installing, ensure your system has the following dependencies:

### Arch Linux / CachyOS / Manjaro:
```bash
sudo pacman -S --needed rust cargo spectacle wl-clipboard tesseract tesseract-data-eng
```

### Fedora:
```bash
sudo dnf install cargo rust spectacle wl-clipboard tesseract tesseract-langpack-eng
```

### Ubuntu / Debian:
```bash
sudo apt update
sudo apt install cargo rustc kde-spectacle wl-clipboard tesseract-ocr
```

---

## 🚀 Quick Installation

Clone the repository and run the automated installer:

```bash
git clone https://github.com/AnandBhasme/circle-to-gemini.git
cd circle-to-gemini
./install.sh
```

### What `install.sh` Does:
1. Compiles the optimized release binary (`~15MB`).
2. Installs the binary to `~/.local/bin/circle-to-gemini`.
3. Installs high-resolution app icons into `~/.local/share/icons/`.
4. Installs the `.desktop` launcher and enables autostart on login.
5. Installs the KWin position memory script into `~/.local/share/kwin/scripts/`.
6. Configures KWin window management rules in `~/.config/kwinrulesrc`.
7. Registers the global shortcut **`Meta + S`** in KDE's `kglobalaccel` over D-Bus.

---

## 🎮 How to Use

1. **Start the daemon (if not already running):**
   ```bash
   circle-to-gemini &
   ```
   *(The installer sets this up to start automatically on login.)*

2. **Trigger the Screen Overlay:**
   - Press **`Meta + S`** (<kbd>Windows Key</kbd> + <kbd>S</kbd>), OR
   - Click the Gemini star icon in your KDE System Tray, OR
   - Run `circle-to-gemini --trigger` from any terminal or custom keybinding.

3. **Select Any Region:**
   - Click and drag across any area of your screen to capture it.
   - Press <kbd>Esc</kbd> to dismiss.

4. **Ask Gemini or Take Action:**
   - **Explain:** Get a comprehensive multimodal breakdown.
   - **Copy Text:** Extract text straight to your clipboard via offline OCR.
   - **Google Lens:** Search the visual web with Google Lens.
   - **Open in Gemini Web:** Continue your conversation on the official web app.
   - **Follow-up Chat:** Type any conversational question in the prompt input bar.

---

## ⚙️ Configuration & API Key

You can configure your free Gemini API key directly inside the app:
1. Click the **Settings** button in the top bar of the Assistant Card.
2. Enter your Gemini API key (get a free key at [Google AI Studio](https://aistudio.google.com/app/apikey)).
3. Click **Save**.

Settings are persisted in `~/.config/ctg/circle-to-gemini/config.json`.

---

## 🏗️ Architecture Overview

For in-depth technical documentation, see [ARCHITECTURE.md](ARCHITECTURE.md).

```
[Meta+S or Tray] ──> [Unix IPC Socket /tmp/circle-to-gemini.sock]
                           │
                           ▼
          [Wayland Freeze Capture (KDE Spectacle)]
                           │
                           ▼
       [Fullscreen Overlay (circle-to-gemini-overlay)]
                           │  (Drag to select)
                           ▼
       [Assistant Card (circle-to-gemini-assistant)]
          ├── Mode Selector (Smart vs. Fast)
          ├── Gemini SSE Streaming Client
          ├── LaTeX Math-to-Unicode Sanitizer
          ├── KWin Geometry Memory Script
          └── 2-Step Browser Clipboard Sync
```

---

## 🗑️ Uninstallation

To cleanly remove Circle to Gemini, its desktop entries, KWin scripts, and shortcuts:

```bash
./uninstall.sh
```

---

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
