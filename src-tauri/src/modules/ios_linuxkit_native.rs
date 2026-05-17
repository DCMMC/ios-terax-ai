use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use tauri::ipc::{Channel, Response};

use crate::modules::linuxkit::linuxkit_root_dir;

const LINUXKIT_SHELL: &str = "/bin/ash";
const LINUXKIT_SHELL_ARG0: &str = "-ash";
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

extern "C" {
    fn terax_linuxkit_boot(root_path: *const c_char) -> c_int;
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

struct NativePtyContext {
    on_data: Channel<Response>,
    on_exit: Channel<i32>,
    closed: AtomicBool,
    exited: AtomicBool,
}

unsafe impl Send for NativePtySession {}

impl NativePtySession {
    pub fn open(
        cols: u16,
        rows: u16,
        on_data: Channel<Response>,
        on_exit: Channel<i32>,
    ) -> Result<Self, String> {
        boot()?;

        let exe = CString::new(LINUXKIT_SHELL).map_err(|e| e.to_string())?;
        let arg0 = CString::new(LINUXKIT_SHELL_ARG0).map_err(|e| e.to_string())?;
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
        let status = unsafe { terax_linuxkit_boot(root.as_ptr()) };
        if status == 0 {
            Ok(())
        } else {
            Err(format!("ios-linuxkit boot failed: {status}"))
        }
    })
    .clone()
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
        let _ = context.on_exit.send(code);
    }
}
