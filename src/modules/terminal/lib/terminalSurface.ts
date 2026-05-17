import { FitAddon, Terminal } from "ghostty-web";

export { FitAddon, Terminal };

export type TerminalSurface = Terminal;

export type TerminalSearchAddon = {
  findNext: (query: string, options?: unknown) => void;
  findPrevious: (query: string, options?: unknown) => void;
  clearDecorations: () => void;
};

type GhosttyCell = {
  codepoint: number;
  width?: number;
};

type SearchMatch = {
  line: number;
  column: number;
  length: number;
};

type SearchDecorations = {
  matchBackground?: string;
  activeMatchBackground?: string;
};

type GhosttySearchTerm = Terminal & {
  getScrollbackLength?: () => number;
  getScrollbackLine?: (offset: number) => GhosttyCell[] | null;
  scrollToLine?: (line: number) => void;
  select?: (column: number, row: number, length: number) => void;
  clearSelection?: () => void;
  getViewportY?: () => number;
  onScroll?: (listener: (viewportY: number) => void) => { dispose: () => void };
  onResize?: (listener: (size: { cols: number; rows: number }) => void) => { dispose: () => void };
  setDecorations?: (decorations: Array<{ line: number; column: number; length: number; background?: string; foreground?: string }>) => void;
  clearDecorations?: () => void;
  renderer?: {
    getCanvas?: () => HTMLCanvasElement;
    charWidth?: number;
    charHeight?: number;
  };
  wasmTerm?: {
    getLine?: (y: number) => GhosttyCell[] | null;
    getScrollbackLength?: () => number;
  };
};

function cellsToText(cells: GhosttyCell[] | null | undefined): string {
  if (!cells) return "";
  let out = "";
  for (const cell of cells) {
    if (!cell || cell.width === 0) continue;
    const cp = cell.codepoint;
    if (!cp) {
      out += " ";
      continue;
    }
    try {
      out += String.fromCodePoint(cp);
    } catch {
      out += " ";
    }
  }
  return out.replace(/\s+$/u, "");
}

function parseDecorations(options: unknown): Required<SearchDecorations> {
  const opts = options as { decorations?: SearchDecorations } | undefined;
  return {
    matchBackground: opts?.decorations?.matchBackground ?? "#515c6a",
    activeMatchBackground: opts?.decorations?.activeMatchBackground ?? "#d18616",
  };
}

function ensureOverlay(term: GhosttySearchTerm): HTMLCanvasElement | null {
  const parent = term.element;
  if (!parent) return null;
  let overlay = parent.querySelector<HTMLCanvasElement>(
    ":scope > canvas[data-terax-search-overlay]",
  );
  if (!overlay) {
    overlay = document.createElement("canvas");
    overlay.dataset.teraxSearchOverlay = "true";
    overlay.setAttribute("aria-hidden", "true");
    overlay.style.position = "absolute";
    overlay.style.inset = "0";
    overlay.style.pointerEvents = "none";
    overlay.style.zIndex = "2";
    const cs = getComputedStyle(parent);
    if (cs.position === "static" || cs.position === "") parent.style.position = "relative";
    parent.appendChild(overlay);
  }
  return overlay;
}

function removeOverlay(term: GhosttySearchTerm): void {
  const overlay = term.element?.querySelector<HTMLCanvasElement>(
    ":scope > canvas[data-terax-search-overlay]",
  );
  overlay?.remove();
}

/**
 * Terax-side search adapter for ghostty-web.
 *
 * It scans ghostty-web's scrollback plus active viewport, then paints a small
 * overlay canvas above ghostty-web's renderer for xterm-search-style highlights.
 * This uses only public-ish ghostty-web APIs (`getScrollbackLine`, `wasmTerm`
 * line access, renderer cell metrics, `scrollToLine`, `select`) so it can be
 * replaced by a native ghostty-web addon later without changing Terax's UI.
 */
export function createSearchAdapter(term: Terminal): TerminalSearchAddon {
  const t = term as GhosttySearchTerm;
  let lastQuery = "";
  let matches: SearchMatch[] = [];
  let active = -1;
  let decorations = parseDecorations(undefined);
  let redrawRaf: number | null = null;

  const scrollbackLength = () => {
    try {
      return t.getScrollbackLength?.() ?? t.wasmTerm?.getScrollbackLength?.() ?? 0;
    } catch {
      return 0;
    }
  };

  const lineText = (absoluteLine: number, historyLines: number): string => {
    try {
      if (absoluteLine < historyLines) {
        return cellsToText(t.getScrollbackLine?.(absoluteLine));
      }
      const y = absoluteLine - historyLines;
      return cellsToText(t.wasmTerm?.getLine?.(y));
    } catch {
      return "";
    }
  };

  const syncOverlaySize = (overlay: HTMLCanvasElement): boolean => {
    const main = t.renderer?.getCanvas?.();
    if (!main) return false;
    const rect = main.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return false;
    if (overlay.width !== main.width) overlay.width = main.width;
    if (overlay.height !== main.height) overlay.height = main.height;
    overlay.style.width = `${rect.width}px`;
    overlay.style.height = `${rect.height}px`;
    return true;
  };

  const drawHighlights = () => {
    redrawRaf = null;
    if (typeof t.setDecorations === "function") {
      if (!lastQuery || matches.length === 0) {
        t.clearDecorations?.();
        removeOverlay(t);
        return;
      }
      t.setDecorations(
        matches.map((match, i) => ({
          line: match.line,
          column: match.column,
          length: match.length,
          background: i === active ? decorations.activeMatchBackground : decorations.matchBackground,
        })),
      );
      removeOverlay(t);
      return;
    }
    if (!lastQuery || matches.length === 0) {
      removeOverlay(t);
      return;
    }
    const overlay = ensureOverlay(t);
    if (!overlay || !syncOverlaySize(overlay)) return;
    const ctx = overlay.getContext("2d", { alpha: true });
    if (!ctx) return;
    ctx.clearRect(0, 0, overlay.width, overlay.height);

    const main = t.renderer?.getCanvas?.();
    const mainRect = main?.getBoundingClientRect();
    const dpr = mainRect && mainRect.width > 0 ? overlay.width / mainRect.width : window.devicePixelRatio || 1;
    const cellW = (t.renderer?.charWidth ?? 0) * dpr;
    const cellH = (t.renderer?.charHeight ?? 0) * dpr;
    if (cellW <= 0 || cellH <= 0) return;

    const history = scrollbackLength();
    const viewportY = typeof t.getViewportY === "function" ? Math.floor(t.getViewportY()) : 0;
    for (let i = 0; i < matches.length; i++) {
      const match = matches[i];
      const row = match.line - history + viewportY;
      if (row < 0 || row >= t.rows) continue;
      ctx.fillStyle = i === active ? decorations.activeMatchBackground : decorations.matchBackground;
      ctx.globalAlpha = i === active ? 0.78 : 0.48;
      ctx.fillRect(
        match.column * cellW,
        row * cellH,
        Math.max(cellW, match.length * cellW),
        cellH,
      );
    }
    ctx.globalAlpha = 1;
  };

  const scheduleRedraw = () => {
    if (redrawRaf !== null) cancelAnimationFrame(redrawRaf);
    redrawRaf = requestAnimationFrame(drawHighlights);
  };

  const rebuild = (query: string) => {
    lastQuery = query;
    matches = [];
    active = -1;
    const needle = query.toLocaleLowerCase();
    if (!needle) {
      scheduleRedraw();
      return;
    }
    const history = scrollbackLength();
    const total = history + Math.max(0, t.rows ?? 0);
    for (let line = 0; line < total; line++) {
      const haystack = lineText(line, history);
      if (!haystack) continue;
      const lower = haystack.toLocaleLowerCase();
      let from = 0;
      while (from <= lower.length) {
        const column = lower.indexOf(needle, from);
        if (column < 0) break;
        matches.push({ line, column, length: query.length });
        from = column + Math.max(1, needle.length);
      }
    }
    scheduleRedraw();
  };

  const activate = (index: number) => {
    if (matches.length === 0) {
      scheduleRedraw();
      return;
    }
    active = ((index % matches.length) + matches.length) % matches.length;
    const match = matches[active];
    try {
      t.scrollToLine?.(match.line);
    } catch {}
    if (typeof t.setDecorations !== "function") {
      try {
        const history = scrollbackLength();
        const viewportY = typeof t.getViewportY === "function" ? Math.floor(t.getViewportY()) : 0;
        const row = match.line - history + viewportY;
        if (row >= 0 && row < t.rows) t.select?.(match.column, row, match.length);
      } catch {}
    }
    scheduleRedraw();
  };

  const ensure = (query: string, options?: unknown) => {
    decorations = parseDecorations(options);
    if (query !== lastQuery) rebuild(query);
    else scheduleRedraw();
  };

  const scrollSub = t.onScroll?.(() => scheduleRedraw());
  const resizeSub = t.onResize?.(() => scheduleRedraw());
  const originalDispose = t.dispose.bind(t);
  if (!(t as unknown as { __teraxSearchDisposePatched?: boolean }).__teraxSearchDisposePatched) {
    (t as unknown as { __teraxSearchDisposePatched?: boolean }).__teraxSearchDisposePatched = true;
    t.dispose = () => {
      scrollSub?.dispose();
      resizeSub?.dispose();
      if (redrawRaf !== null) cancelAnimationFrame(redrawRaf);
      removeOverlay(t);
      originalDispose();
    };
  }

  return {
    findNext: (query: string, options?: unknown) => {
      ensure(query, options);
      activate(active + 1);
    },
    findPrevious: (query: string, options?: unknown) => {
      ensure(query, options);
      activate(active - 1);
    },
    clearDecorations: () => {
      lastQuery = "";
      matches = [];
      active = -1;
      try {
        t.clearSelection?.();
        t.clearDecorations?.();
      } catch {}
      if (redrawRaf !== null) cancelAnimationFrame(redrawRaf);
      redrawRaf = null;
      removeOverlay(t);
    },
  };
}

export function serializeTerminal(_term: Terminal, _scrollback: number): string | null {
  // TODO(ghostty-web): use ghostty-web row/scrollback APIs to serialize the
  // visible buffer for renderer-slot recycling. Returning null is safe; dormant
  // PTY output is still buffered separately by DormantRing.
  return null;
}
