import type { Terminal } from "./terminalSurface";

type Disposable = { dispose: () => void };
type IMarker = { isDisposed?: boolean; dispose: () => void };
type XtermCompat = Terminal & {
  parser?: {
    registerOscHandler?: (ident: number, handler: (data: string) => boolean) => Disposable;
  };
  registerMarker?: (cursorYOffset: number) => IMarker | null;
};

export function registerCwdHandler(
  term: Terminal,
  onCwd: (cwd: string) => void,
): () => void {
  const compat = term as XtermCompat;
  const d = compat.parser?.registerOscHandler?.(7, (data: string) => {
    const cwd = parseOsc7(data);
    if (cwd) onCwd(cwd);
    return true;
  });
  return () => d?.dispose();
}

export type PromptTracker = {
  getMarker: () => IMarker | null;
  dispose: () => void;
};

export function registerPromptTracker(term: Terminal): PromptTracker {
  const compat = term as XtermCompat;
  let marker: IMarker | null = null;
  const d = compat.parser?.registerOscHandler?.(133, (data: string) => {
    if (data.startsWith("A")) {
      marker?.dispose();
      marker = compat.registerMarker?.(0) ?? null;
    }
    return true;
  });
  return {
    getMarker: () => (marker && !marker.isDisposed ? marker : null),
    dispose: () => {
      d?.dispose();
      marker?.dispose();
      marker = null;
    },
  };
}

function parseOsc7(data: string): string | null {
  const m = data.match(/^file:\/\/[^/]*(\/.*)$/);
  if (!m) return null;
  let path = m[1];
  try {
    path = decodeURIComponent(path);
  } catch {}
  // /C:/Users/foo -> C:/Users/foo so it's a valid Windows path.
  if (/^\/[A-Za-z]:/.test(path)) path = path.slice(1);
  return path;
}
