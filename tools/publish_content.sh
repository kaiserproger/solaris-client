#!/usr/bin/env bash
# Build the Solaris singleplayer content bundle and merge its entry into the
# content manifest for the static file server.
#
# The bundle is the complete vanilla content cache that
# `solaris-v2 content import` derives (version.json, data/, reports/). The
# launcher's `--singleplayer-runtime` installs it as the local server's
# `vanilla_data_dir`, so players need no JDK, no Mojang server jar and no
# Java extractors. Owner decision 2026-10-28: this derived data is published
# from the Solaris CDN. Mojang textures/models/fonts/sounds are NOT part of it:
# they still come from Mojang's own CDN (client jar + asset index).
#
# Tar top-level layout:
#   bundle.json
#   content/version.json, content/data/, content/reports/, and any supported
#   root sidecar (currently gameplay_version.json)
# Before packing, the staged tree is validated by the real server with
# `solaris-v2 content verify`, so a cache the server would refuse at startup is
# never published. The release is the launcher's GAME_VERSION
# (launcher/src/paths.rs); the cache's version.json must match it.
#
# The manifest entry (kind "content") is merged into
# $OUT_DIR/solaris-content.json, replacing only an entry with the same name, so
# the facts bundle and older pinned revisions stay listed.
#
# Usage: CONTENT=<cache> SOLARIS_V2=<server binary> tools/publish_content.sh [REV]
#   REV         bundle revision (default 1)
#   CONTENT     content cache to pack (e.g. <solaris>/data/vanilla)
#   SOLARIS_V2  solaris-v2 built for the same release, used for `content verify`
#   OUT_DIR     output dir (default <repo>/target/publish)
set -euo pipefail

REV="${1:-1}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(dirname "$SCRIPT_DIR")"
CONTENT="${CONTENT:?set CONTENT to the content cache to pack}"
SOLARIS_V2="${SOLARIS_V2:?set SOLARIS_V2 to the solaris-v2 binary for this release}"
OUT_DIR="${OUT_DIR:-$REPO/target/publish}"
GAME_VERSION="$(awk -F'"' '/pub const GAME_VERSION: &str/ { print $2; exit }' "$REPO/launcher/src/paths.rs")"
if [[ ! "$GAME_VERSION" =~ ^[0-9]+(\.[0-9]+)+$ ]]; then
  echo "publish_content: launcher GAME_VERSION is not a release version: $GAME_VERSION" >&2
  exit 1
fi
NAME="solaris-content-${GAME_VERSION}-r${REV}"
BASE_URL="http://135.106.208.217:8080/static"
ENTRIES=(version.json data reports)
OPTIONAL_ROOT_FILES=(gameplay_version.json)

for entry in "${ENTRIES[@]}"; do
  if [[ ! -e "$CONTENT/$entry" ]]; then
    echo "publish_content: missing required input: $CONTENT/$entry" >&2
    exit 1
  fi
done
CACHE_VERSION="$(jq -r .id "$CONTENT/version.json")"
if [[ "$CACHE_VERSION" != "$GAME_VERSION" ]]; then
  echo "publish_content: $CONTENT is for $CACHE_VERSION, the launcher targets $GAME_VERSION" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# 1. Stage exactly the cache entries (no README or other local files).
mkdir -p "$STAGE/content"
for entry in "${ENTRIES[@]}"; do
  cp -RL "$CONTENT/$entry" "$STAGE/content/$entry"
done
for entry in "${OPTIONAL_ROOT_FILES[@]}"; do
  if [[ -f "$CONTENT/$entry" ]]; then
    cp -L "$CONTENT/$entry" "$STAGE/content/$entry"
  fi
done

# 2. The server must accept the staged tree as its startup cache.
(cd "$STAGE" && "$SOLARIS_V2" content verify --cache "$STAGE/content")

printf '{"name":"%s","kind":"content","game_version":"%s","rev":%s}\n' \
  "$NAME" "$GAME_VERSION" "$REV" > "$STAGE/bundle.json"

# 3. Per-file audit list (sha256 + path).
LIST="$OUT_DIR/$NAME.files.txt"
(cd "$STAGE" && find bundle.json content -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum) > "$LIST"

# 4. Deterministic archive of the staged tree.
OUT="$OUT_DIR/$NAME.tar.zst"
tar --sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner --mode='u+rwX,go+rX,go-w' \
  --use-compress-program='zstd -19 -T0' \
  -cf "$OUT" \
  -C "$STAGE" bundle.json content

SHA="$(sha256sum "$OUT" | awk '{print $1}')"
SIZE="$(stat -c%s "$OUT")"

# 5. Merge the entry into the manifest, keeping every other bundle.
JSON="$OUT_DIR/solaris-content.json"
if [[ -f "$JSON" ]]; then
  BASE="$(cat "$JSON")"
else
  BASE="{\"schema\":1,\"game_version\":\"$GAME_VERSION\",\"bundles\":[]}"
fi
jq --arg version "$GAME_VERSION" \
  --argjson entry "{\"name\":\"$NAME\",\"kind\":\"content\",\"url\":\"$BASE_URL/$NAME.tar.zst\",\"sha256\":\"$SHA\",\"size\":$SIZE}" \
  'if .game_version != $version
   then error("manifest is for \(.game_version), bundle is for \($version)")
   else .bundles = ([.bundles[] | select(.name != $entry.name)] + [$entry]) end' \
  <<<"$BASE" > "$JSON.tmp"
mv "$JSON.tmp" "$JSON"

echo "$OUT"
echo "$JSON"
echo "$LIST"
echo "launcher/src/pins.rs:"
echo "  pub const CONTENT_NAME: &str = \"$NAME\";"
echo "  pub const CONTENT_SHA256: &str = \"$SHA\";"
echo "Publication pending approval: $OUT and $JSON"
