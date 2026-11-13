#!/usr/bin/env bash
set -e

DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DIR"

echo "=========================================="
echo "  Installing Circle to Gemini for Linux   "
echo "=========================================="

# 1. Build release binary if not present
if [ ! -f "$DIR/target/release/circle-to-gemini" ]; then
    echo "🔨 Building release binary..."
    cargo build --release
fi

# 2. Install binary to ~/.local/bin
mkdir -p "$HOME/.local/bin"
cp -f "$DIR/target/release/circle-to-gemini" "$HOME/.local/bin/circle-to-gemini"
chmod +x "$HOME/.local/bin/circle-to-gemini"
echo "✓ Installed binary to ~/.local/bin/circle-to-gemini"

# 3. Install application icon
mkdir -p "$HOME/.local/share/icons/hicolor/scalable/apps"
mkdir -p "$HOME/.local/share/icons/hicolor/32x32/apps"
mkdir -p "$HOME/.local/share/icons/hicolor/128x128/apps"
if [ -f "$DIR/assets/circle-to-gemini.svg" ]; then
    cp -f "$DIR/assets/circle-to-gemini.svg" "$HOME/.local/share/icons/hicolor/scalable/apps/circle-to-gemini.svg"
fi
if [ -f "$DIR/assets/gemini-icon-32.png" ]; then
    cp -f "$DIR/assets/gemini-icon-32.png" "$HOME/.local/share/icons/hicolor/32x32/apps/circle-to-gemini.png"
fi
if [ -f "$DIR/assets/gemini-icon-128.png" ]; then
    cp -f "$DIR/assets/gemini-icon-128.png" "$HOME/.local/share/icons/hicolor/128x128/apps/circle-to-gemini.png"
fi
echo "✓ Installed icons"

# 4. Install desktop entry
mkdir -p "$HOME/.local/share/applications"
cat << 'EOF' > "$HOME/.local/share/applications/circle-to-gemini.desktop"
[Desktop Entry]
Name=Circle to Gemini
GenericName=AI Screen Assistant
Comment=Select any area on screen to ask Gemini AI or search with Google Lens
Exec=circle-to-gemini --trigger
Icon=circle-to-gemini
Terminal=false
Type=Application
Categories=Utility;Graphics;Development;
Keywords=gemini;ai;screenshot;lens;circle;search;
StartupNotify=false
StartupWMClass=circle-to-gemini
Actions=Capture;

[Desktop Action Capture]
Name=Capture & Ask Gemini
Exec=circle-to-gemini --trigger
EOF
chmod +x "$HOME/.local/share/applications/circle-to-gemini.desktop"
echo "✓ Installed desktop entry"

# 5. Enable autostart on login
mkdir -p "$HOME/.config/autostart"
cat << 'EOF' > "$HOME/.config/autostart/circle-to-gemini.desktop"
[Desktop Entry]
Name=Circle to Gemini
Comment=AI Screen Assistant Daemon
Exec=circle-to-gemini
Icon=circle-to-gemini
Terminal=false
Type=Application
Categories=Utility;
X-GNOME-Autostart-enabled=true
EOF
echo "✓ Configured autostart"

# 6. Install KWin compositor script for window position memory
if [ -d "$DIR/kwin-script" ]; then
    mkdir -p "$HOME/.local/share/kwin/scripts/remember-assistant"
    cp -rf "$DIR/kwin-script/"* "$HOME/.local/share/kwin/scripts/remember-assistant/"
    if which kwriteconfig6 >/dev/null 2>&1; then
        kwriteconfig6 --file kwinrc --group Plugins --key remember-assistantEnabled true || true
    fi
    echo "✓ Installed KWin position memory script"
fi

# 7. Configure KWin window rules
if which kwriteconfig6 >/dev/null 2>&1; then
    kwriteconfig6 --file kwinrulesrc --group General --key Order "circle-to-gemini-overlay,remember-circle-to-gemini-assistant" || true
    
    # Overlay fullscreen rule
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key Description "Circle to Gemini Selection Overlay" || true
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key wmclass "circle-to-gemini-overlay" || true
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key wmclassmatch 1 || true
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key types 1 || true
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key fullscreen true || true
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key fullscreenrule 2 || true
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key noborder true || true
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key noborderrule 2 || true
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key above true || true
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key aboverule 2 || true
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key positionrule 1 || true
    kwriteconfig6 --file kwinrulesrc --group "circle-to-gemini-overlay" --key sizerule 1 || true

    # Assistant remember rule
    kwriteconfig6 --file kwinrulesrc --group "remember-circle-to-gemini-assistant" --key Description "Remember Circle to Gemini Assistant Position" || true
    kwriteconfig6 --file kwinrulesrc --group "remember-circle-to-gemini-assistant" --key wmclass "circle-to-gemini-assistant" || true
    kwriteconfig6 --file kwinrulesrc --group "remember-circle-to-gemini-assistant" --key wmclassmatch 1 || true
    kwriteconfig6 --file kwinrulesrc --group "remember-circle-to-gemini-assistant" --key position "500,200" || true
    kwriteconfig6 --file kwinrulesrc --group "remember-circle-to-gemini-assistant" --key positionrule 4 || true
    kwriteconfig6 --file kwinrulesrc --group "remember-circle-to-gemini-assistant" --key size "560,720" || true
    kwriteconfig6 --file kwinrulesrc --group "remember-circle-to-gemini-assistant" --key sizerule 4 || true

    gdbus call --session --dest org.kde.KWin --object-path /KWin --method org.kde.KWin.reconfigure >/dev/null 2>&1 || true
    echo "✓ Configured KWin window management rules"
fi

# 8. Register KDE global shortcut: Meta+S
if which gdbus >/dev/null 2>&1; then
    gdbus call --session --dest org.kde.kglobalaccel --object-path /kglobalaccel \
        --method org.kde.KGlobalAccel.setForeignShortcutKeys \
        "['circle-to-gemini.desktop', 'Capture', 'Circle to Gemini', 'Capture & Ask Gemini']" \
        "[([268435539],)]" >/dev/null 2>&1 || true

    gdbus call --session --dest org.kde.kglobalaccel --object-path /kglobalaccel \
        --method org.kde.KGlobalAccel.setForeignShortcutKeys \
        "['circle-to-gemini.desktop', '_launch', 'Circle to Gemini', 'Circle to Gemini']" \
        "[([268435539],)]" >/dev/null 2>&1 || true

    echo "✓ Registered global shortcut: Meta+S (Win+S)"
fi

if which kbuildsycoca6 >/dev/null 2>&1; then
    kbuildsycoca6 >/dev/null 2>&1 || true
fi

echo ""
echo "🎉 Circle to Gemini installed successfully!"
echo "Trigger anytime with Meta+S (Win+S) or click the Gemini star icon in your tray."
