// On-device debug console for diagnosing issues without a Mac / remote
// inspector. When built with VITE_DEBUG_CONSOLE=1, Terax loads the eruda
// on-screen devtools (console / network / elements …) and opens it by default;
// the user can close it via eruda's own UI. Even if the app fails to mount
// (black screen), the panel and the captured startup errors are still visible
// right on the device.
//
// Loaded from index.html BEFORE main.tsx so the early error listeners are
// installed before app code runs. Tree-shaken from normal builds (flag unset).
if (import.meta.env.VITE_DEBUG_CONSOLE) {
  const buffer: string[] = [];

  const fmt = (v: unknown): string => {
    if (v instanceof Error) return v.stack || `${v.name}: ${v.message}`;
    if (typeof v === "string") return v;
    try {
      return JSON.stringify(v);
    } catch {
      return String(v);
    }
  };

  const record = (tag: string, args: unknown[]) =>
    buffer.push(`[${tag}] ${args.map(fmt).join(" ")}`);

  // Capture the earliest failures synchronously, before eruda finishes loading.
  window.addEventListener("error", (e) =>
    record("error", [e.message, e.error, `${e.filename}:${e.lineno}:${e.colno}`]),
  );
  window.addEventListener("unhandledrejection", (e) =>
    record("promise", [(e as PromiseRejectionEvent).reason]),
  );

  // Minimal zero-dependency fallback if eruda fails to load (e.g. under CSP).
  const showFallbackOverlay = () => {
    const box = document.createElement("pre");
    box.style.cssText =
      "position:fixed;inset:0 0 auto 0;max-height:60vh;overflow:auto;margin:0;" +
      "padding:8px 8px 8px;z-index:2147483647;background:rgba(0,0,0,0.9);" +
      "color:#19ff8c;font:12px/1.4 ui-monospace,Menlo,monospace;white-space:pre-wrap";
    const close = document.createElement("button");
    close.textContent = "× close";
    close.style.cssText =
      "position:fixed;top:4px;right:4px;z-index:2147483647;background:#222;color:#fff;" +
      "border:1px solid #555;border-radius:4px;padding:2px 10px;font:12px monospace";
    close.onclick = () => {
      box.remove();
      close.remove();
    };
    const render = () => {
      box.textContent = buffer.join("\n") || "[debug-console] ready — no errors captured";
    };
    const origError = console.error.bind(console);
    console.error = (...args: unknown[]) => {
      record("console.error", args);
      render();
      origError(...args);
    };
    const attach = () => {
      const root = document.body || document.documentElement;
      root.appendChild(box);
      root.appendChild(close);
      render();
    };
    if (document.body) attach();
    else document.addEventListener("DOMContentLoaded", attach);
  };

  void (async () => {
    try {
      // @ts-ignore - eruda ships no types entrypoint
      const eruda = (await import("eruda")).default as {
        init: () => void;
        show: () => void;
      };
      eruda.init();
      eruda.show(); // open by default; closable from eruda's UI
      buffer.forEach((line) => console.error(line)); // replay early startup errors
    } catch {
      showFallbackOverlay();
    }
  })();
}

export {};
