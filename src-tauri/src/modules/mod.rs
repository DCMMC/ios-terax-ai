pub mod fs;
#[cfg(mobile)]
pub mod linuxkit;
pub mod net;
#[cfg(not(mobile))]
pub mod pty;
#[cfg(mobile)]
pub mod pty {
    use tauri::ipc::{Channel, Response};

    use crate::modules::linuxkit::LinuxKitBackend;
    use crate::modules::workspace::WorkspaceEnv;

    #[derive(Default)]
    pub struct PtyState;

    #[tauri::command]
    pub fn pty_open(
        _state: tauri::State<PtyState>,
        cols: u16,
        rows: u16,
        cwd: Option<String>,
        _workspace: Option<WorkspaceEnv>,
        on_data: Channel<Response>,
        on_exit: Channel<i32>,
    ) -> Result<u32, String> {
        LinuxKitBackend::new().pty_open(cols, rows, cwd, on_data, on_exit)
    }

    #[tauri::command]
    pub fn pty_write(_state: tauri::State<PtyState>, id: u32, data: String) -> Result<(), String> {
        LinuxKitBackend::new().pty_write(id, data)
    }

    #[tauri::command]
    pub fn pty_resize(
        _state: tauri::State<PtyState>,
        id: u32,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        LinuxKitBackend::new().pty_resize(id, cols, rows)
    }

    #[tauri::command]
    pub fn pty_close(_state: tauri::State<PtyState>, id: u32) -> Result<(), String> {
        LinuxKitBackend::new().pty_close(id)
    }
}
pub mod secrets;
#[cfg(not(mobile))]
pub mod shell;
#[cfg(mobile)]
pub mod shell {
    use serde::{Deserialize, Serialize};

    use crate::modules::linuxkit::{
        BackgroundKillRequest, BackgroundLogsRequest, BackgroundSpawnRequest, LinuxKitBackend,
        LinuxKitRequest, LinuxKitResponse, ShellRunRequest, ShellSessionCloseRequest,
        ShellSessionOpenRequest, ShellSessionRunRequest,
    };
    use crate::modules::workspace::WorkspaceEnv;

    #[derive(Default)]
    pub struct ShellState;

    #[derive(Serialize, Deserialize)]
    pub struct CommandOutput {
        pub stdout: String,
        pub stderr: String,
        pub exit_code: Option<i32>,
        pub timed_out: bool,
        pub truncated: bool,
    }

    #[derive(Serialize, Deserialize)]
    pub struct SessionRunOutput {
        pub stdout: String,
        pub stderr: String,
        pub exit_code: Option<i32>,
        pub timed_out: bool,
        pub truncated: bool,
        pub cwd_after: String,
    }

    #[derive(Serialize, Deserialize)]
    pub struct BackgroundLogResponse {
        pub bytes: String,
        pub next_offset: u64,
        pub dropped: u64,
        pub exited: bool,
        pub exit_code: Option<i32>,
    }

    #[derive(Serialize, Deserialize)]
    pub struct BackgroundProcInfo {
        pub handle: u32,
        pub command: String,
        pub cwd: Option<String>,
        pub started_at_ms: u64,
        pub exited: bool,
        pub exit_code: Option<i32>,
    }

    fn decode_json<T: serde::de::DeserializeOwned>(
        response: LinuxKitResponse,
        command: &str,
    ) -> Result<T, String> {
        match response {
            LinuxKitResponse::Json { value } => {
                serde_json::from_value(value).map_err(|e| e.to_string())
            }
            _ => Err(format!(
                "ios-linuxkit {command} returned an unexpected response"
            )),
        }
    }

    #[tauri::command]
    pub async fn shell_run_command(
        command: String,
        cwd: Option<String>,
        timeout_secs: Option<u64>,
        _workspace: Option<WorkspaceEnv>,
    ) -> Result<CommandOutput, String> {
        let response =
            LinuxKitBackend::new().request(LinuxKitRequest::ShellRun(ShellRunRequest {
                command,
                cwd,
                timeout_secs,
            }))?;
        decode_json(response, "shell_run_command")
    }

    #[tauri::command]
    pub fn shell_session_open(
        _state: tauri::State<ShellState>,
        cwd: Option<String>,
        _workspace: Option<WorkspaceEnv>,
    ) -> Result<u32, String> {
        match LinuxKitBackend::new().request(LinuxKitRequest::ShellSessionOpen(
            ShellSessionOpenRequest { cwd },
        ))? {
            LinuxKitResponse::U32 { value } => Ok(value),
            _ => Err("ios-linuxkit shell_session_open returned an unexpected response".into()),
        }
    }

    #[tauri::command]
    pub async fn shell_session_run(
        _state: tauri::State<'_, ShellState>,
        id: u32,
        command: String,
        cwd: Option<String>,
        timeout_secs: Option<u64>,
    ) -> Result<SessionRunOutput, String> {
        let response = LinuxKitBackend::new().request(LinuxKitRequest::ShellSessionRun(
            ShellSessionRunRequest {
                id,
                command,
                cwd,
                timeout_secs,
            },
        ))?;
        decode_json(response, "shell_session_run")
    }

    #[tauri::command]
    pub fn shell_session_close(_state: tauri::State<ShellState>, id: u32) -> Result<(), String> {
        LinuxKitBackend::new()
            .request(LinuxKitRequest::ShellSessionClose(
                ShellSessionCloseRequest { id },
            ))
            .map(|_| ())
    }

    #[tauri::command]
    pub fn shell_bg_spawn(
        _state: tauri::State<ShellState>,
        command: String,
        cwd: Option<String>,
        _workspace: Option<WorkspaceEnv>,
    ) -> Result<u32, String> {
        match LinuxKitBackend::new().request(LinuxKitRequest::BackgroundSpawn(
            BackgroundSpawnRequest { command, cwd },
        ))? {
            LinuxKitResponse::U32 { value } => Ok(value),
            _ => Err("ios-linuxkit shell_bg_spawn returned an unexpected response".into()),
        }
    }

    #[tauri::command]
    pub fn shell_bg_logs(
        _state: tauri::State<ShellState>,
        handle: u32,
        since_offset: Option<u64>,
    ) -> Result<BackgroundLogResponse, String> {
        let response = LinuxKitBackend::new().request(LinuxKitRequest::BackgroundLogs(
            BackgroundLogsRequest {
                handle,
                since_offset,
            },
        ))?;
        decode_json(response, "shell_bg_logs")
    }

    #[tauri::command]
    pub fn shell_bg_kill(_state: tauri::State<ShellState>, handle: u32) -> Result<(), String> {
        LinuxKitBackend::new()
            .request(LinuxKitRequest::BackgroundKill(BackgroundKillRequest {
                handle,
            }))
            .map(|_| ())
    }

    #[tauri::command]
    pub fn shell_bg_list(
        _state: tauri::State<ShellState>,
    ) -> Result<Vec<BackgroundProcInfo>, String> {
        let response = LinuxKitBackend::new().request(LinuxKitRequest::BackgroundList)?;
        decode_json(response, "shell_bg_list")
    }
}
pub mod workspace;
