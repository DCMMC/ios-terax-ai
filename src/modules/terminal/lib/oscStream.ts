const MAX_TAIL = 8192;

function appendBytes(text: string, bytes: Uint8Array): string {
  let out = text;
  const chunk = 2048;
  for (let i = 0; i < bytes.length; i += chunk) {
    out += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return out;
}

function parseOsc7(payload: string): string | null {
  const m = payload.match(/^file:\/\/[^/]*(\/.*)$/);
  if (!m) return null;
  let path = m[1];
  try {
    path = decodeURIComponent(path);
  } catch {}
  if (/^\/[A-Za-z]:/.test(path)) path = path.slice(1);
  return path;
}

export class TerminalOscStream {
  private tail = "";

  feed(bytes: Uint8Array): string[] {
    this.tail = appendBytes(this.tail, bytes);

    const cwdEvents: string[] = [];
    const osc7 = /\x1b\]7;([^\x07]*?)(?:\x07|\x1b\\)/g;
    let consumed = 0;
    let match: RegExpExecArray | null = null;
    while ((match = osc7.exec(this.tail)) !== null) {
      consumed = osc7.lastIndex;
      const cwd = parseOsc7(match[1]);
      if (cwd) cwdEvents.push(cwd);
    }

    if (consumed > 0) {
      this.tail = this.tail.slice(consumed);
    }
    if (this.tail.length > MAX_TAIL) {
      const esc = this.tail.lastIndexOf("\x1b]");
      this.tail = esc >= 0 ? this.tail.slice(esc) : this.tail.slice(-MAX_TAIL);
    }

    return cwdEvents;
  }
}
