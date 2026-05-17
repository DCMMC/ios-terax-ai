# iOS backend: `rcarmo/ios-linuxkit`

Terax-on-iOS should not attempt to expose iOS as a local POSIX host. Instead, the mobile app should embed or bridge to [`rcarmo/ios-linuxkit`](https://github.com/rcarmo/ios-linuxkit) and treat it as the execution/workspace backend.

## Rationale

Terax already has a clean split between UI/agent orchestration in the webview and privileged OS operations in Rust/Tauri commands. On desktop those commands talk to the host filesystem, `portable-pty`, and host shells. On iOS, those same command names can be backed by an `ios-linuxkit` guest instead:

- terminal tabs attach to a LinuxKit PTY/session
- AI shell tools execute inside LinuxKit
- file explorer/editor operate on the LinuxKit workspace filesystem
- language runtimes, package managers, and agent CLIs run in the guest rather than the iOS sandbox

This keeps the frontend largely intact while replacing the command backend on `mobile` builds.

## Backend contract

The iOS backend should provide the same semantic surface as the current Tauri commands, even if the implementation is different:

### PTY/session

- `pty_open(cwd, cols, rows, workspace)` → session id + stream of bytes/events
- `pty_write(id, data)`
- `pty_resize(id, cols, rows)`
- `pty_close(id)`

### Shell/agent tools

- `shell_session_open(cwd, workspace)`
- `shell_session_run(id, command, cwd, timeout_secs)` → stdout/stderr/exit/timed_out/cwd_after
- `shell_session_close(id)`
- optional background process API matching `shell_bg_*`

### Filesystem

- `fs_read_dir`, `fs_read_file`, `fs_write_file`, `fs_stat`, `fs_canonicalize`
- `fs_create_file`, `fs_create_dir`, `fs_rename`, `fs_delete`
- `fs_search`, `fs_list_files`, `fs_grep`, `fs_glob`

These should operate on the LinuxKit root/workspace, not arbitrary iOS paths.

## Suggested Rust shape

Introduce a backend trait and select implementation by target:

```rust
#[cfg(mobile)]
mod linuxkit_backend;
#[cfg(not(mobile))]
mod desktop_backend;

trait TeraxBackend {
    // pty/session/fs/shell methods matching command semantics
}
```

Then keep the existing command names stable and delegate internally. This avoids touching most frontend code and preserves AI tool behavior.

## Frontend capability model

Add a small capability layer so the UI can hide desktop-only affordances:

```ts
export type RuntimeBackend = "desktop" | "ios-linuxkit";

export const platformCapabilities = {
  backend: isIOS() ? "ios-linuxkit" : "desktop",
  localHostShell: !isIOS(),
  linuxGuestShell: isIOS(),
  arbitraryHostFs: !isIOS(),
  multipleWindows: !isIOS(),
  autoUpdater: !isIOS(),
};
```

On iOS, Settings should become an in-app route/panel rather than a second Tauri window, and host filesystem pickers should be constrained to import/export/document-provider flows.

## Integration notes

- Prefer `ios-linuxkit` as an embedded runtime/library when feasible, not a network service.
- If the first integration is bridge-based, use a narrow message protocol with explicit method names, bounded payloads, request ids, and cancellation.
- Preserve Terax's read/write/shell approval model above the LinuxKit layer.
- Keep secret/API-key storage in iOS keychain/Tauri secret facilities, not inside the Linux guest filesystem.
- Store project workspaces in a LinuxKit-visible area, with explicit import/export to iOS Files.

## First milestone

1. Generate Tauri iOS project.
2. Compile a mobile build that opens the Preact UI.
3. Stub desktop-only commands on mobile with clear `unsupported on iOS until linuxkit backend is wired` errors. **Started:** `src-tauri/src/modules/mod.rs` now provides `cfg(mobile)` PTY/shell stubs routed through `modules/linuxkit.rs`.
4. Add LinuxKit-backed `pty_*` and `shell_session_*` first.
5. Add LinuxKit-backed filesystem commands.
6. Re-enable AI tools against the LinuxKit workspace.
