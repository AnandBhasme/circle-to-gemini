#!/usr/bin/env bash
set -e

echo "Uninstalling Circle to Gemini..."

# Stop any running instances
killall -9 circle-to-gemini 2>/dev/null || true
rm -f /tmp/circle-to-gemini.sock 2>/dev/null || true

# Remove binaries and desktop files
rm -f "$HOME/.local/bin/circle-to-gemini"
rm -f "$HOME/.local/share/applications/circle-to-gemini.desktop"
rm -f "$HOME/.config/autostart/circle-to-gemini.desktop"
rm -f "$HOME/.local/share/icons/hicolor/scalable/apps/circle-to-gemini.svg"
rm -f "$HOME/.local/share/icons/hicolor/32x32/apps/circle-to-gemini.png"
rm -f "$HOME/.local/share/icons/hicolor/128x128/apps/circle-to-gemini.png"

# Remove KWin script
rm -rf "$HOME/.local/share/kwin/scripts/remember-assistant"

# Unset shortcut in kglobalaccel
if which gdbus >/dev/null 2>&1; then
    gdbus call --session --dest org.kde.kglobalaccel --object-path /kglobalaccel \
        --method org.kde.KGlobalAccel.setForeignShortcutKeys \
        "['circle-to-gemini.desktop', 'Capture', 'Circle to Gemini', 'Capture & Ask Gemini']" \
        "[]" >/dev/null 2>&1 || true
    gdbus call --session --dest org.kde.kglobalaccel --object-path /kglobalaccel \
        --method org.kde.KGlobalAccel.setForeignShortcutKeys \
        "['circle-to-gemini.desktop', '_launch', 'Circle to Gemini', 'Circle to Gemini']" \
        "[]" >/dev/null 2>&1 || true
fi

if which kbuildsycoca6 >/dev/null 2>&1; then
    kbuildsycoca6 >/dev/null 2>&1 || true
fi

echo "✓ Circle to Gemini uninstalled."
