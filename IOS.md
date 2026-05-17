# Terax iOS port notes

This branch starts the iOS conversion but the app is not yet a shippable iOS terminal.

The intended execution backend is [`rcarmo/ios-linuxkit`](https://github.com/rcarmo/ios-linuxkit), not the iOS host sandbox. See [`docs/IOS_LINUXKIT_BACKEND.md`](docs/IOS_LINUXKIT_BACKEND.md) for the command/backend contract.

## Frontend runtime

The React runtime has been swapped to Preact using `@preact/preset-vite` and `preact/compat` aliases. Existing imports from `react` / `react-dom/client` are intentionally left in place so third-party React libraries and app code resolve through the compatibility layer.

## Tauri iOS commands

```bash
bun install
bun run ios:init
bun run ios:dev
bun run ios:build
```

`tauri ios init` will generate the Xcode project under `src-tauri/gen/apple` on a macOS host with Xcode, Rust iOS targets, CocoaPods, and Tauri mobile prerequisites installed.

## iOS backend direction

Terax should map its existing Tauri command surface onto `ios-linuxkit`:

- terminal tabs attach to LinuxKit PTYs
- AI shell tools execute inside the Linux guest
- file explorer/editor operate on the LinuxKit workspace filesystem
- host/iOS filesystem access is limited to explicit import/export flows
- secrets stay in iOS keychain/Tauri secret storage, not inside the guest filesystem

## Remaining iOS blockers

Several capabilities need mobile-specific replacements or stubs before App Store-ready iOS builds are realistic:

- `portable-pty` and desktop shell/session code need a `mobile` LinuxKit implementation.
- Multiple desktop windows/settings window APIs need an iOS-specific settings route/screen.
- Desktop plugins such as updater/window-state/autostart are disabled for mobile; any UI that assumes them should be hidden on iOS.
- File explorer/editor access must be constrained to the LinuxKit workspace plus iOS document-provider import/export flows.
- Voice/network/key storage need device testing with Tauri's iOS WebView and plugin availability.

Recommended next step: introduce a `platformCapabilities` layer in the frontend and Rust command stubs for `mobile`, then replace those stubs with `ios-linuxkit` PTY/shell/filesystem adapters.
