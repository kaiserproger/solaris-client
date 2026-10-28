#!/usr/bin/env bash
set -euo pipefail

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
source_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
release="$work/release space \"quote\" \$dollar \`tick\` \\backslash %f | &"
mkdir -p "$release" "$work/data"
cp "$source_dir/install-desktop.sh" "$source_dir/solaris-launcher.desktop" "$release/"
printf '#!/bin/sh\n' > "$release/solaris-launcher"
chmod +x "$release/solaris-launcher"
printf 'icon' > "$release/solaris-launcher.png"
XDG_DATA_HOME="$work/data" bash "$release/install-desktop.sh"
python3 - "$work/data/applications/solaris-launcher.desktop" "$release/solaris-launcher" <<'PY'
import pathlib
import sys

entry = pathlib.Path(sys.argv[1]).read_text()
actual = next(line[5:] for line in entry.splitlines() if line.startswith('Exec='))
# Desktop entry string escaping precedes the Exec quoting and field-code layers.
path = sys.argv[2]
quoted = '"' + ''.join(('\\' + c) if c in '"`$\\' else ('%%' if c == '%' else c) for c in path) + '"'
expected = quoted.replace('\\', '\\\\')
assert actual == expected, (actual, expected)
PY

# '=' cannot be represented by Desktop Entry Exec; reject it before changing
# an existing valid installation rather than emitting a broken shortcut.
unsupported="$work/release=rc1"
mkdir -p "$unsupported"
cp "$release/install-desktop.sh" "$release/solaris-launcher.desktop" "$release/solaris-launcher" "$release/solaris-launcher.png" "$unsupported/"
cp "$work/data/applications/solaris-launcher.desktop" "$work/before.desktop"
if XDG_DATA_HOME="$work/data" bash "$unsupported/install-desktop.sh" >"$work/error.log" 2>&1; then
  echo "expected unrepresentable Exec path to be rejected" >&2
  exit 1
fi
cmp "$work/before.desktop" "$work/data/applications/solaris-launcher.desktop"
python3 - "$work/error.log" <<'PY'
import pathlib
import sys
assert "not representable in Desktop Entry Exec" in pathlib.Path(sys.argv[1]).read_text()
PY
