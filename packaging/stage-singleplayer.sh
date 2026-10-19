#!/usr/bin/env bash
# Stage the singleplayer server and plugin pack beside the client binaries.
#   stage-singleplayer.sh DEST_DIR [EXE_SUFFIX]
# Expects the pinned checkouts `solaris-server/` (built in release mode) and
# `solaris-default-plugins/` in the current directory (see release.yml).
# The client drops `solaris-auth` from the pack at runtime: singleplayer is
# offline and the local player is the operator.
set -euo pipefail

dest="${1:?usage: stage-singleplayer.sh DEST_DIR [EXE_SUFFIX]}"
suffix="${2:-}"
packages="basic-economy solaris-audit solaris-auth solaris-economy solaris-essentials solaris-permissions solaris-towns"

install -m 0755 "solaris-server/target/release/solaris-v2${suffix}" "$dest/solaris-v2${suffix}"
for pkg in $packages; do
  src="solaris-default-plugins/$pkg"
  mkdir -p "$dest/plugins/$pkg"
  install -m 0644 "$src/plugin.toml" "$src/plugin.wasm" "$dest/plugins/$pkg/"
  if [ -f "$src/config.toml" ]; then
    install -m 0644 "$src/config.toml" "$dest/plugins/$pkg/"
  fi
done
