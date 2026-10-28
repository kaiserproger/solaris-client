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

# The Desktop Entry specification forbids '=' in the executable path, even quoted.
if [[ "$BIN" == *=* ]]; then
  echo "install-desktop: executable path contains '=' (not representable in Desktop Entry Exec): $BIN" >&2
  exit 1
fi

mkdir -p "$APPS_DIR" "$ICONS_DIR"
# Exec is a desktop-entry string, not a shell command. First quote the
# executable and escape Exec's special characters, then apply string escaping.
escaped_bin=""
for ((i = 0; i < ${#BIN}; i++)); do
  char=${BIN:i:1}
  case "$char" in
    '\') escaped_bin+='\\\\' ;;
    '"'|'$'|'`') escaped_bin+="\\\\$char" ;;
    '%') escaped_bin+='%%' ;;
    $'\n') escaped_bin+='\n' ;;
    $'\r') escaped_bin+='\r' ;;
    $'\t') escaped_bin+='\t' ;;
    *) escaped_bin+="$char" ;;
  esac
done
while IFS= read -r line || [[ -n "$line" ]]; do
  if [[ "$line" == Exec=* ]]; then
    printf 'Exec="%s"\n' "$escaped_bin"
  else
    printf '%s\n' "$line"
  fi
done < "$DESKTOP_SRC" > "$APPS_DIR/$APP_NAME.desktop"
cp "$ICON_SRC" "$ICONS_DIR/$APP_NAME.png"
refresh_db
echo "Installed $APPS_DIR/$APP_NAME.desktop (Exec=$BIN)"
echo "Installed $ICONS_DIR/$APP_NAME.png"
