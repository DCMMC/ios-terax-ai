#!/usr/bin/env bash
# Local dev-signed iOS build + USB install loop (no AirDrop, no TrollStore).
# Requires: rustup cargo on PATH first, a free/paid Apple Development team, and
# the device's developer cert trusted once on-device.
#
#   bundle id : com.dcmmc.teraxdbg   (tauri.conf identifier)
#   team      : TERAX_IOS_DEV_TEAM   (enables real signing in patch-ios-project)
#
# Usage: scripts/dev-ios.sh            # clean build + install
#        scripts/dev-ios.sh build      # build only
set -euo pipefail
cd "$(dirname "$0")/.."

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"   # rustup cargo MUST win (iOS std)
export TERAX_IOS_DEV_TEAM="${TERAX_IOS_DEV_TEAM:-2LRBQ7W9E4}"
UDID="${IOS_DEVICE_UDID:-00008103-001A04502133001E}"
IPA="src-tauri/gen/apple/build/arm64/Terax.ipa"

echo "== which cargo: $(command -v cargo) =="

# A clean export is required for a correctly signed IPA — the incremental
# `tauri ios build` can reuse a stale (unsigned) archive.
rm -rf src-tauri/gen/apple
bunx tauri ios init --ci
# pod install exits 1 on the benign `terax_macOS` target warning (Tauri emits an
# iOS+macOS Podfile; the project has only iOS) but still writes Pods/ + workspace.
( cd src-tauri/gen/apple && pod install ) || true
bunx tauri ios build --target aarch64 --export-method debugging

echo "== signature check =="
rm -rf /tmp/ipachk && mkdir -p /tmp/ipachk && unzip -q "$IPA" -d /tmp/ipachk
codesign -dv /tmp/ipachk/Payload/Terax.app 2>&1 | grep -iE "Authority|TeamIdentifier" || { echo "!! IPA is UNSIGNED"; exit 1; }

if [ "${1:-}" = "build" ]; then echo "build-only: skipping install"; exit 0; fi

echo "== install over USB (close the app on device first if it hangs) =="
ideviceinstaller -u "$UDID" install "$IPA"
echo "== done =="
