#!/usr/bin/env bash
# Registry-consistency helpers shared by the npm release publisher and its
# deterministic contract tests. The caller supplies remote_integrity().

VISIBILITY_ATTEMPTS="${UMADEV_NPM_VISIBILITY_ATTEMPTS:-60}"
VISIBILITY_DELAY_SECONDS="${UMADEV_NPM_VISIBILITY_DELAY_SECONDS:-5}"

[[ "$VISIBILITY_ATTEMPTS" =~ ^[1-9][0-9]*$ ]] || {
  echo "publish.sh: UMADEV_NPM_VISIBILITY_ATTEMPTS must be a positive integer" >&2
  return 1 2>/dev/null || exit 1
}
[[ "$VISIBILITY_DELAY_SECONDS" =~ ^[0-9]+$ ]] || {
  echo "publish.sh: UMADEV_NPM_VISIBILITY_DELAY_SECONDS must be a non-negative integer" >&2
  return 1 2>/dev/null || exit 1
}

recoverable_duplicate_publish() {
  local output="$1"
  [[ "$output" == *"cannot publish over"* ||
     "$output" == *"previously published version"* ||
     "$output" == *"previously published versions"* ]]
}

wait_for_remote_integrity() {
  local name="$1"
  local version="$2"
  local expected="$3"
  local actual=""
  local attempt

  for ((attempt = 1; attempt <= VISIBILITY_ATTEMPTS; attempt += 1)); do
    actual="$(remote_integrity "$name" "$version" || true)"
    if [[ "$actual" == "$expected" ]]; then
      return 0
    fi
    if [[ -n "$actual" ]]; then
      echo "publish.sh: $name@$version became visible with different contents" >&2
      return 2
    fi
    if ((attempt < VISIBILITY_ATTEMPTS)); then
      sleep "$VISIBILITY_DELAY_SECONDS"
    fi
  done

  return 1
}
