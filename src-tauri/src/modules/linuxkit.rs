//! iOS LinuxKit command adapter.
//!
//! Desktop builds run directly against the host shell, PTY and filesystem.
//! Mobile builds keep the same Tauri command names, but route through this
//! adapter so the frontend can exercise the normal Terax workflows on iOS.
//! The adapter is intentionally narrow and stateful; a native ios-linuxkit
//! transport can replace the command execution internals without changing the
//! frontend-facing command surface.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use flate2::read::GzDecoder;
use globset::{Glob, GlobSet, GlobSetBuilder};
use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, Response};

const MAX_READ_BYTES: u64 = 10 * 1024 * 1024;
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
const FILE_SIZE_CAP: u64 = 5 * 1024 * 1024;
const DEFAULT_MAX_RESULTS: usize = 200;
const HARD_MAX_RESULTS: usize = 2000;
const MAX_SCANNED: usize = 50_000;
const ROOT_READY_MARKER: &str = ".terax-root-ready";
const LINUXKIT_SHELL: &str = "/bin/ash";
const LINUXKIT_SHELL_ARG0: &str = "-ash";
const LINUXKIT_HOME: &str = "/root";
const LINUXKIT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
const OSC_ST: &str = "\x1b\\";
const PRUNE_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    ".next",
    ".turbo",
    ".cache",
    ".venv",
    "__pycache__",
];
const HIDDEN_ROOT_DIRS: &[&str] = &[
    "bin", "dev", "etc", "lib", "media", "mnt", "opt", "proc", "run", "sbin", "srv", "sys", "tmp",
    "usr", "var",
];

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
    ReadDir {
        path: String,
        show_hidden: bool,
    },
    ReadFile {
        path: String,
    },
    WriteFile {
        path: String,
        content: String,
    },
    Stat {
        path: String,
    },
    Canonicalize {
        path: String,
    },
    CreateFile {
        path: String,
    },
    CreateDir {
        path: String,
    },
    Rename {
        from: String,
        to: String,
    },
    Delete {
        path: String,
    },
    Search {
        root: String,
        query: String,
        max_results: Option<usize>,
    },
    ListFiles {
        root: String,
        max_results: Option<usize>,
    },
    Grep {
        root: String,
        pattern: String,
        glob: Option<Vec<String>>,
        case_insensitive: Option<bool>,
        max_results: Option<usize>,
    },
    Glob {
        root: String,
        pattern: String,
        max_results: Option<usize>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinuxKitResponse {
    Unit,
    U32 { value: u32 },
    Json { value: serde_json::Value },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinuxKitHealth {
    pub available: bool,
    pub version: Option<String>,
    pub message: Option<String>,
}

#[derive(Default)]
pub struct LinuxKitBackend;

impl LinuxKitBackend {
    pub fn new() -> Self {
        Self
    }

    pub fn request(&self, request: LinuxKitRequest) -> Result<LinuxKitResponse, String> {
        match request {
            LinuxKitRequest::Fs(request) => handle_fs(request),
            LinuxKitRequest::ShellRun(request) => {
                ensure_linuxkit_ash()?;
                let mut cwd = request
                    .cwd
                    .map(|path| normalize_virtual_path(&path))
                    .unwrap_or_else(default_cwd);
                let output = run_virtual_command(&request.command, &mut cwd);
                json_response(CommandOutput {
                    stdout: output.stdout,
                    stderr: output.stderr,
                    exit_code: Some(output.exit_code),
                    timed_out: false,
                    truncated: false,
                })
            }
            LinuxKitRequest::ShellSessionOpen(request) => {
                ensure_linuxkit_ash()?;
                let mut state = state().lock().unwrap();
                let id = state.next_shell_id;
                state.next_shell_id += 1;
                state.shell_sessions.insert(
                    id,
                    request
                        .cwd
                        .map(|path| normalize_virtual_path(&path))
                        .unwrap_or_else(default_cwd),
                );
                Ok(LinuxKitResponse::U32 { value: id })
            }
            LinuxKitRequest::ShellSessionRun(request) => {
                let mut state = state().lock().unwrap();
                let cwd = state
                    .shell_sessions
                    .get_mut(&request.id)
                    .ok_or_else(|| "no shell session".to_string())?;
                if let Some(hint) = request.cwd.filter(|s| !s.is_empty()) {
                    let hinted = normalize_virtual_path(&hint);
                    if real_path_for_virtual(&hinted).is_ok_and(|path| path.is_dir()) {
                        *cwd = hinted;
                    }
                }
                let output = run_virtual_command(&request.command, cwd);
                json_response(SessionRunOutput {
                    stdout: output.stdout,
                    stderr: output.stderr,
                    exit_code: Some(output.exit_code),
                    timed_out: false,
                    truncated: false,
                    cwd_after: to_canon(cwd),
                })
            }
            LinuxKitRequest::ShellSessionClose(request) => {
                state().lock().unwrap().shell_sessions.remove(&request.id);
                Ok(LinuxKitResponse::Unit)
            }
            LinuxKitRequest::BackgroundSpawn(request) => {
                ensure_linuxkit_ash()?;
                let mut cwd = request
                    .cwd
                    .clone()
                    .map(|path| normalize_virtual_path(&path))
                    .unwrap_or_else(default_cwd);
                let output = run_virtual_command(&request.command, &mut cwd);
                let mut state = state().lock().unwrap();
                let id = state.next_bg_id;
                state.next_bg_id += 1;
                let mut logs = output.stdout;
                logs.push_str(&output.stderr);
                state.bg.insert(
                    id,
                    BackgroundProc {
                        command: request.command,
                        cwd: request.cwd,
                        started_at_ms: now_ms(),
                        logs,
                        exited: true,
                        exit_code: Some(output.exit_code),
                    },
                );
                Ok(LinuxKitResponse::U32 { value: id })
            }
            LinuxKitRequest::BackgroundLogs(request) => {
                let state = state().lock().unwrap();
                let proc = state
                    .bg
                    .get(&request.handle)
                    .ok_or_else(|| "no background handle".to_string())?;
                let offset = request.since_offset.unwrap_or(0) as usize;
                let bytes = proc.logs.as_bytes();
                let start = offset.min(bytes.len());
                let chunk = String::from_utf8_lossy(&bytes[start..]).into_owned();
                json_response(BackgroundLogResponse {
                    bytes: chunk,
                    next_offset: bytes.len() as u64,
                    dropped: 0,
                    exited: proc.exited,
                    exit_code: proc.exit_code,
                })
            }
            LinuxKitRequest::BackgroundKill(request) => {
                if let Some(proc) = state().lock().unwrap().bg.get_mut(&request.handle) {
                    proc.exited = true;
                    proc.exit_code = Some(0);
                }
                Ok(LinuxKitResponse::Unit)
            }
            LinuxKitRequest::BackgroundList => {
                let state = state().lock().unwrap();
                let mut out: Vec<BackgroundProcInfo> = state
                    .bg
                    .iter()
                    .map(|(handle, proc)| BackgroundProcInfo {
                        handle: *handle,
                        command: proc.command.clone(),
                        cwd: proc.cwd.clone(),
                        started_at_ms: proc.started_at_ms,
                        exited: proc.exited,
                        exit_code: proc.exit_code,
                    })
                    .collect();
                out.sort_by_key(|proc| proc.handle);
                json_response(out)
            }
            LinuxKitRequest::PtyOpen(_) => Err("pty_open requires frontend channels".into()),
            LinuxKitRequest::PtyWrite(request) => {
                self.pty_write(request.id, request.data)?;
                Ok(LinuxKitResponse::Unit)
            }
            LinuxKitRequest::PtyResize(request) => {
                self.pty_resize(request.id, request.cols, request.rows)?;
                Ok(LinuxKitResponse::Unit)
            }
            LinuxKitRequest::PtyClose(request) => {
                self.pty_close(request.id)?;
                Ok(LinuxKitResponse::Unit)
            }
        }
    }

    pub fn pty_open(
        &self,
        cols: u16,
        rows: u16,
        cwd: Option<String>,
        on_data: Channel<Response>,
        on_exit: Channel<i32>,
    ) -> Result<u32, String> {
        ensure_linuxkit_ash()?;
        let mut state = state().lock().unwrap();
        let id = state.next_pty_id;
        state.next_pty_id += 1;
        let cwd = cwd
            .map(|path| normalize_virtual_path(&path))
            .unwrap_or_else(default_cwd);
        let initial_prompt = prompt(&cwd, 0);
        state.pty_sessions.insert(
            id,
            PtySession {
                cwd,
                input: String::new(),
                input_state: InputState::Normal,
                cols,
                rows,
                on_data: on_data.clone(),
                on_exit,
            },
        );
        drop(state);

        send_pty(
            &on_data,
            format!(
                "\x1b[2mStarting iOS LinuxKit {LINUXKIT_SHELL} -l ({cols}x{rows})\x1b[0m\r\n{initial_prompt}",
            )
            .as_bytes(),
        )?;
        Ok(id)
    }

    pub fn pty_write(&self, id: u32, data: String) -> Result<(), String> {
        let mut close = false;
        let mut on_exit = None;
        {
            let mut state = state().lock().unwrap();
            let session = state
                .pty_sessions
                .get_mut(&id)
                .ok_or_else(|| "no session".to_string())?;
            for ch in data.chars() {
                if handle_input_escape(session, ch) {
                    continue;
                }
                match ch {
                    '\r' | '\n' => {
                        send_pty(&session.on_data, b"\r\n")?;
                        let command = std::mem::take(&mut session.input);
                        if command.trim() == "exit" {
                            send_pty(&session.on_data, b"logout\r\n")?;
                            close = true;
                            on_exit = Some(session.on_exit.clone());
                            break;
                        }
                        send_pty(&session.on_data, format!("\x1b]133;C{OSC_ST}").as_bytes())?;
                        let output = run_virtual_command(&command, &mut session.cwd);
                        if !output.stdout.is_empty() {
                            send_pty(&session.on_data, output.stdout.as_bytes())?;
                        }
                        if !output.stderr.is_empty() {
                            send_pty(&session.on_data, output.stderr.as_bytes())?;
                        }
                        let prompt = prompt(&session.cwd, output.exit_code);
                        send_pty(&session.on_data, prompt.as_bytes())?;
                    }
                    '\u{3}' => {
                        session.input.clear();
                        send_pty(&session.on_data, b"^C\r\n")?;
                        let prompt = prompt(&session.cwd, 130);
                        send_pty(&session.on_data, prompt.as_bytes())?;
                    }
                    '\u{c}' => {
                        send_pty(&session.on_data, b"\x1b[2J\x1b[H")?;
                        let prompt = prompt(&session.cwd, 0);
                        send_pty(&session.on_data, prompt.as_bytes())?;
                    }
                    '\u{15}' => {
                        let removed = session.input.chars().count();
                        session.input.clear();
                        send_backspaces(&session.on_data, removed)?;
                    }
                    '\u{17}' => {
                        let removed = erase_word(&mut session.input);
                        send_backspaces(&session.on_data, removed)?;
                    }
                    '\u{8}' | '\u{7f}' => {
                        if session.input.pop().is_some() {
                            send_pty(&session.on_data, b"\x08 \x08")?;
                        }
                    }
                    ch if !ch.is_control() || ch == '\t' => {
                        session.input.push(ch);
                        let mut buf = [0; 4];
                        send_pty(&session.on_data, ch.encode_utf8(&mut buf).as_bytes())?;
                    }
                    _ => {}
                }
            }
            if close {
                state.pty_sessions.remove(&id);
            }
        }
        if let Some(channel) = on_exit {
            let _ = channel.send(0);
        }
        Ok(())
    }

    pub fn pty_resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        let mut state = state().lock().unwrap();
        let session = state
            .pty_sessions
            .get_mut(&id)
            .ok_or_else(|| "no session".to_string())?;
        session.cols = cols;
        session.rows = rows;
        Ok(())
    }

    pub fn pty_close(&self, id: u32) -> Result<(), String> {
        if let Some(session) = state().lock().unwrap().pty_sessions.remove(&id) {
            let _ = session.on_exit.send(0);
        }
        Ok(())
    }
}

struct AdapterState {
    next_pty_id: u32,
    next_shell_id: u32,
    next_bg_id: u32,
    pty_sessions: HashMap<u32, PtySession>,
    shell_sessions: HashMap<u32, PathBuf>,
    bg: HashMap<u32, BackgroundProc>,
}

impl Default for AdapterState {
    fn default() -> Self {
        Self {
            next_pty_id: 1,
            next_shell_id: 1,
            next_bg_id: 1,
            pty_sessions: HashMap::new(),
            shell_sessions: HashMap::new(),
            bg: HashMap::new(),
        }
    }
}

struct PtySession {
    cwd: PathBuf,
    input: String,
    input_state: InputState,
    cols: u16,
    rows: u16,
    on_data: Channel<Response>,
    on_exit: Channel<i32>,
}

#[derive(Clone, Copy)]
enum InputState {
    Normal,
    Esc,
    Csi,
}

struct BackgroundProc {
    command: String,
    cwd: Option<String>,
    started_at_ms: u64,
    logs: String,
    exited: bool,
    exit_code: Option<i32>,
}

fn state() -> &'static Mutex<AdapterState> {
    static STATE: OnceLock<Mutex<AdapterState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(AdapterState::default()))
}

fn json_response<T: Serialize>(value: T) -> Result<LinuxKitResponse, String> {
    Ok(LinuxKitResponse::Json {
        value: serde_json::to_value(value).map_err(|e| e.to_string())?,
    })
}

fn handle_fs(request: FsRequest) -> Result<LinuxKitResponse, String> {
    match request {
        FsRequest::ReadDir { path, show_hidden } => json_response(read_dir(&path, show_hidden)?),
        FsRequest::ReadFile { path } => json_response(read_file(&path)?),
        FsRequest::WriteFile { path, content } => {
            fs::write(real_path(&path)?, content).map_err(|e| e.to_string())?;
            Ok(LinuxKitResponse::Unit)
        }
        FsRequest::Stat { path } => json_response(stat(&path)?),
        FsRequest::Canonicalize { path } => {
            let canon = fs::canonicalize(real_path(&path)?).map_err(|e| e.to_string())?;
            json_response(virtual_path_for_real(&canon)?)
        }
        FsRequest::CreateFile { path } => {
            let path = real_path(&path)?;
            if path.exists() {
                return Err(format!("already exists: {}", path.display()));
            }
            fs::write(path, "").map_err(|e| e.to_string())?;
            Ok(LinuxKitResponse::Unit)
        }
        FsRequest::CreateDir { path } => {
            let path = real_path(&path)?;
            if path.exists() {
                return Err(format!("already exists: {}", path.display()));
            }
            fs::create_dir_all(path).map_err(|e| e.to_string())?;
            Ok(LinuxKitResponse::Unit)
        }
        FsRequest::Rename { from, to } => {
            let from = real_path(&from)?;
            let to = real_path(&to)?;
            if !from.exists() {
                return Err(format!("not found: {}", from.display()));
            }
            if to.exists() {
                return Err(format!("already exists: {}", to.display()));
            }
            fs::rename(from, to).map_err(|e| e.to_string())?;
            Ok(LinuxKitResponse::Unit)
        }
        FsRequest::Delete { path } => {
            let path = real_path(&path)?;
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.is_dir() {
                fs::remove_dir_all(path).map_err(|e| e.to_string())?;
            } else {
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            Ok(LinuxKitResponse::Unit)
        }
        FsRequest::Search {
            root,
            query,
            max_results,
        } => json_response(search(&root, &query, max_results)?),
        FsRequest::ListFiles { root, max_results } => {
            json_response(list_files(&root, max_results)?)
        }
        FsRequest::Grep {
            root,
            pattern,
            glob,
            case_insensitive,
            max_results,
        } => json_response(grep(&root, &pattern, glob, case_insensitive, max_results)?),
        FsRequest::Glob {
            root,
            pattern,
            max_results,
        } => json_response(glob(&root, &pattern, max_results)?),
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum ReadResult {
    Text { content: String, size: u64 },
    Binary { size: u64 },
    TooLarge { size: u64, limit: u64 },
}

fn read_file(path: &str) -> Result<ReadResult, String> {
    let path = real_path(path)?;
    let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
    let size = meta.len();
    if size > MAX_READ_BYTES {
        return Ok(ReadResult::TooLarge {
            size,
            limit: MAX_READ_BYTES,
        });
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let sniff_len = bytes.len().min(BINARY_SNIFF_BYTES);
    if bytes[..sniff_len].contains(&0) {
        return Ok(ReadResult::Binary { size });
    }
    match String::from_utf8(bytes) {
        Ok(content) => Ok(ReadResult::Text { content, size }),
        Err(_) => Ok(ReadResult::Binary { size }),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum EntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Serialize)]
struct DirEntry {
    name: String,
    kind: EntryKind,
    size: u64,
    mtime: u64,
}

fn read_dir(path: &str, show_hidden: bool) -> Result<Vec<DirEntry>, String> {
    let virtual_path = normalize_virtual_path(path);
    let mut entries: Vec<DirEntry> = fs::read_dir(real_path_for_virtual(&virtual_path)?)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if name.starts_with('.') && !show_hidden {
                return None;
            }
            if is_hidden_root_entry(&virtual_path, &name) {
                return None;
            }
            let meta = fs::symlink_metadata(entry.path()).ok()?;
            let kind = if meta.file_type().is_symlink() {
                EntryKind::Symlink
            } else if meta.is_dir() {
                EntryKind::Dir
            } else {
                EntryKind::File
            };
            Some(DirEntry {
                name,
                kind,
                size: meta.len(),
                mtime: modified_ms(&meta),
            })
        })
        .collect();
    entries.sort_by(|a, b| {
        let rank = |kind: &EntryKind| match kind {
            EntryKind::Dir => 0,
            EntryKind::Symlink => 1,
            EntryKind::File => 2,
        };
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

#[derive(Serialize)]
struct FileStat {
    size: u64,
    mtime: u64,
    kind: EntryKind,
}

fn stat(path: &str) -> Result<FileStat, String> {
    let meta = fs::symlink_metadata(real_path(path)?).map_err(|e| e.to_string())?;
    let kind = if meta.file_type().is_symlink() {
        EntryKind::Symlink
    } else if meta.is_dir() {
        EntryKind::Dir
    } else {
        EntryKind::File
    };
    Ok(FileStat {
        size: meta.len(),
        mtime: modified_ms(&meta),
        kind,
    })
}

#[derive(Serialize)]
struct SearchHit {
    path: String,
    rel: String,
    name: String,
    is_dir: bool,
}

#[derive(Serialize)]
struct SearchResult {
    hits: Vec<SearchHit>,
    truncated: bool,
}

fn search(root: &str, query: &str, limit: Option<usize>) -> Result<SearchResult, String> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(SearchResult {
            hits: Vec::new(),
            truncated: false,
        });
    }
    let root_virtual = normalize_virtual_path(root);
    let root_path = real_path_for_virtual(&root_virtual)?;
    if !root_path.is_dir() {
        return Err(format!("not a directory: {root}"));
    }
    let cap = limit.unwrap_or(200).min(1000);
    let mut hits = Vec::with_capacity(cap.min(64));
    let mut scanned = 0usize;
    let mut truncated = false;
    let walker = build_walker(&root_path, &root_virtual, None).build();
    for dent in walker.flatten() {
        scanned += 1;
        if scanned > MAX_SCANNED || hits.len() >= cap {
            truncated = true;
            break;
        }
        let path = dent.path();
        if path == root_path {
            continue;
        }
        let rel = match path.strip_prefix(&root_path) {
            Ok(path) => to_canon(path),
            Err(_) => continue,
        };
        if !rel.to_lowercase().contains(&q) {
            continue;
        }
        hits.push(SearchHit {
            path: virtual_join_rel(&root_virtual, &rel),
            rel,
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            is_dir: dent.file_type().map(|kind| kind.is_dir()).unwrap_or(false),
        });
    }
    hits.sort_by(|a, b| {
        let an = a.name.to_lowercase().contains(&q);
        let bn = b.name.to_lowercase().contains(&q);
        bn.cmp(&an).then(a.rel.len().cmp(&b.rel.len()))
    });
    Ok(SearchResult { hits, truncated })
}

#[derive(Serialize)]
struct ListFilesResult {
    files: Vec<String>,
    truncated: bool,
}

fn list_files(root: &str, limit: Option<usize>) -> Result<ListFilesResult, String> {
    let root_virtual = normalize_virtual_path(root);
    let root_path = real_path_for_virtual(&root_virtual)?;
    if !root_path.is_dir() {
        return Err(format!("not a directory: {root}"));
    }
    let cap = limit.unwrap_or(2_000).clamp(1, 10_000);
    let mut files = Vec::with_capacity(cap.min(256));
    let mut scanned = 0usize;
    let mut truncated = false;
    let walker = build_walker(&root_path, &root_virtual, Some(8)).build();
    for dent in walker.flatten() {
        scanned += 1;
        if scanned > MAX_SCANNED {
            truncated = true;
            break;
        }
        if !dent.file_type().map(|kind| kind.is_file()).unwrap_or(false) {
            continue;
        }
        let rel = match dent.path().strip_prefix(&root_path) {
            Ok(path) => to_canon(path),
            Err(_) => continue,
        };
        if !rel.is_empty() {
            files.push(rel);
        }
        if files.len() >= cap {
            truncated = true;
            break;
        }
    }
    files.sort_by_key(|path| path.to_lowercase());
    Ok(ListFilesResult { files, truncated })
}

#[derive(Serialize)]
struct GrepHit {
    path: String,
    rel: String,
    line: u64,
    text: String,
}

#[derive(Serialize)]
struct GrepResponse {
    hits: Vec<GrepHit>,
    truncated: bool,
    files_scanned: usize,
}

fn grep(
    root: &str,
    pattern: &str,
    glob: Option<Vec<String>>,
    case_insensitive: Option<bool>,
    max_results: Option<usize>,
) -> Result<GrepResponse, String> {
    if pattern.is_empty() {
        return Err("empty pattern".into());
    }
    let root_virtual = normalize_virtual_path(root);
    let root_path = real_path_for_virtual(&root_virtual)?;
    if !root_path.is_dir() {
        return Err(format!("not a directory: {root}"));
    }
    let cap = max_results
        .unwrap_or(DEFAULT_MAX_RESULTS)
        .clamp(1, HARD_MAX_RESULTS);
    let matcher = RegexMatcherBuilder::new()
        .case_insensitive(case_insensitive.unwrap_or(false))
        .build(pattern)
        .map_err(|e| format!("bad regex: {e}"))?;
    let globs = build_globset(glob.as_deref().unwrap_or(&[]))?;
    let mut hits = Vec::new();
    let mut files_scanned = 0usize;
    let mut truncated = false;
    let walker = build_walker(&root_path, &root_virtual, None).build();
    for dent in walker.flatten() {
        if hits.len() >= cap {
            truncated = true;
            break;
        }
        if !dent.file_type().map(|kind| kind.is_file()).unwrap_or(false) {
            continue;
        }
        let path = dent.path();
        let rel = match path.strip_prefix(&root_path) {
            Ok(path) => to_canon(path),
            Err(_) => continue,
        };
        if let Some(set) = globs.as_ref() {
            if !set.is_match(&rel) {
                continue;
            }
        }
        if fs::metadata(path)
            .map(|meta| meta.len() > FILE_SIZE_CAP)
            .unwrap_or(true)
        {
            continue;
        }
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(_) => continue,
        };
        files_scanned += 1;
        for (idx, line) in text.lines().enumerate() {
            if matcher.is_match(line.as_bytes()).unwrap_or(false) {
                hits.push(GrepHit {
                    path: virtual_join_rel(&root_virtual, &rel),
                    rel: rel.clone(),
                    line: (idx + 1) as u64,
                    text: line.to_string(),
                });
                if hits.len() >= cap {
                    truncated = true;
                    break;
                }
            }
        }
    }
    Ok(GrepResponse {
        hits,
        truncated,
        files_scanned,
    })
}

#[derive(Serialize)]
struct GlobHit {
    path: String,
    rel: String,
}

#[derive(Serialize)]
struct GlobResponse {
    hits: Vec<GlobHit>,
    truncated: bool,
}

fn glob(root: &str, pattern: &str, max_results: Option<usize>) -> Result<GlobResponse, String> {
    if pattern.is_empty() {
        return Err("empty pattern".into());
    }
    let root_virtual = normalize_virtual_path(root);
    let root_path = real_path_for_virtual(&root_virtual)?;
    if !root_path.is_dir() {
        return Err(format!("not a directory: {root}"));
    }
    let mut gb = GlobSetBuilder::new();
    gb.add(Glob::new(pattern).map_err(|e| format!("bad glob: {e}"))?);
    let set = gb.build().map_err(|e| format!("globset build: {e}"))?;
    let cap = max_results.unwrap_or(500).clamp(1, HARD_MAX_RESULTS);
    let mut hits = Vec::new();
    let mut truncated = false;
    let walker = build_walker(&root_path, &root_virtual, None).build();
    for dent in walker.flatten() {
        if hits.len() >= cap {
            truncated = true;
            break;
        }
        if !dent.file_type().map(|kind| kind.is_file()).unwrap_or(false) {
            continue;
        }
        let rel = match dent.path().strip_prefix(&root_path) {
            Ok(path) => to_canon(path),
            Err(_) => continue,
        };
        if set.is_match(&rel) {
            hits.push(GlobHit {
                path: virtual_join_rel(&root_virtual, &rel),
                rel,
            });
        }
    }
    Ok(GlobResponse { hits, truncated })
}

fn build_walker(root: &Path, root_virtual: &Path, depth: Option<usize>) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    let hide_root_system = is_virtual_root(root_virtual);
    builder
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .ignore(true)
        .parents(true)
        .follow_links(false)
        .filter_entry(move |dent| {
            if dent.depth() == 0 {
                return true;
            }
            if hide_root_system && dent.depth() == 1 {
                if let Some(name) = dent.file_name().to_str() {
                    return !HIDDEN_ROOT_DIRS.contains(&name);
                }
            }
            dent.file_name()
                .to_str()
                .map(|name| !PRUNE_DIRS.contains(&name))
                .unwrap_or(true)
        });
    if let Some(depth) = depth {
        builder.max_depth(Some(depth));
    }
    builder
}

fn build_globset(patterns: &[String]) -> Result<Option<GlobSet>, String> {
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).map_err(|e| format!("bad glob {pattern:?}: {e}"))?);
    }
    Ok(Some(
        builder.build().map_err(|e| format!("globset build: {e}"))?,
    ))
}

#[derive(Serialize)]
struct CommandOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    timed_out: bool,
    truncated: bool,
}

#[derive(Serialize)]
struct SessionRunOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    timed_out: bool,
    truncated: bool,
    cwd_after: String,
}

#[derive(Serialize)]
struct BackgroundLogResponse {
    bytes: String,
    next_offset: u64,
    dropped: u64,
    exited: bool,
    exit_code: Option<i32>,
}

#[derive(Serialize)]
struct BackgroundProcInfo {
    handle: u32,
    command: String,
    cwd: Option<String>,
    started_at_ms: u64,
    exited: bool,
    exit_code: Option<i32>,
}

struct VirtualOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

fn run_virtual_command(command: &str, cwd: &mut PathBuf) -> VirtualOutput {
    let mut final_output = VirtualOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
    };
    for line in command
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let output = run_virtual_line(line, cwd);
        final_output.stdout.push_str(&output.stdout);
        final_output.stderr.push_str(&output.stderr);
        final_output.exit_code = output.exit_code;
        if output.exit_code != 0 {
            break;
        }
    }
    final_output
}

fn run_virtual_line(line: &str, cwd: &mut PathBuf) -> VirtualOutput {
    let tokens = split_command(line);
    if tokens.is_empty() {
        return VirtualOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        };
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code = 0;
    match tokens[0].as_str() {
        "pwd" => {
            stdout.push_str(&to_canon(cwd));
            stdout.push('\n');
        }
        "cd" => {
            let target = tokens.get(1).map(String::as_str).unwrap_or("~");
            let path = resolve_shell_path(target, cwd);
            if real_path_for_virtual(&path).is_ok_and(|path| path.is_dir()) {
                *cwd = normalize_virtual_path(path.to_string_lossy().as_ref());
            } else {
                stderr.push_str(&format!("cd: no such directory: {target}\n"));
                exit_code = 1;
            }
        }
        "ls" => {
            let show_all = tokens.iter().any(|token| token.contains('a') && token.starts_with('-'));
            let target = tokens
                .iter()
                .skip(1)
                .find(|token| !token.starts_with('-'))
                .map(String::as_str)
                .unwrap_or(".");
            let path = resolve_shell_path(target, cwd);
            match read_dir(&to_canon(&path), show_all) {
                Ok(entries) => {
                    for entry in entries {
                        stdout.push_str(&entry.name);
                        if matches!(entry.kind, EntryKind::Dir) {
                            stdout.push('/');
                        }
                        stdout.push('\n');
                    }
                }
                Err(e) => {
                    stderr.push_str(&format!("ls: {e}\n"));
                    exit_code = 1;
                }
            }
        }
        "cat" => {
            for arg in tokens.iter().skip(1) {
                let path = resolve_shell_path(arg, cwd);
                match real_path_for_virtual(&path).and_then(|path| {
                    fs::read_to_string(&path)
                        .map_err(|e| format!("{}: {e}", virtual_path_for_real_lossy(&path)))
                }) {
                    Ok(content) => stdout.push_str(&content),
                    Err(e) => {
                        stderr.push_str(&format!("cat: {e}\n"));
                        exit_code = 1;
                    }
                }
            }
        }
        "echo" => {
            let expanded = tokens
                .iter()
                .skip(1)
                .map(|token| expand_shell_word(token, cwd))
                .collect::<Vec<_>>()
                .join(" ");
            stdout.push_str(&expanded);
            stdout.push('\n');
        }
        "ash" | "/bin/ash" => {
            stdout.push_str("ash: already running as login shell\r\n");
        }
        "command" if tokens.get(1).map(String::as_str) == Some("-v") => {
            match tokens.get(2).map(String::as_str) {
                Some("ash") => stdout.push_str("/bin/ash\n"),
                Some(other) => {
                    stderr.push_str(&format!("{other}: not found\n"));
                    exit_code = 127;
                }
                None => {
                    stderr.push_str("command: usage: command -v name\n");
                    exit_code = 2;
                }
            }
        }
        "which" => {
            for name in tokens.iter().skip(1) {
                if name == "ash" {
                    stdout.push_str("/bin/ash\n");
                } else {
                    stderr.push_str(&format!("{name}: not found\n"));
                    exit_code = 1;
                }
            }
        }
        "printenv" => {
            if let Some(name) = tokens.get(1) {
                if let Some(value) = shell_env_value(name, cwd) {
                    stdout.push_str(&value);
                    stdout.push('\n');
                } else {
                    exit_code = 1;
                }
            } else {
                stdout.push_str(&format!(
                    "SHELL={LINUXKIT_SHELL}\nHOME={LINUXKIT_HOME}\nPATH={LINUXKIT_PATH}\nPWD={}\n",
                    to_canon(cwd)
                ));
            }
        }
        "mkdir" => {
            let paths = tokens.iter().skip(1).filter(|token| !token.starts_with('-'));
            for arg in paths {
                let path = resolve_shell_path(arg, cwd);
                if let Err(e) = real_path_for_virtual(&path).and_then(|path| {
                    fs::create_dir_all(&path)
                        .map_err(|e| format!("{}: {e}", virtual_path_for_real_lossy(&path)))
                }) {
                    stderr.push_str(&format!("mkdir: {arg}: {e}\n"));
                    exit_code = 1;
                }
            }
        }
        "touch" => {
            for arg in tokens.iter().skip(1) {
                let path = resolve_shell_path(arg, cwd);
                if let Err(e) = real_path_for_virtual(&path).and_then(|path| {
                    fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                        .map(|_| ())
                        .map_err(|e| format!("{}: {e}", virtual_path_for_real_lossy(&path)))
                }) {
                    stderr.push_str(&format!("touch: {e}\n"));
                    exit_code = 1;
                }
            }
        }
        "rm" => {
            let recursive = tokens
                .iter()
                .any(|token| token.starts_with('-') && token.contains('r'));
            for arg in tokens.iter().skip(1).filter(|token| !token.starts_with('-')) {
                let path = resolve_shell_path(arg, cwd);
                if let Err(e) = real_path_for_virtual(&path).and_then(|real| {
                    let result = if real.is_dir() && recursive {
                        fs::remove_dir_all(&real)
                    } else {
                        fs::remove_file(&real)
                    };
                    result.map_err(|e| format!("{}: {e}", virtual_path_for_real_lossy(&real)))
                }) {
                    stderr.push_str(&format!("rm: {e}\n"));
                    exit_code = 1;
                }
            }
        }
        "rmdir" => {
            for arg in tokens.iter().skip(1) {
                let path = resolve_shell_path(arg, cwd);
                if let Err(e) = real_path_for_virtual(&path).and_then(|real| {
                    fs::remove_dir(&real)
                        .map_err(|e| format!("{}: {e}", virtual_path_for_real_lossy(&real)))
                }) {
                    stderr.push_str(&format!("rmdir: {e}\n"));
                    exit_code = 1;
                }
            }
        }
        "clear" => stdout.push_str("\x1b[2J\x1b[H"),
        "uname" => stdout.push_str("Linux ios-linuxkit 6.1.0-ish aarch64 Linux\n"),
        "whoami" => stdout.push_str("root\n"),
        "help" => stdout.push_str(
            "iOS LinuxKit ash adapter. Available commands: ash, cat, cd, clear, command -v, echo, help, ls, mkdir, printenv, pwd, rm, rmdir, touch, uname, which, whoami\n",
        ),
        other => {
            stderr.push_str(&format!(
                "{other}: command not available in the mobile LinuxKit adapter\n"
            ));
            exit_code = 127;
        }
    }
    VirtualOutput {
        stdout: stdout.replace('\n', "\r\n"),
        stderr: stderr.replace('\n', "\r\n"),
        exit_code,
    }
}

fn split_command(command: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ch if ch.is_whitespace() => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

fn resolve_shell_path(raw: &str, cwd: &Path) -> PathBuf {
    if raw == "~" {
        return default_cwd();
    }
    if let Some(suffix) = raw.strip_prefix("~/") {
        return normalize_virtual_path(&format!("/root/{suffix}"));
    }
    if raw.starts_with('/') {
        normalize_virtual_path(raw)
    } else {
        normalize_virtual_path(&format!("{}/{}", to_canon(cwd).trim_end_matches('/'), raw))
    }
}

fn expand_shell_word(raw: &str, cwd: &Path) -> String {
    match raw {
        "$0" => LINUXKIT_SHELL_ARG0.into(),
        "$SHELL" => LINUXKIT_SHELL.into(),
        "$HOME" => LINUXKIT_HOME.into(),
        "$PATH" => LINUXKIT_PATH.into(),
        "$PWD" => to_canon(cwd),
        _ => raw.to_string(),
    }
}

fn shell_env_value(name: &str, cwd: &Path) -> Option<String> {
    match name {
        "SHELL" => Some(LINUXKIT_SHELL.into()),
        "HOME" => Some(LINUXKIT_HOME.into()),
        "PATH" => Some(LINUXKIT_PATH.into()),
        "PWD" => Some(to_canon(cwd)),
        _ => None,
    }
}

fn handle_input_escape(session: &mut PtySession, ch: char) -> bool {
    match session.input_state {
        InputState::Normal => {
            if ch == '\x1b' {
                session.input_state = InputState::Esc;
                true
            } else {
                false
            }
        }
        InputState::Esc => {
            if ch == '[' {
                session.input_state = InputState::Csi;
                true
            } else {
                session.input_state = InputState::Normal;
                false
            }
        }
        InputState::Csi => {
            if ('\u{40}'..='\u{7e}').contains(&ch) {
                session.input_state = InputState::Normal;
            }
            true
        }
    }
}

fn erase_word(input: &mut String) -> usize {
    let before = input.chars().count();
    while input.chars().last().is_some_and(|ch| ch.is_whitespace()) {
        input.pop();
    }
    while input.chars().last().is_some_and(|ch| !ch.is_whitespace()) {
        input.pop();
    }
    before.saturating_sub(input.chars().count())
}

fn send_backspaces(channel: &Channel<Response>, count: usize) -> Result<(), String> {
    if count == 0 {
        return Ok(());
    }
    let mut bytes = Vec::with_capacity(count * 3);
    for _ in 0..count {
        bytes.extend_from_slice(b"\x08 \x08");
    }
    send_pty(channel, &bytes)
}

fn ensure_linuxkit_ash() -> Result<PathBuf, String> {
    let ash = real_path_for_virtual(&PathBuf::from(LINUXKIT_SHELL))?;
    if ash.exists() {
        Ok(ash)
    } else {
        Err(format!(
            "embedded ios-linuxkit shell is missing: {LINUXKIT_SHELL}"
        ))
    }
}

fn prompt(cwd: &Path, status: i32) -> String {
    format!(
        "\x1b]133;D;{status}{OSC_ST}\x1b]7;file://localhost{}{OSC_ST}\x1b]133;A{OSC_ST}root@ios-linuxkit:{}# \x1b]133;B{OSC_ST}",
        encode_file_uri_path(&to_canon(cwd)),
        cwd.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("/")
    )
}

fn encode_file_uri_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'.' | b'_' | b'~' | b'-' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push(hex(byte >> 4));
                out.push(hex(byte & 0x0f));
            }
        }
    }
    out
}

fn hex(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'A' + (n - 10)) as char,
    }
}

fn send_pty(channel: &Channel<Response>, bytes: &[u8]) -> Result<(), String> {
    channel
        .send(Response::new(bytes.to_vec()))
        .map_err(|e| e.to_string())
}

fn default_cwd() -> PathBuf {
    PathBuf::from("/root")
}

fn linuxkit_root_dir() -> Result<PathBuf, String> {
    static ROOT: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    ROOT.get_or_init(init_linuxkit_root_dir).clone()
}

fn init_linuxkit_root_dir() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("TERAX_IOS_LINUXKIT_ROOT") {
        let path = PathBuf::from(path);
        if path.is_dir() {
            return Ok(path);
        }
    }

    let target = dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("Terax")
        .join("ios-linuxkit-root");
    let marker = target.join(ROOT_READY_MARKER);
    if marker.exists() {
        rewrite_absolute_symlinks(&target)?;
        return Ok(target);
    }

    let tar_path = locate_root_tar()?;
    if target.exists() {
        fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(&target).map_err(|e| e.to_string())?;
    let file = fs::File::open(&tar_path).map_err(|e| e.to_string())?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(&target).map_err(|e| e.to_string())?;
    rewrite_absolute_symlinks(&target)?;
    fs::write(&marker, b"ready").map_err(|e| e.to_string())?;
    Ok(target)
}

fn rewrite_absolute_symlinks(root: &Path) -> Result<(), String> {
    fn visit(root: &Path, dir: &Path) -> Result<(), String> {
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            if meta.file_type().is_symlink() {
                let target = fs::read_link(&path).map_err(|e| e.to_string())?;
                if target.is_absolute() {
                    let embedded_target = root.join(target.strip_prefix("/").unwrap_or(&target));
                    fs::remove_file(&path).map_err(|e| e.to_string())?;
                    std::os::unix::fs::symlink(&embedded_target, &path)
                        .map_err(|e| e.to_string())?;
                }
            } else if meta.is_dir() {
                visit(root, &path)?;
            }
        }
        Ok(())
    }

    visit(root, root)
}

fn locate_root_tar() -> Result<PathBuf, String> {
    if let Ok(path) = std::env::var("TERAX_IOS_LINUXKIT_ROOT_TAR") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }

    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("resources/ios-linuxkit/root.tar.gz"));
            candidates.push(parent.join("assets/resources/ios-linuxkit/root.tar.gz"));
            candidates.push(parent.join("ios-linuxkit/root.tar.gz"));
            candidates.push(parent.join("assets/ios-linuxkit/root.tar.gz"));
            candidates.push(parent.join("root.tar.gz"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for ancestor in cwd.ancestors() {
            candidates.push(ancestor.join("src-tauri/resources/ios-linuxkit/root.tar.gz"));
            candidates.push(ancestor.join("resources/ios-linuxkit/root.tar.gz"));
        }
    }

    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| "embedded ios-linuxkit root.tar.gz was not found".to_string())
}

fn real_path(path: &str) -> Result<PathBuf, String> {
    real_path_for_virtual(&normalize_virtual_path(path))
}

fn real_path_for_virtual(path: &Path) -> Result<PathBuf, String> {
    let root = linuxkit_root_dir()?;
    let virtual_path = normalize_virtual_path(&to_canon(path));
    let rel = to_canon(&virtual_path);
    let rel = rel.trim_start_matches('/');
    Ok(if rel.is_empty() { root } else { root.join(rel) })
}

fn normalize_virtual_path(raw: &str) -> PathBuf {
    let raw = raw.replace('\\', "/");
    if raw.is_empty()
        || raw.starts_with("/private/var/")
        || raw.contains("/Containers/Data/Application/")
    {
        return default_cwd();
    }

    let mut parts: Vec<&str> = Vec::new();
    for part in raw.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    if parts.is_empty() {
        PathBuf::from("/")
    } else {
        PathBuf::from(format!("/{}", parts.join("/")))
    }
}

fn virtual_path_for_real(real: &Path) -> Result<String, String> {
    let root = linuxkit_root_dir()?;
    let rel = real.strip_prefix(&root).map_err(|_| {
        format!(
            "path escaped embedded ios-linuxkit root: {}",
            real.to_string_lossy()
        )
    })?;
    let rel = to_canon(rel);
    Ok(if rel.is_empty() {
        "/".to_string()
    } else {
        format!("/{rel}")
    })
}

fn virtual_path_for_real_lossy(real: &Path) -> String {
    virtual_path_for_real(real).unwrap_or_else(|_| to_canon(real))
}

fn virtual_join_rel(root: &Path, rel: &str) -> String {
    let root = to_canon(root);
    if rel.is_empty() {
        return root;
    }
    if root == "/" {
        format!("/{rel}")
    } else {
        format!("{}/{rel}", root.trim_end_matches('/'))
    }
}

fn is_virtual_root(path: &Path) -> bool {
    to_canon(path) == "/"
}

fn is_hidden_root_entry(parent: &Path, name: &str) -> bool {
    is_virtual_root(parent) && HIDDEN_ROOT_DIRS.contains(&name)
}

fn modified_ms(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn to_canon(path: impl AsRef<Path>) -> String {
    path.as_ref().to_string_lossy().replace('\\', "/")
}

#[tauri::command]
pub fn linuxkit_health() -> LinuxKitHealth {
    match ensure_linuxkit_ash() {
        Ok(_) => LinuxKitHealth {
            available: true,
            version: Some("ios-linuxkit-adapter".into()),
            message: Some("embedded ios-linuxkit ash is ready".into()),
        },
        Err(e) => LinuxKitHealth {
            available: false,
            version: Some("ios-linuxkit-adapter".into()),
            message: Some(e),
        },
    }
}
