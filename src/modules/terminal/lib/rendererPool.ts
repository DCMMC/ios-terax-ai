import { detectMonoFontFamily } from "@/lib/fonts";
import { IS_IOS_RUNTIME } from "@/lib/platform";
import { usePreferencesStore } from "@/modules/settings/preferences";
import { invoke } from "@tauri-apps/api/core";
import { buildTerminalTheme } from "@/styles/terminalTheme";
import {
  createSearchAdapter,
  serializeTerminal,
  FitAddon,
  Terminal,
  type TerminalSearchAddon,
} from "./terminalSurface";

export const POOL_MAX_SIZE = 5;
const FIT_DEBOUNCE_MS = 8;
const PTY_RESIZE_DEBOUNCE_MS = 256;
const SNAPSHOT_SCROLLBACK_CAP = 5_000;

export type SlotAdapter = {
  resolveLeaf(leafId: number): LeafBridge | null;
  evictLeaf(leafId: number): void;
  isLeafFocused(leafId: number): boolean;
};

export type LeafBridge = {
  writeToPty(data: string): void;
  resizePty(cols: number, rows: number): void;
};

export type Slot = {
  readonly id: number;
  readonly term: Terminal;
  readonly fitAddon: FitAddon;
  readonly searchAddon: TerminalSearchAddon;
  readonly host: HTMLDivElement;
  currentLeafId: number | null;
  oscDisposers: (() => void)[];
  observer: ResizeObserver | null;
  fitTimer: ReturnType<typeof setTimeout> | null;
  ptyTimer: ReturnType<typeof setTimeout> | null;
  unhideRaf: number | null;
  lastCols: number;
  lastRows: number;
  lastW: number;
  lastH: number;
  lastUsedAt: number;
};

const slots: Slot[] = [];
let recyclerEl: HTMLDivElement | null = null;
let adapter: SlotAdapter | null = null;
let nativeInputListenerInstalled = false;
let nativeInputEnabled = false;

export function configureRendererPool(a: SlotAdapter): void {
  adapter = a;
  installIosNativeInputBridge();
}

export function forEachSlot(fn: (slot: Slot) => void): void {
  for (const s of slots) fn(s);
}

export function poolSize(): number {
  return slots.length;
}

function getRecycler(): HTMLDivElement {
  if (recyclerEl && recyclerEl.isConnected) return recyclerEl;
  const el = document.createElement("div");
  el.setAttribute("data-terax-recycler", "");
  el.style.cssText =
    "position:fixed;left:-99999px;top:-99999px;width:1024px;height:768px;overflow:hidden;pointer-events:none;contain:strict;";
  document.body.appendChild(el);
  recyclerEl = el;
  return el;
}

function termOptions() {
  const prefs = usePreferencesStore.getState();
  return {
    fontFamily: detectMonoFontFamily(),
    fontSize: Math.max(4, Math.round(prefs.terminalFontSize * prefs.zoomLevel)),
    theme: buildTerminalTheme(),
    cursorBlink: false,
    cursorStyle: "bar" as const,
    cursorInactiveStyle: "outline" as const,
    scrollback: prefs.terminalScrollback,
    allowProposedApi: true,
  };
}

function createSlot(): Slot {
  const term = new Terminal(termOptions());
  const fitAddon = new FitAddon();
  const searchAddon = createSearchAdapter(term);
  term.loadAddon(fitAddon);

  const host = document.createElement("div");
  host.style.cssText = "width:100%;height:100%;";
  host.setAttribute("data-terax-slot", String(slots.length));
  getRecycler().appendChild(host);
  term.open(host);
  disableIosWebTerminalInput(host, term);

  const slot: Slot = {
    id: slots.length,
    term,
    fitAddon,
    searchAddon,
    host,
    currentLeafId: null,
    oscDisposers: [],
    observer: null,
    fitTimer: null,
    ptyTimer: null,
    unhideRaf: null,
    lastCols: term.cols,
    lastRows: term.rows,
    lastW: 0,
    lastH: 0,
    lastUsedAt: 0,
  };


  term.attachCustomKeyEventHandler((event) => {
    const leafId = slot.currentLeafId;
    if (leafId === null) return false;
    const bridge = adapter?.resolveLeaf(leafId);
    if (!bridge) return true;
    if (isCtrlBackspace(event)) {
      event.preventDefault();
      if (event.type === "keydown") bridge.writeToPty("\x17");
      return false;
    }
    if (isShiftEnter(event)) {
      event.preventDefault();
      if (event.type === "keydown") bridge.writeToPty("\x1b\r");
      return false;
    }
    return true;
  });

  term.onData((data) => {
    const leafId = slot.currentLeafId;
    if (leafId === null) return;
    adapter?.resolveLeaf(leafId)?.writeToPty(data);
  });

  slots.push(slot);
  return slot;
}

function installIosNativeInputBridge(): void {
  if (
    !IS_IOS_RUNTIME ||
    nativeInputListenerInstalled ||
    typeof window === "undefined"
  )
    return;
  nativeInputListenerInstalled = true;
  window.addEventListener("terax:native-terminal-input", (event) => {
    const data = (event as CustomEvent<unknown>).detail;
    if (typeof data !== "string" || data.length === 0) return;
    iosDebugLog(`js native input event bytes=${data.length}`);
    const slot = slots.find(
      (s) =>
        s.currentLeafId !== null &&
        (adapter?.isLeafFocused(s.currentLeafId) ?? false),
    );
    if (slot?.currentLeafId === null || slot?.currentLeafId === undefined) {
      iosDebugLog("js native input dropped: no focused terminal slot");
      return;
    }
    iosDebugLog(`js native input -> pty leaf=${slot.currentLeafId}`);
    adapter?.resolveLeaf(slot.currentLeafId)?.writeToPty(data);
  });
  window.addEventListener("focusin", syncIosNativeInputFromDomFocus, true);
  window.addEventListener("focusout", () => {
    window.setTimeout(syncIosNativeInputFromDomFocus, 0);
  }, true);
}

function disableIosWebTerminalInput(host: HTMLDivElement, term: Terminal): void {
  if (!IS_IOS_RUNTIME) return;

  // Match ios-linuxkit's terminal app: iOS owns text entry natively, while
  // Ghostty remains the renderer and input encoder for non-native events.
  host.style.setProperty("-webkit-touch-callout", "none");
  host.style.setProperty("-webkit-tap-highlight-color", "transparent");
  host.style.setProperty("-webkit-user-select", "none");
  host.style.userSelect = "none";
  host.style.touchAction = "manipulation";
  host.removeAttribute("contenteditable");
  host.removeAttribute("role");
  host.removeAttribute("aria-label");
  host.removeAttribute("aria-multiline");
  host.setAttribute("tabindex", "-1");

  for (const el of host.querySelectorAll<HTMLElement>("*")) {
    el.style.setProperty("-webkit-touch-callout", "none");
    el.style.setProperty("-webkit-user-select", "none");
    el.style.userSelect = "none";
  }

  const textarea = term.textarea;
  if (!textarea) return;
  textarea.readOnly = true;
  textarea.setAttribute("tabindex", "-1");
  textarea.style.pointerEvents = "none";
  textarea.style.setProperty("-webkit-user-select", "none");
  textarea.style.userSelect = "none";
  textarea.blur();

  const activate = (event: Event) => {
    const hasSelection =
      typeof term.hasSelection === "function" ? term.hasSelection() : false;
    if (!hasSelection) {
      event.preventDefault();
      event.stopImmediatePropagation();
    }
    setIosNativeTerminalInputEnabled(true);
    focusIosNativeTerminalInput();
  };
  host.addEventListener("touchstart", activate, { capture: true });
  host.addEventListener("touchend", activate, { capture: true });
  host.addEventListener("mousedown", activate, { capture: true });
}

export function setIosNativeTerminalInputEnabled(enabled: boolean): void {
  if (!IS_IOS_RUNTIME || nativeInputEnabled === enabled) return;
  nativeInputEnabled = enabled;
  iosDebugLog(`native input enabled=${enabled}`);
  void invoke("ios_set_terminal_input_enabled", { enabled }).catch((e) => {
    console.error("[terax] failed to update iOS terminal input focus:", e);
  });
}

function focusIosNativeTerminalInput(): void {
  if (!IS_IOS_RUNTIME) return;
  iosDebugLog("native input focus requested");
  void invoke("ios_focus_terminal_input").catch((e) => {
    console.error("[terax] failed to focus iOS terminal input:", e);
  });
}

function iosDebugLog(message: string): void {
  if (!IS_IOS_RUNTIME) return;
  void invoke("ios_debug_log", { message }).catch(() => {});
}

export function iosTerminalDebugLog(message: string): void {
  iosDebugLog(message);
}

function syncIosNativeInputFromDomFocus(): void {
  if (!IS_IOS_RUNTIME) return;
  const active = document.activeElement;
  if (isEditableElement(active)) {
    setIosNativeTerminalInputEnabled(false);
    return;
  }
  const hasFocusedTerminal = slots.some(
    (s) =>
      s.currentLeafId !== null &&
      (adapter?.isLeafFocused(s.currentLeafId) ?? false),
  );
  setIosNativeTerminalInputEnabled(hasFocusedTerminal);
}

function isEditableElement(el: Element | null): boolean {
  if (!(el instanceof HTMLElement)) return false;
  if (el.closest("[data-terax-slot]")) return false;
  const tag = el.tagName.toLowerCase();
  if (tag === "textarea" || tag === "select") return true;
  if (tag !== "input") return el.isContentEditable;
  const input = el as HTMLInputElement;
  return !["button", "checkbox", "color", "file", "radio", "range", "reset", "submit"]
    .includes(input.type);
}

type PickResult = { slot: Slot; previousLeafId: number | null };

function isAltScreen(s: Slot): boolean {
  try {
    return s.term.buffer.active.type === "alternate";
  } catch {
    return false;
  }
}

function pickSlotFor(leafId: number): PickResult {
  const free = slots.find((s) => s.currentLeafId === null);
  if (free) return { slot: free, previousLeafId: null };
  if (slots.length < POOL_MAX_SIZE)
    return { slot: createSlot(), previousLeafId: null };

  let best: Slot | null = null;
  let bestScore = Number.POSITIVE_INFINITY;
  for (const s of slots) {
    if (s.currentLeafId === leafId) return { slot: s, previousLeafId: null };
    const focused =
      s.currentLeafId !== null &&
      (adapter?.isLeafFocused(s.currentLeafId) ?? false);
    const score =
      (isAltScreen(s) ? 100 : 0) + (focused ? 10 : 0) + s.lastUsedAt / 1e12;
    if (score < bestScore) {
      bestScore = score;
      best = s;
    }
  }
  const chosen = best!;
  return { slot: chosen, previousLeafId: chosen.currentLeafId };
}

export type AcquireParams = {
  leafId: number;
  container: HTMLDivElement;
  snapshot: string | null;
  drainRing: (write: (bytes: Uint8Array) => void) => void;
  shellExited: boolean;
  searchQuery: string | null;
  cols: number;
  rows: number;
  onScopeChange: (cols: number, rows: number) => void;
  registerOsc: (term: Terminal) => (() => void)[];
  onSearchReady: (addon: TerminalSearchAddon) => void;
};

export function acquireSlot(params: AcquireParams): Slot {
  const existing = slots.find((s) => s.currentLeafId === params.leafId);
  if (existing) {
    rewireSlot(existing, params);
    return existing;
  }

  const pick = pickSlotFor(params.leafId);
  if (pick.previousLeafId !== null) {
    adapter?.evictLeaf(pick.previousLeafId);
  }
  if (
    pick.slot.currentLeafId !== null &&
    pick.slot.currentLeafId !== params.leafId
  ) {
    detachSlotFromLeaf(pick.slot);
  }
  bindSlot(pick.slot, params);
  return pick.slot;
}

function bindSlot(slot: Slot, p: AcquireParams): void {
  slot.currentLeafId = p.leafId;
  slot.lastUsedAt = performance.now();

  cancelPendingUnhide(slot);
  slot.host.style.visibility = "hidden";

  if (slot.host.parentNode !== p.container) {
    p.container.appendChild(slot.host);
  }

  slot.term.options.disableStdin = p.shellExited;
  iosDebugLog(
    `renderer bind leaf=${p.leafId} slot=${slot.id} shellExited=${p.shellExited} snapshot=${p.snapshot ? p.snapshot.length : 0}`,
  );
  slot.term.clear();
  slot.term.reset();

  if (
    p.cols > 0 &&
    p.rows > 0 &&
    (slot.term.cols !== p.cols || slot.term.rows !== p.rows)
  ) {
    slot.term.resize(p.cols, p.rows);
  }

  if (p.snapshot) {
    try {
      slot.term.write(p.snapshot);
    } catch (e) {
      console.warn("[terax] snapshot replay failed:", e);
    }
  }
  p.drainRing((bytes) => slot.term.write(bytes));
  try {
    slot.term.write("\x1b[?25h");
  } catch {}

  for (const d of slot.oscDisposers) {
    try {
      d();
    } catch {}
  }
  slot.oscDisposers = p.registerOsc(slot.term);

  setupResizeObserver(slot, p);
  slot.fitAddon.fit();
  slot.lastCols = slot.term.cols;
  slot.lastRows = slot.term.rows;
  slot.lastW = p.container.clientWidth;
  slot.lastH = p.container.clientHeight;
  if (slot.lastCols !== p.cols || slot.lastRows !== p.rows) {
    p.onScopeChange(slot.lastCols, slot.lastRows);
  }

  if (p.searchQuery) {
    try {
      slot.searchAddon.findNext(p.searchQuery);
    } catch {}
  }

  applyCursorBlinkOnSlot(slot, adapter?.isLeafFocused(p.leafId) ?? false);

  scheduleUnhide(slot);

  p.onSearchReady(slot.searchAddon);
}

function scheduleUnhide(slot: Slot): void {
  slot.unhideRaf = requestAnimationFrame(() => {
    slot.unhideRaf = requestAnimationFrame(() => {
      slot.unhideRaf = null;
      slot.host.style.visibility = "";
      const leafId = slot.currentLeafId;
      if (leafId !== null && adapter?.isLeafFocused(leafId)) {
        if (IS_IOS_RUNTIME) {
          setIosNativeTerminalInputEnabled(true);
          focusIosNativeTerminalInput();
        } else {
          slot.term.focus();
        }
      }
    });
  });
}

function cancelPendingUnhide(slot: Slot): void {
  if (slot.unhideRaf !== null) {
    cancelAnimationFrame(slot.unhideRaf);
    slot.unhideRaf = null;
  }
}

function rewireSlot(slot: Slot, p: AcquireParams): void {
  slot.lastUsedAt = performance.now();
  if (slot.host.parentNode !== p.container) {
    p.container.appendChild(slot.host);
  }
  setupResizeObserver(slot, p);
  slot.fitAddon.fit();
  slot.lastW = p.container.clientWidth;
  slot.lastH = p.container.clientHeight;
  if (slot.term.cols !== p.cols || slot.term.rows !== p.rows) {
    p.onScopeChange(slot.term.cols, slot.term.rows);
  }
  p.onSearchReady(slot.searchAddon);
}

function setupResizeObserver(slot: Slot, p: AcquireParams): void {
  slot.observer?.disconnect();
  if (slot.fitTimer) clearTimeout(slot.fitTimer);
  if (slot.ptyTimer) clearTimeout(slot.ptyTimer);
  slot.fitTimer = null;
  slot.ptyTimer = null;

  const container = p.container;
  const flushPty = () => {
    slot.ptyTimer = null;
    if (slot.currentLeafId !== p.leafId) return;
    if (slot.term.cols === slot.lastCols && slot.term.rows === slot.lastRows)
      return;
    slot.lastCols = slot.term.cols;
    slot.lastRows = slot.term.rows;
    const bridge = adapter?.resolveLeaf(p.leafId);
    bridge?.resizePty(slot.term.cols, slot.term.rows);
    p.onScopeChange(slot.lastCols, slot.lastRows);
  };

  slot.observer = new ResizeObserver(() => {
    if (slot.fitTimer) clearTimeout(slot.fitTimer);
    slot.fitTimer = setTimeout(() => {
      slot.fitTimer = null;
      if (slot.currentLeafId !== p.leafId) return;
      const w = container.clientWidth;
      const h = container.clientHeight;
      if (w === slot.lastW && h === slot.lastH) return;
      slot.lastW = w;
      slot.lastH = h;
      slot.fitAddon.fit();
      if (slot.ptyTimer) clearTimeout(slot.ptyTimer);
      slot.ptyTimer = setTimeout(flushPty, PTY_RESIZE_DEBOUNCE_MS);
    }, FIT_DEBOUNCE_MS);
  });
  slot.observer.observe(container);
}

export type SerializeOutput = {
  snapshot: string | null;
  cols: number;
  rows: number;
};

export function releaseSlot(leafId: number): SerializeOutput | null {
  const slot = slots.find((s) => s.currentLeafId === leafId);
  if (!slot) return null;
  const out = serializeSlot(slot);
  detachSlotFromLeaf(slot);
  return out;
}

function serializeSlot(slot: Slot): SerializeOutput {
  let snapshot: string | null = null;
  try {
    const cap = Math.min(
      SNAPSHOT_SCROLLBACK_CAP,
      usePreferencesStore.getState().terminalScrollback,
    );
    snapshot = serializeTerminal(slot.term, cap);
  } catch (e) {
    console.warn("[terax] serialize failed:", e);
  }
  return { snapshot, cols: slot.term.cols, rows: slot.term.rows };
}

function detachSlotFromLeaf(slot: Slot): void {
  for (const d of slot.oscDisposers) {
    try {
      d();
    } catch {}
  }
  slot.oscDisposers = [];

  slot.observer?.disconnect();
  slot.observer = null;
  if (slot.fitTimer) clearTimeout(slot.fitTimer);
  if (slot.ptyTimer) clearTimeout(slot.ptyTimer);
  slot.fitTimer = null;
  slot.ptyTimer = null;

  cancelPendingUnhide(slot);
  slot.host.style.visibility = "";

  if (slot.host.parentNode !== getRecycler()) {
    getRecycler().appendChild(slot.host);
  }

  slot.currentLeafId = null;
  slot.lastUsedAt = performance.now();
}

export function applyWebglPreference(_enabled: boolean): void {
  // ghostty-web uses its own canvas renderer; xterm WebGL toggles are ignored.
}

export function applyFontSize(size: number): void {
  for (const slot of slots) {
    if (slot.term.options.fontSize === size) continue;
    slot.term.options.fontSize = size;
    slot.fitAddon.fit();
    if (slot.currentLeafId !== null) {
      slot.lastCols = slot.term.cols;
      slot.lastRows = slot.term.rows;
      const bridge = adapter?.resolveLeaf(slot.currentLeafId);
      bridge?.resizePty(slot.term.cols, slot.term.rows);
    }
  }
}

export function applyScrollback(value: number): void {
  for (const slot of slots) {
    if (slot.term.options.scrollback === value) continue;
    slot.term.options.scrollback = value;
  }
}

export function applyTheme(): void {
  const theme = buildTerminalTheme();
  for (const slot of slots) {
    slot.term.options.theme = theme;
  }
}

export function focusSlot(leafId: number): void {
  const slot = slots.find((s) => s.currentLeafId === leafId);
  if (slot && IS_IOS_RUNTIME) {
    setIosNativeTerminalInputEnabled(true);
    focusIosNativeTerminalInput();
    return;
  }
  slot?.term.focus();
}

export function setSlotFocused(leafId: number, focused: boolean): void {
  const slot = slots.find((s) => s.currentLeafId === leafId);
  if (!slot) return;
  applyCursorBlinkOnSlot(slot, focused);
}

function applyCursorBlinkOnSlot(slot: Slot, focused: boolean): void {
  const desired = focused;
  if (slot.term.options.cursorBlink === desired) return;
  slot.term.options.cursorBlink = desired;
}

export function getSlotForLeaf(leafId: number): Slot | null {
  return slots.find((s) => s.currentLeafId === leafId) ?? null;
}

function isCtrlBackspace(e: KeyboardEvent): boolean {
  const ua = typeof navigator !== "undefined" ? navigator.userAgent : "";
  const isMac = /Mac|iPhone|iPad/.test(ua);
  const mod = isMac ? e.metaKey : e.ctrlKey;
  return mod && (e.key === "Backspace" || e.code === "Backspace");
}

function isShiftEnter(e: KeyboardEvent): boolean {
  return (
    e.key === "Enter" && e.shiftKey && !e.altKey && !e.ctrlKey && !e.metaKey
  );
}
