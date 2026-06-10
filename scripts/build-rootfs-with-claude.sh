#!/usr/bin/env bash
# Bake node + npm + @anthropic-ai/claude-code into the ios-linuxkit rootfs.
#
# The committed src-tauri/resources/ios-linuxkit/root.tar.gz is the MINIMAL
# Alpine base (kept small in git). This script produces the claude-enriched
# rootfs that actually ships, by importing the committed base into Docker,
# running the same steps that work on-device (TUNA apk mirror -> apk add nodejs
# npm -> npm i -g claude-code via npmmirror), and exporting back to root.tar.gz.
#
# Runs in CI on a Linux runner (docker + binfmt/qemu for linux/arm64) and
# locally on Apple-Silicon (native arm64 via colima/Docker). The base is read
# from `git show HEAD:` so re-runs are idempotent regardless of working-tree
# state. Does NOT bake ~/.claude/settings.json (auth token is dropped in at
# runtime). Commit the MINIMAL base only — let CI bake.
set -euo pipefail
cd "$(dirname "$0")/.."
export PATH="/opt/homebrew/bin:$PATH"   # harmless on Linux; finds docker on macOS

ROOTFS="src-tauri/resources/ios-linuxkit/root.tar.gz"
IMG_BASE="terax-rootfs-base:latest"
IMG_OUT="terax-rootfs-claude:latest"
PLATFORM="linux/arm64"
sha() { command -v sha256sum >/dev/null && sha256sum "$1" | awk '{print $1}' || shasum -a 256 "$1" | awk '{print $1}'; }

echo "== import committed minimal base (git HEAD: $ROOTFS) =="
docker rmi "$IMG_BASE" "$IMG_OUT" >/dev/null 2>&1 || true
git show "HEAD:$ROOTFS" | docker import --platform "$PLATFORM" - "$IMG_BASE"

echo "== build claude-enriched rootfs =="
docker build --platform "$PLATFORM" -t "$IMG_OUT" -f - . <<'DOCKERFILE'
# syntax=docker/dockerfile:1
ARG BASE=terax-rootfs-base:latest
FROM ${BASE}
RUN printf '%s\n' \
      "https://mirrors.tuna.tsinghua.edu.cn/alpine/latest-stable/main" \
      "https://mirrors.tuna.tsinghua.edu.cn/alpine/latest-stable/community" \
      > /etc/apk/repositories \
 && apk update \
 && apk add --no-cache nodejs npm \
 && npm config set registry https://registry.npmmirror.com \
 && npm i -g @anthropic-ai/claude-code \
 && npm cache clean --force \
 && mkdir -p /root/.claude \
 && node --version && /usr/local/bin/claude --version
DOCKERFILE

echo "== export enriched rootfs -> $ROOTFS =="
CID=$(docker create --platform "$PLATFORM" "$IMG_OUT" /bin/sh)
trap 'docker rm -f "$CID" >/dev/null 2>&1 || true' EXIT
docker export "$CID" | gzip -9 > "$ROOTFS.new"
mv "$ROOTFS.new" "$ROOTFS"

echo "== result =="
ls -la "$ROOTFS"
echo "sha256: $(sha "$ROOTFS")"
echo "done. (working-tree $ROOTFS is now baked; do NOT commit it — git keeps the minimal base)"
