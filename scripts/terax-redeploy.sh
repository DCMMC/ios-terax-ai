#!/usr/bin/env bash
# One-shot: rebuild ios-linuxkit engine (with correct iphoneos SDK) + relink
# Terax + bundle the claude rootfs + fakesign + install over USB.
#
# Avoids the traps hit during debugging:
#  - ninja must run with SDKROOT=<iphoneos sdk> or it compiles for macOS
#    (cross.txt has no -isysroot; the SDK comes from the env).
#  - libish.a is whole-archived into cargo's libapp.a, and build.rs's
#    rerun-if-changed watches a stale symlink, so force a libapp rebuild.
#  - the build's resource staging is cached, so re-inject root.tar.gz into the
#    built .app before fakesigning.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:$PATH"

UDID="${IOS_DEVICE_UDID:-00008103-001A04502133001E}"
LK="${IOS_LINUXKIT_DIR:-$(cd ../ios-linuxkit && pwd)}"
MESON="$LK/build/Debug-ApplePleaseFixFB19282108-iphoneos/meson"
APP="src-tauri/gen/apple/build/terax_iOS.xcarchive/Products/Applications/Terax.app"
IPA="src-tauri/gen/apple/build/arm64/Terax.ipa"
SDK="$(xcrun --sdk iphoneos --show-sdk-path)"

echo "== [1/5] rebuild engine libs (iphoneos SDK) =="
SDKROOT="$SDK" ninja -C "$MESON" libish.a libfakefs.a libish_emu.a

echo "== [2/5] relink Terax (force libapp rebuild so fresh libish is embedded) =="
rm -f src-tauri/gen/apple/Externals/arm64/release/libapp.a
touch src-tauri/build.rs
bun scripts/ios-linuxkit.mjs build-jailbreak 2>&1 | tail -3

echo "== [3/5] inject claude rootfs into built .app =="
cp src-tauri/resources/ios-linuxkit/root.tar.gz "$APP/assets/resources/ios-linuxkit/root.tar.gz"

echo "== [4/5] fakesign + package =="
bun scripts/ios-fakesign.mjs --search src-tauri/gen/apple/build --out "$IPA" \
  --entitlements src-tauri/ios/Terax.jailbreak.entitlements 2>&1 | tail -2

echo "== [5/5] install over USB =="
ideviceinstaller -u "$UDID" install "$IPA" 2>&1 | tail -2
echo "== done. Force-quit + reopen Terax on the iPad. =="
