mod modules;

use modules::{fs, net, pty, secrets, shell, workspace};
#[cfg(not(mobile))]
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
#[cfg(not(mobile))]
use tauri_plugin_window_state::StateFlags;

#[cfg(target_os = "ios")]
extern "C" {
    fn TeraxInstallKeyCommands();
    fn TeraxFocusTerminalInput();
    fn TeraxSetTerminalInputEnabled(enabled: bool);
    fn TeraxSetTerminalApplicationCursor(enabled: bool);
}

#[cfg(target_os = "ios")]
#[tauri::command]
fn ios_focus_terminal_input() {
    unsafe {
        TeraxFocusTerminalInput();
    }
}

#[cfg(not(target_os = "ios"))]
#[tauri::command]
fn ios_focus_terminal_input() {}

#[cfg(target_os = "ios")]
#[tauri::command]
fn ios_set_terminal_input_enabled(enabled: bool) {
    unsafe {
        TeraxSetTerminalInputEnabled(enabled);
    }
}

#[cfg(not(target_os = "ios"))]
#[tauri::command]
fn ios_set_terminal_input_enabled(_enabled: bool) {}

#[cfg(target_os = "ios")]
#[tauri::command]
fn ios_set_terminal_application_cursor(enabled: bool) {
    unsafe {
        TeraxSetTerminalApplicationCursor(enabled);
    }
}

#[cfg(not(target_os = "ios"))]
#[tauri::command]
fn ios_set_terminal_application_cursor(_enabled: bool) {}

#[tauri::command]
fn ios_debug_log(message: String) {
    log::info!("[ios-terminal] {message}");
}

#[cfg(not(mobile))]
#[tauri::command]
async fn open_settings_window(app: tauri::AppHandle, tab: Option<String>) -> Result<(), String> {
    let url_path = match tab.as_deref() {
        Some(t) if !t.is_empty() => format!("settings.html?tab={}", t),
        _ => "settings.html".to_string(),
    };

    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.set_focus();
        if let Some(t) = tab.as_deref().filter(|s| !s.is_empty()) {
            // emit() serializes via JSON — no string-escape footgun, unlike
            // eval() with format!(). Frontend listens via Tauri event API.
            let _ = window.emit("terax:settings-tab", t);
        }
        return Ok(());
    }

    let mut builder = WebviewWindowBuilder::new(&app, "settings", WebviewUrl::App(url_path.into()))
        .title("Settings")
        .inner_size(720.0, 520.0)
        .min_inner_size(720.0, 520.0)
        .max_inner_size(720.0, 520.0)
        .resizable(false)
        .visible(false)
        // Keep settings above the main app window so it doesn't get hidden
        // when the user clicks back into the editor or terminal (#33).
        .always_on_top(true);

    // Tie lifecycle to the main window so settings minimizes/closes with it.
    if let Some(main) = app.get_webview_window("main") {
        builder = builder.parent(&main).map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true);

    // On Linux/Windows we render our own titlebar, so drop native chrome
    // and make the window transparent.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let builder = builder.decorations(false).transparent(true);

    let window = builder.build().map_err(|e| e.to_string())?;

    // Some Linux compositors (GNOME/Mutter with CSD-by-default) ignore the
    // builder-time decorations flag — re-assert it after realize.
    #[cfg(target_os = "linux")]
    {
        let _ = window.set_decorations(false);
    }
    let _ = window;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "ios")]
    unsafe {
        TeraxInstallKeyCommands();
    }

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_process::init());

    // Skip restoring VISIBLE — frontend calls window.show() after first
    // paint so the user never sees a transparent window-shadow flash on
    // Windows/Linux. These plugins are desktop-only.
    #[cfg(not(mobile))]
    let builder = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_window_state::Builder::new()
                .with_state_flags(StateFlags::all() & !StateFlags::VISIBLE)
                .build(),
        )
        .plugin(tauri_plugin_autostart::Builder::new().build());

    builder
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(tauri_plugin_log::log::LevelFilter::Info)
                // Forward to the webview console too, so native logs
                // ([ios-terminal/native] …) are visible in the on-device
                // devtools (eruda) for debugging the LinuxKit backend.
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Webview),
                ])
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .manage(pty::PtyState::default())
        .manage(shell::ShellState::default())
        .manage(secrets::SecretsState::default())
        .setup(|_app| {
            #[cfg(all(mobile, target_os = "ios", terax_ios_linuxkit_native))]
            modules::ios_linuxkit_native::ensure_booted()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            pty::pty_open,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_close,
            fs::tree::list_subdirs,
            fs::tree::fs_read_dir,
            fs::file::fs_read_file,
            fs::file::fs_write_file,
            fs::file::fs_stat,
            fs::file::fs_canonicalize,
            fs::mutate::fs_create_file,
            fs::mutate::fs_create_dir,
            fs::mutate::fs_rename,
            fs::mutate::fs_delete,
            fs::search::fs_search,
            fs::search::fs_list_files,
            fs::grep::fs_grep,
            fs::grep::fs_glob,
            shell::shell_run_command,
            shell::shell_session_open,
            shell::shell_session_run,
            shell::shell_session_close,
            shell::shell_bg_spawn,
            shell::shell_bg_logs,
            shell::shell_bg_kill,
            shell::shell_bg_list,
            workspace::wsl_list_distros,
            workspace::wsl_default_distro,
            workspace::wsl_home,
            #[cfg(mobile)]
            modules::linuxkit::linuxkit_health,
            ios_focus_terminal_input,
            ios_set_terminal_input_enabled,
            ios_set_terminal_application_cursor,
            ios_debug_log,
            #[cfg(not(mobile))]
            open_settings_window,
            secrets::secrets_get,
            secrets::secrets_set,
            secrets::secrets_delete,
            secrets::secrets_get_all,
            net::lm_ping,
            net::ai_http_request,
            net::ai_http_stream,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
