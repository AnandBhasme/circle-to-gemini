# Circle to Gemini 🔍✨

[![Rust](https://img.shields.io/badge/built_with-Rust-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20KDE%20Plasma%206%20%7C%20Wayland-blue.svg)](https://kde.org/plasma-desktop/)
[![License](https://img.shields.io/badge/license-MIT-green.svg)](LICENSE)

A native, lightweight Linux counterpart to Android's **"Circle to Search"** and Google Gemini screen assistant. Built entirely in **Rust** using `iced 0.13`, Tokio, and D-Bus.

Select any region, image, formula, code error, or UI element on your screen to instantly analyze it with Google Gemini, extract text with local OCR, or search with Google Lens.

---

## ✨ Features

- **⚡ Instant Screen Freeze & Selection:** Freeze your display on demand and drag a glowing neon box around any content.
- **🧠 Multimodal Gemini AI:** Ask questions about diagrams, math problems, code snippets, or error dialogs with real-time streaming answers.
- **💬 Continuous Multi-Turn Chat:** Follow up with context-aware conversational questions without losing your selection.
- **🎛️ Smart vs. Fast Mode Switcher:**
  - **Smart:** Full reasoning cascade (`gemini-3.8-flash` $\rightarrow$ `gemini-3.6-flash` $\rightarrow$ `gemini-3.5-flash-lite` $\rightarrow$ `gemini-3.1-flash-lite`) with automatic failover recovery on `503 Service Unavailable` spikes.
  - **Fast:** Ultra-low latency processing (`gemini-3.5-flash-lite` $\rightarrow$ `gemini-3.1-flash-lite`).
  - Automatically remembers your last selected mode.
- **📐 Mathematical Formula Sanitization:** Pre-processes raw LaTeX expressions (`$x_i$`, `$\sigma_i$`, `\forall`, `\le`, `$$...$$`) into clean, readable Unicode (`xᵢ`, `σᵢ`, `∀`, `≤`) and formatted blocks.
- **📍 Persistent Window Position:** Remembers where you move and resize the Assistant Card on Wayland using KWin compositor integration.
- **🌐 Open in Gemini Web:** 2-step clipboard pipeline copies your image attachment, opens `gemini.google.com`, and synchronizes your chat transcript.
- **🔍 Google Lens:** Jump directly to reverse visual search in your browser with one click (no API key required).
- **📝 Offline OCR:** Instant local text extraction to clipboard via `tesseract` (zero network dependency).
- **🔒 Privacy First & RAM-Only:** Zero chat logs or user snippets are stored on disk. All conversational memory lives strictly in volatile RAM.
- **🚀 Native & Lightweight:** Single compiled Rust binary (~15 MB) with sub-millisecond IPC invocation.

---

## 📦 Requirements

- **Operating System:** Linux with **KDE Plasma 6 (Wayland)**
- **Dependencies:**
  - `spectacle` (KDE Wayland screenshot tool)
  - `wl-clipboard` (`wl-copy` for Wayland clipboard support)
  - `tesseract` (optional, for local offline OCR)
  - `rust` and `cargo` (for compilation)

On Arch Linux / CachyOS:
```bash
sudo pacman -S spectacle wl-clipboard tesseract tesseract-data-eng rust cargo
```

---

## 🚀 Installation

1. **Clone the repository:**
   ```bash
   git clone https://github.com/AnandBhasme/circle-to-gemini.git
   cd circle-to-gemini
   ```

2. **Run the installation script:**
   ```bash
   ./install.sh
   ```

   The installer will:
   - Compile the optimized release binary.
   - Install the executable to `~/.local/bin/circle-to-gemini`.
   - Register desktop entries and transparent Gemini star icons.
   - Configure autostart on user login.
   - Install the KWin compositor script and window rules for position memory.
   - Register the global shortcut **`Meta+S`** (<kbd>Windows Key</kbd> + <kbd>S</kbd>) in KDE Plasma.

3. **Start the background daemon:**
   ```bash
   circle-to-gemini &
   ```
   *(Or log out and log back in to let the autostart launcher handle it).*

---

## ⌨️ How to Use

| Action | How to Trigger |
|---|---|
| **Global Shortcut** | Press <kbd>Meta</kbd> + <kbd>S</kbd> (<kbd>Win</kbd> + <kbd>S</kbd>) |
| **System Tray** | Click the Gemini star icon in your KDE System Tray |
| **Terminal / CLI** | Run `circle-to-gemini --trigger` |

1. **Freeze & Select:** When triggered, your display freezes. Click and drag to draw a box around any region (press <kbd>Esc</kbd> to cancel).
2. **Assistant Card:** The floating assistant card pops up with your cropped snippet preview.
3. **Quick Actions:**
   - **Explain:** Get a comprehensive multimodal breakdown.
   - **Copy Text:** Extract text using offline OCR directly to your clipboard.
   - **Google Lens:** Reverse image search without needing an API key.
   - **Open in Gemini Web:** Push image and chat transcript to the browser.
4. **Follow-up:** Type any question in the prompt bar to chat with Gemini.

---

## ⚙️ Configuration

- Click **Settings** in the top bar of the Assistant Card to enter your free Gemini API key.
- You can get a free key at [Google AI Studio](https://aistudio.google.com/app/apikey).
- Configuration is saved in `~/.config/ctg/circle-to-gemini/config.json`.

---

## 🏗️ Architecture

See [ARCHITECTURE.md](ARCHITECTURE.md) for full architectural specifications, including IPC socket lifecycle, KWin scripting hooks, and model cascade recovery logic.

---

## 🗑️ Uninstallation

To remove all installed files, shortcuts, and configurations:
```bash
./uninstall.sh
```

---

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.
