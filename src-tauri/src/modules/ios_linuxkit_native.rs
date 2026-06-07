use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::ipc::{Channel, Response};

use crate::modules::linuxkit::linuxkit_root_dir;

const LINUXKIT_SHELL: &str = "/bin/ash";
const COMMAND_TIMEOUT_SECS: u64 = 30;
const LINUXKIT_ENV: &[&str] = &[
    "TERM=xterm-256color",
    "HOME=/root",
    "USER=root",
    "LOGNAME=root",
    "SHELL=/bin/ash",
    "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
];

type OutputCallback = extern "C" fn(*mut c_void, *const u8, usize);
type ExitCallback = extern "C" fn(*mut c_void, i32);
type LogCallback = extern "C" fn(*const c_char, usize);

extern "C" {
    fn terax_linuxkit_boot(root_path: *const c_char) -> c_int;
    fn terax_linuxkit_set_log_callback(callback: LogCallback);
    fn terax_linuxkit_import_root_tar(
        archive_path: *const c_char,
        root_path: *const c_char,
        error_out: *mut c_char,
        error_len: usize,
    ) -> c_int;
    fn terax_linuxkit_start_session(
        exe: *const c_char,
        argv: *const *const c_char,
        envp: *const *const c_char,
        output: OutputCallback,
        exit: ExitCallback,
        user: *mut c_void,
        terminal_out: *mut *mut c_void,
        pid_out: *mut c_int,
    ) -> c_int;
    fn terax_linuxkit_terminal_send(terminal: *mut c_void, data: *const u8, len: usize);
    fn terax_linuxkit_terminal_resize(terminal: *mut c_void, cols: c_int, rows: c_int);
    fn terax_linuxkit_terminal_close(terminal: *mut c_void);
}

pub struct NativePtySession {
    terminal: usize,
    closed: bool,
}

pub struct NativeCommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub cwd_after: String,
}

struct NativePtyContext {
    on_data: Channel<Response>,
    on_exit: Channel<i32>,
    closed: AtomicBool,
    exited: AtomicBool,
}

struct NativeCommandContext {
    state: Mutex<NativeCommandState>,
    ready: Condvar,
}

#[derive(Default)]
struct NativeCommandState {
    output: Vec<u8>,
    exited: bool,
    exit_code: i32,
}

unsafe impl Send for NativePtySession {}

pub fn import_root_tar(archive_path: &str, root_path: &str) -> Result<(), String> {
    let archive_path = CString::new(archive_path).map_err(|e| e.to_string())?;
    let root_path = CString::new(root_path).map_err(|e| e.to_string())?;
    let mut error = vec![0 as c_char; 1024];
    let status = unsafe {
        terax_linuxkit_import_root_tar(
            archive_path.as_ptr(),
            root_path.as_ptr(),
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if status == 0 {
        return Ok(());
    }
    let end = error
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(error.len());
    let message = if end == 0 {
        "fakefs import failed".to_string()
    } else {
        let bytes = error[..end]
            .iter()
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>();
        String::from_utf8_lossy(&bytes).to_string()
    };
    Err(format!("ios-linuxkit root import failed: {message}"))
}

impl NativePtySession {
    pub fn open(
        cols: u16,
        rows: u16,
        on_data: Channel<Response>,
        on_exit: Channel<i32>,
    ) -> Result<Self, String> {
        boot()?;

        // Launch the login shell directly instead of `/bin/login -f root`:
        // /bin/login's setuid/PAM/utmp work crashes (SIGSEGV, exit code 11)
        // under the emulator. A leading-dash arg0 makes ash a login shell; the
        // session env (HOME/USER/SHELL/PATH) is set below. This matches how the
        // rest of the codebase launches the guest shell (LINUXKIT_SHELL_ARG0).
        let exe = CString::new(LINUXKIT_SHELL).map_err(|e| e.to_string())?;
        let arg0 = CString::new("-ash").map_err(|e| e.to_string())?;
        let argv = [arg0.as_ptr(), ptr::null()];
        let env = LINUXKIT_ENV
            .iter()
            .map(|value| CString::new(*value).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut envp = env.iter().map(|value| value.as_ptr()).collect::<Vec<_>>();
        envp.push(ptr::null());

        let context = Box::into_raw(Box::new(NativePtyContext {
            on_data,
            on_exit,
            closed: AtomicBool::new(false),
            exited: AtomicBool::new(false),
        }));

        let mut terminal = ptr::null_mut();
        let mut pid = 0;
        log::info!("[ios-terminal] native pty start {LINUXKIT_SHELL} (-ash login shell)");
        let status = unsafe {
            terax_linuxkit_start_session(
                exe.as_ptr(),
                argv.as_ptr(),
                envp.as_ptr(),
                on_native_output,
                on_native_exit,
                context.cast(),
                &mut terminal,
                &mut pid,
            )
        };

        if status < 0 || terminal.is_null() {
            unsafe {
                drop(Box::from_raw(context));
            }
            return Err(format!("ios-linuxkit session failed: {status}"));
        }

        let session = Self {
            terminal: terminal as usize,
            closed: false,
        };
        session.resize(cols, rows);
        Ok(session)
    }

    pub fn run_command(
        command: &str,
        cwd: &str,
        timeout_secs: Option<u64>,
    ) -> Result<NativeCommandOutput, String> {
        boot()?;

        let sentinel = format!(
            "__TERAX_DONE_{}__",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        );
        let script = format!(
            "cd -- {} || exit $?\n{}\n__terax_status=$?\nprintf '\\n{}:%s:%s\\n' \"$__terax_status\" \"$PWD\"\nexit \"$__terax_status\"\n",
            shell_quote(cwd),
            command,
            sentinel,
        );

        let exe = CString::new(LINUXKIT_SHELL).map_err(|e| e.to_string())?;
        let arg0 = CString::new("ash").map_err(|e| e.to_string())?;
        let arg1 = CString::new("-lc").map_err(|e| e.to_string())?;
        let arg2 = CString::new(script).map_err(|e| e.to_string())?;
        let argv = [arg0.as_ptr(), arg1.as_ptr(), arg2.as_ptr(), ptr::null()];
        let env = LINUXKIT_ENV
            .iter()
            .map(|value| CString::new(*value).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut envp = env.iter().map(|value| value.as_ptr()).collect::<Vec<_>>();
        envp.push(ptr::null());

        let context = Box::into_raw(Box::new(NativeCommandContext {
            state: Mutex::new(NativeCommandState::default()),
            ready: Condvar::new(),
        }));

        let mut terminal = ptr::null_mut();
        let mut pid = 0;
        let status = unsafe {
            terax_linuxkit_start_session(
                exe.as_ptr(),
                argv.as_ptr(),
                envp.as_ptr(),
                on_command_output,
                on_command_exit,
                context.cast(),
                &mut terminal,
                &mut pid,
            )
        };

        if status < 0 || terminal.is_null() {
            unsafe {
                drop(Box::from_raw(context));
            }
            return Err(format!("ios-linuxkit command failed: {status}"));
        }

        let timeout = Duration::from_secs(timeout_secs.unwrap_or(COMMAND_TIMEOUT_SECS).max(1));
        let deadline = Instant::now() + timeout;
        let mut timed_out = false;
        let mut state = unsafe { &*context }.state.lock().unwrap();
        loop {
            if state.exited || contains_bytes(&state.output, sentinel.as_bytes()) {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                timed_out = true;
                break;
            }
            let wait = deadline.saturating_duration_since(now);
            let (next_state, wait_result) = unsafe { &*context }
                .ready
                .wait_timeout(state, wait)
                .unwrap();
            state = next_state;
            if wait_result.timed_out() {
                timed_out = true;
                break;
            }
        }

        if timed_out {
            unsafe {
                terax_linuxkit_terminal_close(terminal);
            }
        }

        let raw = String::from_utf8_lossy(&state.output).replace("\r\n", "\n");
        let fallback_code = state.exit_code;
        drop(state);
        unsafe {
            terax_linuxkit_terminal_close(terminal);
            drop(Box::from_raw(context));
        }

        let (stdout, exit_code, cwd_after) = parse_command_output(&raw, &sentinel)
            .unwrap_or_else(|| (raw, fallback_code, cwd.to_string()));

        Ok(NativeCommandOutput {
            stdout,
            stderr: String::new(),
            exit_code,
            timed_out,
            cwd_after,
        })
    }

    pub fn write(&self, data: &[u8]) {
        let terminal = self.terminal as *mut c_void;
        if terminal.is_null() || data.is_empty() {
            return;
        }
        unsafe {
            terax_linuxkit_terminal_send(terminal, data.as_ptr(), data.len());
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let terminal = self.terminal as *mut c_void;
        if terminal.is_null() {
            return;
        }
        unsafe {
            terax_linuxkit_terminal_resize(terminal, cols.into(), rows.into());
        }
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let terminal = self.terminal as *mut c_void;
        if !terminal.is_null() {
            unsafe {
                terax_linuxkit_terminal_close(terminal);
            }
        }
    }
}

pub fn ensure_booted() -> Result<(), String> {
    boot()
}

impl Drop for NativePtySession {
    fn drop(&mut self) {
        self.close();
    }
}

fn boot() -> Result<(), String> {
    static BOOT: OnceLock<Result<(), String>> = OnceLock::new();
    BOOT.get_or_init(|| {
        let root = linuxkit_root_dir()?;
        let root = CString::new(root.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
        let status = unsafe {
            terax_linuxkit_set_log_callback(on_native_log);
            terax_linuxkit_boot(root.as_ptr())
        };
        if status == 0 {
            Ok(())
        } else {
            Err(format!("ios-linuxkit boot failed: {status}"))
        }
    })
    .clone()
}

extern "C" fn on_native_log(data: *const c_char, len: usize) {
    if data.is_null() || len == 0 {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len) };
    let message = String::from_utf8_lossy(bytes);
    let message = message.trim_end_matches(['\r', '\n']);
    if !message.is_empty() {
        log::info!("[ios-terminal/native] {message}");
    }
}

extern "C" fn on_native_output(user: *mut c_void, data: *const u8, len: usize) {
    if user.is_null() || data.is_null() || len == 0 {
        return;
    }
    let context = unsafe { &*(user as *const NativePtyContext) };
    if context.closed.load(Ordering::Acquire) {
        return;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    let _ = context.on_data.send(Response::new(bytes));
}

extern "C" fn on_native_exit(user: *mut c_void, code: i32) {
    if user.is_null() {
        return;
    }
    let context = unsafe { &*(user as *const NativePtyContext) };
    context.closed.store(true, Ordering::Release);
    if !context.exited.swap(true, Ordering::AcqRel) {
        log::warn!("[ios-terminal] native pty exit code={code}");
        let _ = context.on_exit.send(code);
    }
}

extern "C" fn on_command_output(user: *mut c_void, data: *const u8, len: usize) {
    if user.is_null() || data.is_null() || len == 0 {
        return;
    }
    let context = unsafe { &*(user as *const NativeCommandContext) };
    let bytes = unsafe { std::slice::from_raw_parts(data, len) };
    let mut state = context.state.lock().unwrap();
    state.output.extend_from_slice(bytes);
    context.ready.notify_all();
}

extern "C" fn on_command_exit(user: *mut c_void, code: i32) {
    if user.is_null() {
        return;
    }
    let context = unsafe { &*(user as *const NativeCommandContext) };
    let mut state = context.state.lock().unwrap();
    state.exited = true;
    state.exit_code = code;
    context.ready.notify_all();
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn parse_command_output(raw: &str, sentinel: &str) -> Option<(String, i32, String)> {
    let marker = format!("{sentinel}:");
    let idx = raw.rfind(&marker)?;
    let line_start = raw[..idx].rfind('\n').map_or(0, |pos| pos + 1);
    let line_end = raw[idx..]
        .find('\n')
        .map_or(raw.len(), |offset| idx + offset + 1);
    let line = raw[idx..line_end].trim();
    let mut parts = line.splitn(3, ':');
    let _ = parts.next()?;
    let exit_code = parts.next()?.parse::<i32>().ok()?;
    let cwd_after = parts.next().unwrap_or("/root").to_string();

    let mut stdout = String::with_capacity(raw.len().saturating_sub(line.len()));
    stdout.push_str(&raw[..line_start]);
    stdout.push_str(&raw[line_end..]);
    Some((stdout, exit_code, cwd_after))
}

fn shell_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}
