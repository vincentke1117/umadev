#!/usr/bin/env bash
# Publish the entire npm distribution: five cli-* packages and the knowledge
# corpus first, then the main `umadev` package last.
#
# Assumes:
#   - `stage.sh` has already populated each `npm/cli-<platform>/bin/`
#     with the matching prebuilt binary.
#   - `npm whoami` is logged in with publish rights to the `@umacloud`
#     scope and to the `umadev` name.
#   - All package.json versions are aligned (this script does NOT bump).
#
# Use `--dry-run` to validate without actually publishing.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NPM_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DRY_RUN=""
if [[ "${1:-}" == "--dry-run" ]]; then
  DRY_RUN="--dry-run"
  echo "▶ publish.sh: DRY RUN (nothing will actually publish)"
fi

PLATFORM_PACKAGES=(
  "cli-darwin-arm64"
  "cli-darwin-x64"
  "cli-linux-x64"
  "cli-linux-arm64"
  "cli-win32-x64"
)

node "$SCRIPT_DIR/verify-version-lock.mjs"

# 1) Verify every platform package has its binary staged.
for pkg in "${PLATFORM_PACKAGES[@]}"; do
  case "$pkg" in
    cli-win32-*) bin="umadev.exe" ;;
    *)           bin="umadev" ;;
  esac
  if [[ ! -f "$NPM_ROOT/$pkg/bin/$bin" ]]; then
    echo "publish.sh: missing $NPM_ROOT/$pkg/bin/$bin" >&2
    echo "             run stage.sh for this platform first" >&2
    exit 1
  fi
done

# Pack every package before the first publish. This catches missing files and
# freezes the exact tarballs used by every later retry.
PACK_ROOT="$(mktemp -d)"
trap 'rm -rf "$PACK_ROOT"' EXIT
KNOWLEDGE_SOURCE="$NPM_ROOT/../knowledge"
KNOWLEDGE_STAGE="$PACK_ROOT/knowledge-corpus"
[[ -d "$KNOWLEDGE_SOURCE" ]] || {
  echo "publish.sh: knowledge/ not found" >&2
  exit 1
}
mkdir -p "$KNOWLEDGE_STAGE"
cp "$NPM_ROOT/knowledge-corpus/package.json" "$KNOWLEDGE_STAGE/"
cp -R "$KNOWLEDGE_SOURCE/." "$KNOWLEDGE_STAGE/"

PACKAGE_DIRS=()
for pkg in "${PLATFORM_PACKAGES[@]}"; do
  PACKAGE_DIRS+=("$NPM_ROOT/$pkg")
done
PACKAGE_DIRS+=("$KNOWLEDGE_STAGE" "$NPM_ROOT/umadev")

TARBALLS=()
for dir in "${PACKAGE_DIRS[@]}"; do
  pack_json="$(npm pack "$dir" --pack-destination "$PACK_ROOT" --json)"
  filename="$(node -e '
    const fs = require("node:fs");
    const result = JSON.parse(fs.readFileSync(0, "utf8"));
    if (!Array.isArray(result) || !result[0]?.filename) process.exit(1);
    process.stdout.write(result[0].filename);
  ' <<<"$pack_json")"
  TARBALLS+=("$PACK_ROOT/$filename")
done

integrity_of() {
  node - "$1" <<'NODE'
const crypto = require('node:crypto');
const fs = require('node:fs');
const digest = crypto.createHash('sha512').update(fs.readFileSync(process.argv[2])).digest('base64');
process.stdout.write(`sha512-${digest}`);
NODE
}

remote_integrity() {
  npm view "$1@$2" dist.integrity --json 2>/dev/null | node -e '
    const fs = require("node:fs");
    try {
      const value = JSON.parse(fs.readFileSync(0, "utf8"));
      if (typeof value === "string") process.stdout.write(value);
    } catch (_) {}
  '
}

remote_version() {
  npm view "$1@$2" version --json 2>/dev/null | node -e '
    const fs = require("node:fs");
    try {
      const value = JSON.parse(fs.readFileSync(0, "utf8"));
      if (typeof value === "string") process.stdout.write(value);
    } catch (_) {}
  '
}

# Validate the complete registry state before publishing one package. In
# particular, an old tag rerun must never move `latest` backwards.
if [[ -z "$DRY_RUN" ]]; then
  # Fail before publishing the first exact version when CI has no usable npm
  # identity. Package-level authorization is still decided by the registry, but
  # this catches a missing/expired token without leaving a partial staging set.
  npm whoami >/dev/null
  for tarball in "${TARBALLS[@]}"; do
    manifest="$(tar -xOf "$tarball" package/package.json)"
    name="$(node -p 'JSON.parse(process.argv[1]).name' "$manifest")"
    version="$(node -p 'JSON.parse(process.argv[1]).version' "$manifest")"
    local_integrity="$(integrity_of "$tarball")"
    latest="$(remote_version "$name" latest || true)"
    if [[ -n "$latest" ]] && ! node - "$latest" "$version" <<'NODE'
const parse = (value) => {
  const match = /^(\d+)\.(\d+)\.(\d+)/.exec(value);
  if (!match) process.exit(2);
  return match.slice(1).map(Number);
};
const [latest, target] = process.argv.slice(2).map(parse);
for (let i = 0; i < 3; i += 1) {
  if (latest[i] > target[i]) process.exit(1);
  if (latest[i] < target[i]) process.exit(0);
}
NODE
    then
      echo "publish.sh: refusing to move $name latest backwards ($latest -> $version)" >&2
      exit 1
    fi
    existing="$(remote_integrity "$name" "$version" || true)"
    if [[ -n "$existing" && "$existing" != "$local_integrity" ]]; then
      echo "publish.sh: $name@$version exists with different contents" >&2
      exit 1
    fi
  done
fi

for tarball in "${TARBALLS[@]}"; do
  manifest="$(tar -xOf "$tarball" package/package.json)"
  name="$(node -p 'JSON.parse(process.argv[1]).name' "$manifest")"
  version="$(node -p 'JSON.parse(process.argv[1]).version' "$manifest")"
  local_integrity="$(integrity_of "$tarball")"

  if [[ -n "$DRY_RUN" ]]; then
    echo "▶ publish.sh: validated $name@$version ($local_integrity)"
    continue
  fi

  existing="$(remote_integrity "$name" "$version" || true)"
  if [[ -n "$existing" ]]; then
    [[ "$existing" == "$local_integrity" ]] || {
      echo "publish.sh: $name@$version exists with different contents" >&2
      exit 1
    }
    echo "▶ publish.sh: $name@$version already published and identical; skipping"
    continue
  fi

  echo "▶ publish.sh: npm publish $name@$version..."
  npm publish "$tarball" --access public --tag staging
  published=""
  for _ in 1 2 3 4 5 6; do
    published="$(remote_integrity "$name" "$version" || true)"
    [[ "$published" == "$local_integrity" ]] && break
    sleep 5
  done
  [[ "$published" == "$local_integrity" ]] || {
    echo "publish.sh: registry did not expose the published $name@$version tarball" >&2
    exit 1
  }
done

# All exact versions now exist and passed integrity verification. Promote tags
# in dependency order; `umadev` is last, so ordinary installs stay on the prior
# complete release until every dependency is ready.
if [[ -z "$DRY_RUN" ]]; then
  for tarball in "${TARBALLS[@]}"; do
    manifest="$(tar -xOf "$tarball" package/package.json)"
    name="$(node -p 'JSON.parse(process.argv[1]).name' "$manifest")"
    version="$(node -p 'JSON.parse(process.argv[1]).version' "$manifest")"
    npm dist-tag add "$name@$version" latest
    tagged="$(remote_version "$name" latest || true)"
    [[ "$tagged" == "$version" ]] || {
      echo "publish.sh: latest for $name is $tagged, expected $version" >&2
      exit 1
    }
  done

  for tarball in "${TARBALLS[@]}"; do
    manifest="$(tar -xOf "$tarball" package/package.json)"
    name="$(node -p 'JSON.parse(process.argv[1]).name' "$manifest")"
    npm dist-tag rm "$name" staging >/dev/null 2>&1 || true
  done
fi

echo "✓ publish.sh: done"
