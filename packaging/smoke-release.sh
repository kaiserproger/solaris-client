#!/usr/bin/env bash
# Startup smoke for a packaged client binary on a machine without the build
# checkout: the client must publish its automation descriptor under the data
# root of a fresh HOME. Content is not required; the client may fail on
# missing content after the descriptor line, which is fine here.
#   smoke-release.sh PATH/TO/solaris-client
set -euo pipefail

client="${1:?usage: smoke-release.sh PATH/TO/solaris-client}"
home="$(mktemp -d)"
trap 'rm -rf "$home"' EXIT
root="$home/.local/share/solaris-client"

coproc CLIENT {
  env -u XDG_DATA_HOME HOME="$home" "$client" play --headless --profile fixture --automation 2>&1
}
pid=$CLIENT_PID
descriptor=""
while IFS= read -r -t 60 line <&"${CLIENT[0]}"; do
  printf '%s\n' "$line"
  if [[ $line == "automation descriptor: "* ]]; then
    descriptor="${line#automation descriptor: }"
    break
  fi
done
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true

if [[ $descriptor != "$root/runtime/"*.json ]]; then
  echo "smoke: no automation descriptor under $root/runtime" >&2
  exit 1
fi
echo "smoke: ok ($descriptor)"
