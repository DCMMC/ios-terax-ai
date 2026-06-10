//! USB-reachable remote debug bridge for the iOS LinuxKit guest.
//!
//! DEBUG TOOLING — not for public/App Store builds. It opens a loopback TCP
//! command server on the device so a Mac (connected over USB) can drive the
//! embedded Linux guest *without depending on the guest's network stack*. This
//! is what lets us diagnose the "Connection Refused" issue: the transport rides
//! usbmuxd (via `iproxy`), which is independent of the very network we debug.
//!
//! Wiring on the Mac:
//!   iproxy 8765 8765 -u <device-udid>          # forward Mac:8765 -> device:8765
//!   printf 'env | grep -i proxy\n' | nc 127.0.0.1 8765
//!
//! Protocol (line oriented, easy to drive from `nc`/scripts):
//!   * Each input line is a shell command, run via `run_command` against a
//!     tracked cwd (so `cd` persists across commands, like a session).
//!   * `:timeout <secs>` sets the timeout applied to subsequent commands.
//!   * `:quit` closes the connection.
//!   * After each command the server emits the command's stdout followed by a
//!     single trailer line:
//!         ###TERAX code=<exit> cwd=<cwd_after> timed_out=<bool>###
//!     so the reader knows exactly where output ends.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::thread;

use crate::modules::ios_linuxkit_native::{ensure_booted, NativePtySession};

/// Loopback port served on the device. Reach it from the Mac with
/// `iproxy <local> 8765`. Loopback-only: usbmuxd tunnels to it over USB, so it
/// never touches Wi-Fi / the iOS local-network prompt.
const BRIDGE_PORT: u16 = 8765;
const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Serialize guest command execution across connections. The engine tolerates
/// concurrent sessions (shell tools run alongside the UI terminal), but the
/// bridge has no reason to interleave and a lock keeps output coherent.
static EXEC_LOCK: Mutex<()> = Mutex::new(());

/// Spawn the bridge listener on a background thread. Safe to call once at
/// startup; failures are logged and swallowed so they never block app launch.
pub fn start() {
    let _ = thread::Builder::new()
        .name("terax-debug-bridge".into())
        .spawn(serve);
}

fn serve() {
    if let Err(err) = ensure_booted() {
        log::warn!("[debug-bridge] engine boot failed, bridge disabled: {err}");
        return;
    }

    let listener = match TcpListener::bind(("127.0.0.1", BRIDGE_PORT)) {
        Ok(listener) => listener,
        Err(err) => {
            log::warn!("[debug-bridge] bind 127.0.0.1:{BRIDGE_PORT} failed: {err}");
            return;
        }
    };
    log::info!(
        "[debug-bridge] listening on 127.0.0.1:{BRIDGE_PORT} \
         (Mac: `iproxy {BRIDGE_PORT} {BRIDGE_PORT}` then `nc 127.0.0.1 {BRIDGE_PORT}`)"
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let _ = thread::Builder::new()
                    .name("terax-debug-conn".into())
                    .spawn(move || handle_conn(stream));
            }
            Err(err) => log::warn!("[debug-bridge] accept error: {err}"),
        }
    }
}

fn handle_conn(stream: TcpStream) {
    let peer = stream
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_else(|_| "?".into());
    log::info!("[debug-bridge] client connected: {peer}");

    let mut writer = match stream.try_clone() {
        Ok(writer) => writer,
        Err(err) => {
            log::warn!("[debug-bridge] clone stream failed: {err}");
            return;
        }
    };
    let reader = BufReader::new(stream);

    let mut cwd = String::from("/root");
    let mut timeout = DEFAULT_TIMEOUT_SECS;
    let _ = writeln!(
        writer,
        "# terax-debug-bridge ready (cwd={cwd}, timeout={timeout}s). \
         Lines are shell commands; ':timeout N' / ':quit' are control verbs."
    );
    let _ = writer.flush();

    for line in reader.lines() {
        let line = match line {
            Ok(line) => line,
            Err(_) => break,
        };
        let command = line.trim_end_matches(['\r', '\n']);
        if command.is_empty() {
            continue;
        }
        if command == ":quit" {
            break;
        }
        if let Some(rest) = command.strip_prefix(":timeout ") {
            match rest.trim().parse::<u64>() {
                Ok(value) => {
                    timeout = value.max(1);
                    let _ = writeln!(writer, "# timeout={timeout}s");
                }
                Err(_) => {
                    let _ = writeln!(writer, "# invalid timeout: {rest}");
                }
            }
            let _ = writer.flush();
            continue;
        }

        let result = {
            let _guard = EXEC_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            NativePtySession::run_command(command, &cwd, Some(timeout))
        };

        match result {
            Ok(output) => {
                let _ = writer.write_all(output.stdout.as_bytes());
                if !output.cwd_after.is_empty() {
                    cwd = output.cwd_after.clone();
                }
                let _ = writeln!(
                    writer,
                    "\n###TERAX code={} cwd={} timed_out={}###",
                    output.exit_code, cwd, output.timed_out
                );
            }
            Err(err) => {
                let _ = writeln!(writer, "\n###TERAX error={err}###");
            }
        }
        let _ = writer.flush();
    }

    log::info!("[debug-bridge] client disconnected: {peer}");
}
