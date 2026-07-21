#!/usr/bin/env bash
# Publish the entire npm distribution: seven cli-* packages and the knowledge
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
# shellcheck source=publish-registry.sh
source "$SCRIPT_DIR/publish-registry.sh"

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
  "cli-linux-musl-x64"
  "cli-linux-musl-arm64"
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

# A file existing at the right path is not proof that it belongs to that
# package. This exact mistake can make every non-macOS install fail with
# "exec format error" while `npm publish` itself reports success. Inspect the
# executable headers before packing or touching the registry.
node "$SCRIPT_DIR/verify-platform-binaries.mjs" "$NPM_ROOT"

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
  # A publish can be accepted by the registry before every packument replica
  # exposes it. Force revalidation on every poll instead of reading npm's local
  # cache, otherwise a successful first publish can look absent for the whole
  # retry window.
  npm view "$1@$2" dist.integrity --json --prefer-online 2>/dev/null | node -e '
    const fs = require("node:fs");
    try {
      const value = JSON.parse(fs.readFileSync(0, "utf8"));
      if (typeof value === "string") process.stdout.write(value);
    } catch (_) {}
  '
}

remote_version() {
  # --prefer-online forces a registry revalidation so a retry loop re-reads the
  # live dist-tag instead of a cached packument from an earlier (stale) read.
  npm view "$1@$2" version --json --prefer-online 2>/dev/null | node -e '
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
  publish_status=0
  publish_output="$(npm publish "$tarball" --access public --tag staging 2>&1)" || publish_status=$?
  printf '%s\n' "$publish_output"

  if ((publish_status != 0)); then
    # npm can accept a new immutable version, then keep its public packument
    # stale long enough for a retry to attempt the same publish. That retry gets
    # E403 even though its exact tarball is already durable. Only this precise
    # duplicate-version family is recoverable; authentication/authorization and
    # every other publish failure remain immediate hard failures.
    if ! recoverable_duplicate_publish "$publish_output"; then
      echo "publish.sh: npm publish failed for $name@$version" >&2
      exit "$publish_status"
    fi
    echo "▶ publish.sh: registry accepted $name@$version earlier; waiting for public visibility"
  fi

  if ! wait_for_remote_integrity "$name" "$version" "$local_integrity"; then
    echo "publish.sh: registry did not expose the published $name@$version tarball after $VISIBILITY_ATTEMPTS checks" >&2
    exit 1
  fi
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
    # npm's registry is read-after-write eventually consistent: a read
    # immediately after `dist-tag add` can still return the previous value from a
    # replica or CDN edge. The add above is authoritative, so poll the read-back
    # (up to ~40s) rather than failing an otherwise-successful promotion on the
    # first stale reply — the exact flake that left 1.0.56 published but with
    # `latest` still pointing at the prior version.
    tagged=""
    for ((attempt = 1; attempt <= VISIBILITY_ATTEMPTS; attempt += 1)); do
      tagged="$(remote_version "$name" latest || true)"
      [[ "$tagged" == "$version" ]] && break
      if ((attempt < VISIBILITY_ATTEMPTS)); then
        sleep "$VISIBILITY_DELAY_SECONDS"
      fi
    done
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
