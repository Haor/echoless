#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BUILD_DIR="$SCRIPT_DIR/.build"
OUT="$BUILD_DIR/echoless-process-tap-poc"
STAMP="$BUILD_DIR/source.sha256"

mkdir -p "$BUILD_DIR/module-cache"

# 源码指纹缓存:源码没变就不重建。链接器每次生成随机 LC_UUID → 重建必然改变
# 代码签名哈希 → helper 的 TCC「系统音频录制」授权作废(授权按 路径+签名 记账)。
# 字节稳定 = 授权跨 dev 重启存活。
SIGNING_IDENTITY="$(security find-identity -v -p codesigning 2>/dev/null | grep "Echoless Dev" || true)"
FINGERPRINT=$({ cat "$SCRIPT_DIR/Sources/main.swift" "$SCRIPT_DIR/Info.plist" "$0"; printf '%s\n' "$SIGNING_IDENTITY"; } | shasum -a 256 | cut -d' ' -f1)
if [[ -f "$OUT" && -f "$STAMP" && "$(cat "$STAMP")" == "$FINGERPRINT" ]]; then
  echo "$OUT"
  exit 0
fi

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

# 有 "Echoless Dev" 自签证书就用它(TCC 记 identifier+证书,连源码变更都能存活);
# 否则 ad-hoc。
if [[ -n "$SIGNING_IDENTITY" ]]; then
  codesign --force --sign "Echoless Dev" "$OUT" >/dev/null
else
  codesign --force --sign - "$OUT" >/dev/null
fi

echo "$FINGERPRINT" > "$STAMP"
echo "$OUT"
