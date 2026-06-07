// On-screen debug console for on-device diagnosis without a Mac / remote
// inspector. When built with VITE_DEBUG_CONSOLE=1, this overlays uncaught
// errors, promise rejections, and console.error/warn onto the page — so even a
// "black screen" (the app failing to mount) shows the underlying error right on
// the device. Remove once no longer needed.
//
// Loaded from index.html BEFORE main.tsx so its listeners are installed before
// the app code runs. Tree-shaken out of normal builds (flag undefined).
if (import.meta.env.VITE_DEBUG_CONSOLE) {
  const box = document.createElement("pre");
  box.style.cssText = [
    "position:fixed",
    "left:0",
    "top:0",
    "right:0",
    "max-height:60vh",
    "overflow:auto",
    "margin:0",
    "padding:8px",
    "z-index:2147483647",
    "background:rgba(0,0,0,0.88)",
    "color:#19ff8c",
    "font:12px/1.4 ui-monospace,Menlo,monospace",
    "white-space:pre-wrap",
    "pointer-events:auto",
  ].join(";");

  const fmt = (v: unknown): string => {
    if (v instanceof Error) return v.stack || `${v.name}: ${v.message}`;
    if (typeof v === "string") return v;
    try {
      return JSON.stringify(v);
    } catch {
      return String(v);
    }
  };

  const attach = () => {
    if (!box.isConnected) (document.body || document.documentElement).appendChild(box);
  };

  const log = (tag: string, ...args: unknown[]) => {
    box.textContent += `[${tag}] ${args.map(fmt).join(" ")}\n`;
    attach();
  };

  window.addEventListener("error", (e) =>
    log("error", e.message, e.error, `${e.filename}:${e.lineno}:${e.colno}`),
  );
  window.addEventListener("unhandledrejection", (e) =>
    log("promise", (e as PromiseRejectionEvent).reason),
  );

  const origError = console.error.bind(console);
  console.error = (...args: unknown[]) => {
    log("console.error", ...args);
    origError(...args);
  };
  const origWarn = console.warn.bind(console);
  console.warn = (...args: unknown[]) => {
    log("warn", ...args);
    origWarn(...args);
  };

  document.addEventListener("DOMContentLoaded", attach);
  log("debug-console", "ready — waiting for errors…");
}
