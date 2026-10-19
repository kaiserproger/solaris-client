#!/usr/bin/env bash
# Install (or remove via --uninstall) the Solaris Launcher freedesktop entry.
# Expects to run from the unpacked release dir next to solaris-launcher,
# solaris-launcher.desktop and solaris-launcher.png.
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_NAME="solaris-launcher"
DESKTOP_SRC="$APP_DIR/solaris-launcher.desktop"
ICON_SRC="$APP_DIR/solaris-launcher.png"
BIN="$APP_DIR/solaris-launcher"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
APPS_DIR="$DATA_HOME/applications"
ICONS_DIR="$DATA_HOME/icons/hicolor/256x256/apps"

refresh_db() {
  if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$APPS_DIR" || true
  fi
}

if [[ "${1:-}" == "--uninstall" ]]; then
  rm -f "$APPS_DIR/$APP_NAME.desktop" "$ICONS_DIR/$APP_NAME.png"
  refresh_db
  echo "Uninstalled $APP_NAME desktop entry."
  exit 0
fi

for required in "$DESKTOP_SRC" "$ICON_SRC"; do
  if [[ ! -f "$required" ]]; then
    echo "install-desktop: missing $required (run from the unpacked release dir)" >&2
    exit 1
  fi
done
if [[ ! -x "$BIN" ]]; then
  echo "install-desktop: warning: $BIN is not executable" >&2
fi

mkdir -p "$APPS_DIR" "$ICONS_DIR"
ESC_BIN="${BIN//&/\\&}"
sed "s|^Exec=.*|Exec=${ESC_BIN}|" "$DESKTOP_SRC" > "$APPS_DIR/$APP_NAME.desktop"
cp "$ICON_SRC" "$ICONS_DIR/$APP_NAME.png"
refresh_db
echo "Installed $APPS_DIR/$APP_NAME.desktop (Exec=$BIN)"
echo "Installed $ICONS_DIR/$APP_NAME.png"
