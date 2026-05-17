//! iOS LinuxKit backend scaffold.
//!
//! The desktop build talks directly to the host (`portable-pty`, host shell,
//! host filesystem). Mobile builds should keep the same Tauri command names but
//! route their implementation through `rcarmo/ios-linuxkit`.
//!
//! This file defines the seam: a small typed protocol plus a backend object the
//! `cfg(mobile)` command shims can call. The transport is intentionally a stub
//! for now because the final shape depends on how Terax embeds ios-linuxkit
//! (static library, ObjC/Swift bridge, or a narrow in-process message queue).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinuxKitRequest {
    PtyOpen(PtyOpenRequest),
    PtyWrite(PtyWriteRequest),
    PtyResize(PtyResizeRequest),
    PtyClose(PtyCloseRequest),
    ShellRun(ShellRunRequest),
    ShellSessionOpen(ShellSessionOpenRequest),
    ShellSessionRun(ShellSessionRunRequest),
    ShellSessionClose(ShellSessionCloseRequest),
    BackgroundSpawn(BackgroundSpawnRequest),
    BackgroundLogs(BackgroundLogsRequest),
    BackgroundKill(BackgroundKillRequest),
    BackgroundList,
    Fs(FsRequest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyOpenRequest {
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyWriteRequest {
    pub id: u32,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyResizeRequest {
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PtyCloseRequest {
    pub id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellRunRequest {
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellSessionOpenRequest {
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellSessionRunRequest {
    pub id: u32,
    pub command: String,
    pub cwd: Option<String>,
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellSessionCloseRequest {
    pub id: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundSpawnRequest {
    pub command: String,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundLogsRequest {
    pub handle: u32,
    pub since_offset: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundKillRequest {
    pub handle: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum FsRequest {
    ReadDir { path: String, show_hidden: bool },
    ReadFile { path: String },
    WriteFile { path: String, content: String },
    Stat { path: String },
    Canonicalize { path: String },
    CreateFile { path: String },
    CreateDir { path: String },
    Rename { from: String, to: String },
    Delete { path: String },
    Search { root: String, query: String, max_results: Option<usize> },
    ListFiles { root: String, max_results: Option<usize> },
    Grep {
        root: String,
        pattern: String,
        glob: Option<Vec<String>>,
        case_insensitive: Option<bool>,
        max_results: Option<usize>,
    },
    Glob { root: String, pattern: String, max_results: Option<usize> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinuxKitResponse {
    Unit,
    U32 { value: u32 },
    Json { value: serde_json::Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinuxKitError {
    pub code: String,
    pub message: String,
}

pub type LinuxKitResult<T> = Result<T, LinuxKitError>;

/// Placeholder for the eventual in-process bridge to ios-linuxkit.
#[derive(Default)]
pub struct LinuxKitTransport;

impl LinuxKitTransport {
    pub fn request(&self, request: LinuxKitRequest) -> LinuxKitResult<LinuxKitResponse> {
        let command = match request {
            LinuxKitRequest::PtyOpen(_) => "pty_open",
            LinuxKitRequest::PtyWrite(_) => "pty_write",
            LinuxKitRequest::PtyResize(_) => "pty_resize",
            LinuxKitRequest::PtyClose(_) => "pty_close",
            LinuxKitRequest::ShellRun(_) => "shell_run_command",
            LinuxKitRequest::ShellSessionOpen(_) => "shell_session_open",
            LinuxKitRequest::ShellSessionRun(_) => "shell_session_run",
            LinuxKitRequest::ShellSessionClose(_) => "shell_session_close",
            LinuxKitRequest::BackgroundSpawn(_) => "shell_bg_spawn",
            LinuxKitRequest::BackgroundLogs(_) => "shell_bg_logs",
            LinuxKitRequest::BackgroundKill(_) => "shell_bg_kill",
            LinuxKitRequest::BackgroundList => "shell_bg_list",
            LinuxKitRequest::Fs(_) => "fs_*",
        };
        Err(LinuxKitError {
            code: "linuxkit_transport_unwired".into(),
            message: format!(
                "{command} is not wired yet; mobile builds should delegate this request to rcarmo/ios-linuxkit"
            ),
        })
    }
}

#[derive(Default)]
pub struct LinuxKitBackend {
    transport: LinuxKitTransport,
}

impl LinuxKitBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn unsupported(&self, command: &str) -> String {
        format!(
            "{command} is not wired yet; mobile builds should delegate this command to rcarmo/ios-linuxkit"
        )
    }

    pub fn request(&self, request: LinuxKitRequest) -> Result<LinuxKitResponse, String> {
        self.transport.request(request).map_err(|e| e.message)
    }
}
