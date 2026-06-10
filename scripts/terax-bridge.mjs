#!/usr/bin/env bun
// Mac-side client for the on-device debug bridge (src-tauri/src/modules/debug_bridge.rs).
//
// The bridge listens on 127.0.0.1:8765 *on the iPad*. usbmuxd (via `iproxy`)
// tunnels a Mac-local port to it over USB, so this works even though the guest
// network stack is the thing we're debugging.
//
// Usage:
//   bun scripts/terax-bridge.mjs --forward                 # start iproxy (USB tunnel), keep running
//   bun scripts/terax-bridge.mjs "env | grep -i proxy"     # run one command, print output
//   bun scripts/terax-bridge.mjs -t 120 "claude -p hi"     # with a 120s timeout
//
// Requires `iproxy` (brew install libimobiledevice). The connected iPad UDID is
// read from IOS_DEVICE_UDID or auto-detected via `idevice_id -l`.

import { spawn, spawnSync } from "node:child_process";
import { connect } from "node:net";

const LOCAL_PORT = Number(process.env.TERAX_BRIDGE_PORT ?? 8765);
const DEVICE_PORT = 8765;

function detectUdid() {
  if (process.env.IOS_DEVICE_UDID) return process.env.IOS_DEVICE_UDID;
  const out = spawnSync("idevice_id", ["-l"], { encoding: "utf8" }).stdout ?? "";
  const udid = out.split("\n").map((l) => l.trim()).filter(Boolean)[0];
  if (!udid) {
    console.error("No iPad detected (idevice_id -l empty). Set IOS_DEVICE_UDID.");
    process.exit(2);
  }
  return udid;
}

function startForward() {
  const udid = detectUdid();
  console.error(`iproxy ${LOCAL_PORT} ${DEVICE_PORT} -u ${udid} (USB tunnel to device:${DEVICE_PORT})`);
  const child = spawn("iproxy", [String(LOCAL_PORT), String(DEVICE_PORT), "-u", udid], {
    stdio: "inherit",
  });
  child.on("exit", (code) => process.exit(code ?? 0));
}

function runCommand(command, timeoutSecs) {
  const sock = connect(LOCAL_PORT, "127.0.0.1");
  let buf = "";
  let sentTimeout = !timeoutSecs;
  let sentCommand = false;
  const TRAILER = /^###TERAX (.+)###$/m;

  sock.setEncoding("utf8");
  sock.on("connect", () => {
    if (timeoutSecs) sock.write(`:timeout ${timeoutSecs}\n`);
  });
  sock.on("data", (chunk) => {
    buf += chunk;
    // The greeting / ack lines start with '# '; once we see the prompt is up,
    // send the command (after the optional timeout ack).
    if (!sentCommand) {
      if (timeoutSecs && !sentTimeout && /^# timeout=/m.test(buf)) sentTimeout = true;
      if (sentTimeout && /^# /m.test(buf)) {
        sock.write(`${command}\n`);
        sentCommand = true;
        buf = ""; // drop greeting; keep only command output
        return;
      }
    }
    const m = buf.match(TRAILER);
    if (m) {
      const body = buf.slice(0, m.index);
      process.stdout.write(body);
      console.error(`\n[bridge] ${m[1]}`);
      sock.end();
      const code = /code=(-?\d+)/.exec(m[1]);
      process.exit(code ? Number(code[1]) : 0);
    }
  });
  sock.on("error", (err) => {
    console.error(`[bridge] connection error: ${err.message}`);
    console.error(`Is the tunnel up?  bun scripts/terax-bridge.mjs --forward`);
    process.exit(3);
  });
}

const args = process.argv.slice(2);
if (args[0] === "--forward") {
  startForward();
} else {
  let timeoutSecs = 0;
  if (args[0] === "-t") {
    timeoutSecs = Number(args[1]);
    args.splice(0, 2);
  }
  const command = args.join(" ");
  if (!command) {
    console.error('Usage: bun scripts/terax-bridge.mjs [--forward] [-t SECS] "command"');
    process.exit(2);
  }
  runCommand(command, timeoutSecs);
}
