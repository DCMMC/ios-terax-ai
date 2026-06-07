# Building a certificate-free iOS 16 jailbreak IPA

This document describes how to produce a **fake-signed** Terax `.ipa` for
**jailbroken / sideloaded** iOS 16 devices, without an Apple Developer account,
signing certificate, or provisioning profile.

> ⚠️ Status: this path is new scaffolding. The fake-sign + packaging logic is
> standard, but it has only been exercised on a real macOS runner a limited
> number of times. The `ios-linuxkit` native cross-build (the terminal backend)
> is the most likely place to need per-runner adjustment. Start with the
> backend **disabled** to validate the sign/package pipeline, then enable it.

## Why a separate path from `ios:build:device`

`bun run ios:build:device` runs `tauri ios build`, which archives **and
exports** through Xcode. The export step requires an Apple signing identity and
provisioning profile, so it cannot run without a paid/registered certificate.

The jailbreak path instead:

1. Builds the app **unsigned** with `xcodebuild`
   (`CODE_SIGNING_ALLOWED=NO`), and
2. Fake-signs the resulting `.app` with [`ldid`](https://github.com/ProcursusTeam/ldid),
   embedding [`src-tauri/ios/Terax.jailbreak.entitlements`](../src-tauri/ios/Terax.jailbreak.entitlements),
   then packages `Payload/Terax.app` into a `.ipa`.

Relevant files:

- `scripts/ios-linuxkit.mjs` — `build-jailbreak` command (unsigned xcodebuild)
- `scripts/ios-fakesign.mjs` — ldid fake-sign + IPA packaging
- `src-tauri/ios/Terax.jailbreak.entitlements` — entitlements embedded by ldid
- `.github/workflows/ios-jailbreak.yml` — macOS CI that produces the IPA

## Option A — Build in CI (recommended, no Mac needed)

A macOS GitHub Actions workflow does the whole thing and uploads the IPA as an
artifact.

1. Push this branch.
2. GitHub → **Actions** → **iOS Jailbreak IPA** → **Run workflow**.
   - `configuration`: `debug` (default) or `release`.
   - `linuxkit_backend`:
     - `disabled` (default) — fast; **validates signing/packaging** but ships a
       stubbed terminal (`TERAX_IOS_LINUXKIT_NATIVE=0`).
     - `enabled` — also cross-builds `rcarmo/ios-linuxkit` for a working
       LinuxKit terminal (heavy; see caveats below).
3. Download the `Terax-ios16-jailbreak-*` artifact → `Terax.ipa`.

Pushing a tag matching `ios-v*` also triggers the workflow.

## Option B — Build locally on a Mac

Prerequisites: macOS + Xcode, Rust with the `aarch64-apple-ios` target, Bun,
and `ldid` (`brew install ldid`).

```bash
bun install
rustup target add aarch64-apple-ios

# Pipeline-only validation (stubbed terminal):
TERAX_IOS_LINUXKIT_NATIVE=0 bun run ios:build:jailbreak

# Or with the LinuxKit terminal backend (needs a sibling ../ios-linuxkit):
bun run ios:prepare          # build ios-linuxkit libs + sync rootfs
bun run ios:build:jailbreak  # TERAX_IOS_LINUXKIT_NATIVE defaults to on
```

Output: `src-tauri/gen/apple/build/arm64/Terax.ipa`.

Useful overrides:

- `IOS_JB_CONFIG=release` — build the release configuration (default `debug`).
- `IOS_LINUXKIT_DIR=/path/to/ios-linuxkit` — non-sibling LinuxKit checkout.

You can also fake-sign an already-built `.app` directly:

```bash
bun run ios:fakesign -- --app /path/to/Terax.app --out /tmp/Terax.ipa
```

## Installing on a jailbroken iOS 16 device

- **TrollStore**: open the `.ipa` in TrollStore and install. For TrollStore you
  may uncomment the `platform-application` / `com.apple.private.*` block in the
  entitlements plist to grant more privileges.
- **AppSync Unified** (rootful/rootless jailbreak): with AppSync installed, the
  fake-signed IPA installs via Filza or your preferred installer.

`ios-linuxkit` runs as a userspace emulator (ASBESTOS), so it needs **no** JIT,
no-sandbox, or private entitlements to launch — `get-task-allow` is enough.

## Caveats / things likely to need iteration

These were written without a macOS runner in the loop; expect to adjust on the
first real CI run:

- **`ios-linuxkit` native build.** `bun run ios:linuxkit:build` drives the
  Xcode targets, but `rcarmo/ios-linuxkit` has its own build system for the C
  dependencies (`libarchive` and the meson `libish_emu`/`libfakefs` libs that
  `build.rs` links). If the `enabled` backend run fails at link time, build
  those deps first per that repo's instructions, then re-run.
- **Scheme / configuration names.** `build-jailbreak` auto-detects the iOS
  scheme via `xcodebuild -list -json` and defaults to the `debug`
  configuration. If detection picks the wrong scheme, set it explicitly in
  `scripts/ios-linuxkit.mjs` (`detectIosScheme`).
- **`developmentTeam` in `tauri.conf.json`** (`39MSPXR3CC`) is irrelevant to
  this path because signing is disabled, but `tauri ios init` may still warn.
- **Embedded binaries.** `ios-fakesign.mjs` detects Mach-O files by magic bytes
  and signs every framework/dylib/app-extension, applying entitlements only to
  the main executable. If a future build embeds extensions needing their own
  entitlements, extend the script accordingly.
