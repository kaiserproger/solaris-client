#!/usr/bin/env bash
# Build the data-only Solaris facts bundle from <repo>/dist and merge its entry
# into the content manifest for the static file server (replacing only an entry
# with the same name, so the content bundle and older revisions stay listed).
#
# Public-bundle allowlist (owner rule): the bundle may contain only pure
# non-asset data tables. NO entity models/meshes/UV/animations (no entities/
# dir), no datapack data/ tree, no registry_network_nbt, no packets.json.
#
# Tar top-level layout:
#   bundle.json
#   facts/                      exactly the 12 files below (from dist/facts)
#   data/vanilla/reports/       exactly blocks.json, block_mining.json,
#                               block_light.json, registries.json and the whole
#                               minecraft/components/item/ subtree
#                               (from dist/data/vanilla/reports)
#
# Files are copied through a staging dir so the allowlist is explicit: the
# script fails if any allowlisted file is missing, and fails if the staged
# tree contains anything outside the allowlist.
#
# Usage: tools/publish_facts.sh [REV]
#   REV      bundle revision (default 1)
#   DIST     dist dir to pack from (default <repo>/dist)
#   OUT_DIR  output dir (default <repo>/target/publish)
set -euo pipefail

REV="${1:-1}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(dirname "$SCRIPT_DIR")"
DIST="${DIST:-$REPO/dist}"
OUT_DIR="${OUT_DIR:-$REPO/target/publish}"
GAME_VERSION="26.1.2"
NAME="solaris-facts-${GAME_VERSION}-r${REV}"
BASE_URL="http://135.106.208.217:8080/static"

FACTS_FILES=(
  action-state-facts.json
  block-state-predicates.json
  block-use-classes.json
  block_collision_shapes_26_1_2.bin
  creative-facts.json
  entity-use-static.json
  interaction-facts.json
  item-placement-facts.json
  item-properties.json
  item-use-owners.json
  placement-state-facts.json
  terrain-facts.json
)
REPORT_FILES=(
  blocks.json
  block_mining.json
  block_light.json
  registries.json
)
ITEM_SUBTREE="minecraft/components/item"

mkdir -p "$OUT_DIR"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# 1. Copy the allowlist into the staging dir; fail on any missing input.
for file in "${FACTS_FILES[@]}"; do
  src="$DIST/facts/$file"
  if [[ ! -f "$src" ]]; then
    echo "publish_facts: missing required input: $src" >&2
    exit 1
  fi
  install -D -m 0644 "$src" "$STAGE/facts/$file"
done

for file in "${REPORT_FILES[@]}"; do
  src="$DIST/data/vanilla/reports/$file"
  if [[ ! -f "$src" ]]; then
    echo "publish_facts: missing required input: $src" >&2
    exit 1
  fi
  install -D -m 0644 "$src" "$STAGE/data/vanilla/reports/$file"
done

if [[ ! -d "$DIST/data/vanilla/reports/$ITEM_SUBTREE" ]]; then
  echo "publish_facts: missing required input dir: $DIST/data/vanilla/reports/$ITEM_SUBTREE" >&2
  exit 1
fi
mapfile -t ITEM_FILES < <(cd "$DIST/data/vanilla/reports" && find -L "$ITEM_SUBTREE" -type f | LC_ALL=C sort)
if (( ${#ITEM_FILES[@]} == 0 )); then
  echo "publish_facts: no files under $DIST/data/vanilla/reports/$ITEM_SUBTREE" >&2
  exit 1
fi
for file in "${ITEM_FILES[@]}"; do
  install -D -m 0644 "$DIST/data/vanilla/reports/$file" "$STAGE/data/vanilla/reports/$file"
done

printf '{"name":"%s","game_version":"%s","rev":%s}\n' \
  "$NAME" "$GAME_VERSION" "$REV" > "$STAGE/bundle.json"

# 2. Assert the staged tree holds exactly the allowlist: nothing more.
if [[ -e "$STAGE/entities" ]]; then
  echo "publish_facts: staged tree must not contain entities/" >&2
  exit 1
fi
{
  echo "bundle.json"
  for file in "${FACTS_FILES[@]}"; do echo "facts/$file"; done
  for file in "${REPORT_FILES[@]}"; do echo "data/vanilla/reports/$file"; done
  for file in "${ITEM_FILES[@]}"; do echo "data/vanilla/reports/$file"; done
} | LC_ALL=C sort -u > "$STAGE/expected.txt"
(cd "$STAGE" && find bundle.json facts data \( -type f -o -type l \) | LC_ALL=C sort) > "$STAGE/staged.txt"
if ! diff -u "$STAGE/expected.txt" "$STAGE/staged.txt" >&2; then
  echo "publish_facts: staged tree differs from the allowlist" >&2
  exit 1
fi

# 3. Per-file audit list (sha256 + size) to stdout and to the .files.txt.
LIST="$OUT_DIR/$NAME.files.txt"
: > "$LIST"
while IFS= read -r rel; do
  sha="$(sha256sum "$STAGE/$rel" | awk '{print $1}')"
  size="$(stat -c%s "$STAGE/$rel")"
  printf '%s  %s  %s\n' "$sha" "$size" "$rel" >> "$LIST"
done < "$STAGE/staged.txt"
cat "$LIST"

# 4. Deterministic archive of the staged tree.
OUT="$OUT_DIR/$NAME.tar.zst"
tar --sort=name --mtime='@0' --owner=0 --group=0 --numeric-owner \
  --use-compress-program='zstd -19 -T0' \
  -cf "$OUT" \
  -C "$STAGE" bundle.json facts data

SHA="$(sha256sum "$OUT" | awk '{print $1}')"
SIZE="$(stat -c%s "$OUT")"
JSON="$OUT_DIR/solaris-content.json"
if [[ -f "$JSON" ]]; then
  BASE="$(cat "$JSON")"
else
  BASE="{\"schema\":1,\"game_version\":\"$GAME_VERSION\",\"bundles\":[]}"
fi
jq --arg version "$GAME_VERSION" \
  --argjson entry "{\"name\":\"$NAME\",\"kind\":\"facts\",\"url\":\"$BASE_URL/$NAME.tar.zst\",\"sha256\":\"$SHA\",\"size\":$SIZE}" \
  'if .game_version != $version
   then error("manifest is for \(.game_version), bundle is for \($version)")
   else .bundles = ([.bundles[] | select(.name != $entry.name)] + [$entry]) end' \
  <<<"$BASE" > "$JSON.tmp"
mv "$JSON.tmp" "$JSON"

echo "$OUT"
echo "$JSON"
echo "$LIST"
echo "Upload with one of:"
echo "  scp \"$OUT\" \"$JSON\" user@135.106.208.217:/static/"
echo "  rsync -av \"$OUT\" \"$JSON\" user@135.106.208.217:/static/"
