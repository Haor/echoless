#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BUILD_DIR="$SCRIPT_DIR/.build"
OUT="$BUILD_DIR/echoless-process-tap-poc"

mkdir -p "$BUILD_DIR/module-cache"

xcrun swiftc \
  -module-cache-path "$BUILD_DIR/module-cache" \
  -framework CoreAudio \
  -framework Foundation \
  -Xlinker -sectcreate \
  -Xlinker __TEXT \
  -Xlinker __info_plist \
  -Xlinker "$SCRIPT_DIR/Info.plist" \
  -o "$OUT" \
  "$SCRIPT_DIR/Sources/main.swift"

codesign --force --sign - "$OUT" >/dev/null

echo "$OUT"
