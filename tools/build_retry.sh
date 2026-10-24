#!/usr/bin/env bash
# Rebuild with retries.
#
# Smart App Control randomly blocks freshly compiled build scripts
# ("An Application Control policy has blocked this file", os error 4551).
# Deleting the blocked build dir plus its fingerprint and retrying a few
# times usually gets through, because the policy check is not stable.
#
# Usage: tools/build_retry.sh [extra cargo args...]
#   tools/build_retry.sh --features custom-protocol
set -u

CRATE_DIR="$(cd "$(dirname "$0")/.." && pwd)/src-tauri"
cd "$CRATE_DIR" || exit 1

MAX=20
# 记录 profile，清理失效产物时要用
PROFILE_FLAG=""
for arg in "$@"; do
  if [ "$arg" = "--release" ]; then PROFILE_FLAG="--release"; fi
done

for i in $(seq 1 $MAX); do
  echo "=== attempt $i/$MAX ==="
  OUT="$(cargo build -j 1 "$@" 2>&1)"
  STATUS=$?

  if [ $STATUS -eq 0 ]; then
    echo "$OUT" | tail -5
    echo "BUILD-OK"
    exit 0
  fi

  # 情况二：proc-macro 被拦后产物缺失，rustc 报 "can't find crate for `xxx_impl`"。
  # 这种要把对应包整个清掉重编，改 build 目录没用。
  # 情况二：proc-macro 的 .dll 被拦，rustc 报 LoadLibraryExW / can't find crate。
  # 这类没有 build 目录可删，只能把整个包清掉重编。
  BLOCKED_DLL="$(echo "$OUT" | grep -o 'deps.[a-z0-9_]*-[0-9a-f]\{16\}\.dll' | head -1)"
  if echo "$OUT" | grep -q "4551" && [ -n "$BLOCKED_DLL" ]; then
    LIB="$(basename "$BLOCKED_DLL" | sed 's/-[0-9a-f]\{16\}\.dll//')"
    PKG="$(echo "$LIB" | tr '_' '-')"
    echo "blocked dll: $LIB -> cargo clean -p $PKG $PROFILE_FLAG"
    cargo clean -p "$PKG" $PROFILE_FLAG >/dev/null 2>&1
    continue
  fi

  MISSING="$(echo "$OUT" | grep -o "can't find crate for \`[a-zA-Z0-9_]*\`" | head -1 | sed 's/.*`\(.*\)`/\1/')"
  if [ -n "$MISSING" ]; then
    PKG="$(echo "$MISSING" | tr '_' '-')"
    echo "missing crate: $MISSING -> cargo clean -p $PKG $PROFILE_FLAG"
    cargo clean -p "$PKG" $PROFILE_FLAG >/dev/null 2>&1
    continue
  fi

  # 情况一：os error 4551，直接删掉被拦的 build 目录再试
  BLOCKED="$(echo "$OUT" | grep -o 'target.[a-z]*.build.[A-Za-z0-9_.-]*-[0-9a-f]\{16\}' | head -1 | sed 's|target\.\([a-z]*\)\.build\.|target/\1/build/|')"
  if echo "$OUT" | grep -q "4551" && [ -n "$BLOCKED" ]; then
    FINGERPRINT="$(basename "$BLOCKED")"
    PROFILE="$(echo "$BLOCKED" | sed 's|target/\([a-z]*\)/build/.*|\1|')"
    echo "blocked: $PROFILE/$FINGERPRINT -> removing and retrying"
    rm -rf "$BLOCKED" "target/$PROFILE/.fingerprint/$FINGERPRINT"
    continue
  fi

  # Anything else is a real error: report it and stop.
  echo "$OUT" | tail -30
  echo "BUILD-FAILED"
  exit 1
done

echo "BUILD-GAVE-UP (blocked $MAX times)"
exit 1
