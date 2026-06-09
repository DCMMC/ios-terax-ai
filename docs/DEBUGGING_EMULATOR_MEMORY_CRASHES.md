# Debugging Emulator Memory-Corruption Crashes — An Agent Playbook

This is a battle-tested playbook for debugging **non-deterministic memory-corruption
crashes** of guest programs (Bun/JSC, Node/V8, Go, …) running under the **ios-linuxkit /
asbestos ARM64-on-ARM64 JIT emulator**. It was written after a multi-session hunt for the
"claude-code crashes ~21s into its TUI with `panic: Segmentation fault at address
0xF7FFDFF8`" bug. The root cause turned out to be a **kernel `madvise` semantics error**
(see the worked example at the end), not a JIT/codegen bug — but it took a dozen wrong
hypotheses to get there.

**Read this first if you are an agent picking up a similar crash.** It will save you days.
The single most important lesson:

> **Make it deterministic, then put a hardware watchpoint on the corrupted memory's HOST
> address. The watchpoint tells you the exact instruction — JIT or engine-C — that
> corrupts it. Everything else is guessing.**

---

## 0. Symptom recognition

You are likely looking at this class of bug if:
- A guest program (especially Bun/JSC) SIGSEGVs at a **weird, constant-ish address**
  (e.g. `0xF7FFDFF8` is the asbestos unmapped gap `0xf0000000..0xffff0000`).
- The crash is **non-deterministic** (e.g. ~80% of runs) and **layout-sensitive**.
- The faulting PC is in a tight loop (e.g. JSC `op_enter` zeroing callee locals), and the
  fault address is **far from SP** — a sign of a **runaway loop with a corrupt bound/count**.
- Simpler/shorter guest workloads work fine; only the long, heavy one trips it.

The crash is the **symptom**. Some earlier-corrupted value (a count, a pointer, a length)
is the **cause**. Your job is to find **who wrote the bad value, and why**.

---

## 1. Triage decision tree (do these IN ORDER — don't skip to fun theories)

1. **Reproduce in isolation, cheaply.** Drive the guest under a host PTY so TUIs run:
   `timeout 60 script -q /tmp/out.log <ish> -f <fakefs> <guest.exe>`. Confirm the crash and
   capture the faulting `pc`/`addr`. (`script` gives the guest an `isatty` stdin so TUI code
   paths run; `</dev/null` routes around them.)
2. **Find the corrupt value at the crash.** Add an env-gated C dump in the kernel fault
   handler (`kernel/calls.c` INT_GPF path) that walks the faulting frame and prints the
   suspect object fields. Confirm *which* field is wrong (e.g. `numCalleeLocals == 0`).
3. **Is it concurrency?** Add an env-gated global mutex around `cpu_run_to_interrupt`
   (`kernel/task.c`) so only one guest thread runs JIT at a time. Re-test.
   - Crash persists ⇒ **single-thread bug.** (This is the common case. Do NOT keep chasing
     TLB/GC races.) Sys-time dropping confirms the lock took effect.
4. **Is the memory genuinely wrong, or is the JIT reading it wrong?** At the point the bad
   value is read, also read the same guest address freshly via `mmu_translate(...)` (the
   ground-truth C path) and compare to what the JIT got.
   - They **agree** (both 0) ⇒ the **memory is genuinely corrupt** — it's a *write* bug, not
     a load/TLB bug. Skip all stale-TLB / load-miscompile theories.
   - They **disagree** ⇒ a JIT load or TLB coherence bug.
5. **Make it deterministic** (Section 2) — required before the watchpoint is useful.
6. **Hardware-watchpoint the corrupted memory** (Section 3) — this finds the writer.
7. **Identify the writer's semantics** (Section 4) and fix.

---

## 2. The deterministic-replay harness (prerequisite for catching the writer)

Non-determinism comes from **guest-visible entropy** that changes heap layout → which object
corrupts, and where. Pin it:

- **`getrandom`/`/dev/urandom`** (JSC/V8 hash seeds): in `kernel/random.c get_random()`,
  return a fixed-seed xorshift stream when an env flag is set.
- **Clocks** (`Date.now`, `performance.now` drive seeds + event-loop ordering): in
  `kernel/time.c sys_clock_gettime` and `sys_gettimeofday`, return a **monotonic counter**
  that advances a fixed step per call (keeps timers progressing while being reproducible).
- Guest ASLR is already off (`ADDR_NO_RANDOMIZE`); host ASLR does **not** affect guest
  vaddrs (they're emulated), so you usually don't need to touch it.

With both pinned, the corrupting object lands at the **same guest address every run**
(verify: the crash's CodeBlock/object address is now constant across runs). This is what
makes a one-shot watchpoint possible. (It also tends to push the crash rate to ~100%, which
is good for debugging.)

All of this should be **env-gated and off by default** so it never ships.

---

## 3. The hardware watchpoint (the decisive tool)

Guest stores happen in JIT-compiled gadget code OR in engine C (syscall handlers, COW,
memcpy). A **guest-address** watchpoint (forcing TLB misses) only sees JIT stores to that
exact guest address — it **misses** engine-C writes and **aliased** writes from other guest
pages. An **lldb hardware watchpoint on the HOST backing address** sees **all** of them.

Setup (engine built `-O0 -g`; lldb on macOS):

1. **Hand lldb the host address early.** Add a tiny `__attribute__((noinline,used))`
   C hook `watch_arm_hook(void *host_addr)`. In `emu/tlb.c`, on the first access to the
   watched guest page (force re-miss by keeping `page`/`page_if_writable` empty for that
   page), `mmu_translate` the target guest address to its host pointer and call
   `watch_arm_hook(hostptr)`. Arm **before** the corruption happens (e.g. at first page
   touch), not at the read site (too late).
2. **lldb driver** (`/tmp/wp_cb.py` + a command file, run under `script` for the TTY):
   - `breakpoint command add -F wp_cb.arm watch_arm_hook` → in the callback,
     `target.WatchAddress(host_addr, 8, read=False, write=True, err)`, then
     `watchpoint command add -F wp_cb.wphit <id>` (NOTE: `SBWatchpoint` has **no**
     `SetScriptCallbackFunction` — use the command-interpreter form), disable the bp,
     return `False` (auto-continue).
   - `wphit`: log `frame.GetPC()` (host pc), disassemble it (`target.ReadInstructions`),
     resolve the symbol, read the new value, return `False`.
   - `process handle -p true -s false -n false SIGSEGV SIGBUS SIGABRT SIGILL SIGTRAP SIGUSR1
     SIGUSR2 SIGPIPE SIGCHLD` (the engine handles these; lldb must pass them, else it stops
     and batch-mode quits).
3. Run deterministically (Section 2). The watchpoint stops at the **exact instruction** that
   writes the corrupt value, with full register state.

**Reading the result:**
- Host pc resolves to a **JIT gadget** (in `.text`, e.g. `gadget_store64_imm_fast`) and
  `guest_pc`/`cpu->pc` (from `[x1+CPU_pc]`, x1=`_cpu`) is valid ⇒ a guest store. Map the
  guest pc back to the guest binary (`llvm-objdump`) to see the JSC/V8 source line.
- Host pc resolves to **engine C** (`_platform_memset`, `c_store*`, a syscall handler) and
  `guest_pc == 0` ⇒ **the engine itself is corrupting guest memory.** This is the high-value
  finding — it points at a kernel/mm bug, not the JIT.

---

## 4. Common root-cause classes (and how to tell them apart)

| Class | Tell-tale | Where to look |
|---|---|---|
| **`madvise` semantics** (the bug we hit) | host `_platform_memset` zeroes guest memory; `guest_pc=0`; victim is a file-backed page that was never written by the guest | `kernel/mmap.c sys_madvise` |
| **mmap/COW page handling** (4K guest vs 16K host pages) | aliased writes; a page's host backing is wrong/shared | `kernel/memory.c` COW path, `fs/real.c realfs_mmap`, `pt_map`/`pt_unmap` |
| **TLB read/write coherence** | `mmu_translate` **disagrees** with the JIT; flushing TLB changes the result | `emu/tlb.c`, `asbestos/.../entry.S` block-entry `mem_changes` check |
| **JIT codegen / fused gadget** | a register holding a pointer/value is clobbered across a `read_prep`/`write_prep` miss (C call) | `asbestos/guest-arm64/gadgets-aarch64/*.S`, `gen.c` |
| **GC / lifetime** | object is freed and its slot reused while still referenced | JSC GC; test by disabling collection (huge heap thresholds) |

**Key discriminators learned the hard way:**
- If **`mmu_translate` agrees** the memory is 0, it's a **write** bug — *not* stale-TLB, *not*
  a load miscompile. (We wasted a lot of time here.)
- If **force-TLB-flush "fixes" it but the memory genuinely reads 0**, the flush is only
  **perturbing timing**, not fixing coherence. Don't conclude "TLB bug" from a flush helping.
- `madvise(MADV_DONTNEED)` must **only zero anonymous pages**; on a **file-backed
  `MAP_PRIVATE`** page it must revert to file content (drop the COW copy), never zero.
  `madvise(MADV_FREE)` is **lazy** — pages keep their contents until reclaimed; never zero
  them. A guest allocator (bmalloc/jemalloc) madvises ranges that can overlap the binary's
  own mapped segments.

---

## 5. Anti-patterns / time-sinks (avoid these)

- **Chasing per-run nondeterminism before pinning entropy.** Every probe lands on a
  different instance; nothing reproduces. *Build the deterministic harness first.*
- **Trusting `ninja` after header/source edits.** It frequently says `no work to do` and runs
  a stale binary, silently invalidating your test. **Always verify your change is in the
  binary** (`nm`/`strings`/`llvm-strings b/ish | grep <your-marker>`) before trusting a result.
  Force it with `rm -f b/.../<file>.c.o && ninja` if needed.
- **Verifying injected env via `process.env`.** Bun/JSC does not expose `JSC_*` vars in
  `process.env`; absence there does NOT mean the var wasn't injected. Verify via behavior
  (`JSC_logGC=1`) or an engine-side debug print.
- **Concluding from a fix that "works" in the instrumented build.** Heavy instrumentation
  shifts heap layout and can mask a nondeterministic crash. Re-verify with a **clean,
  fix-only** build across many runs (≥8–10).
- **Assuming it's a JIT/codegen bug.** Most of this hunt assumed JIT miscompilation; the bug
  was in `kernel/mmap.c`. When `guest_pc=0` at the corrupting write, it's the engine.

---

## 6. Worked example — the claude-code / Bun `0xF7FFDFF8` crash (2026-06)

- **Symptom:** claude-code (Bun 1.3.14 / JSC) SIGSEGV ~21s into the TUI at `0xF7FFDFF8`,
  ~80% of runs.
- **Crash mechanism:** JSC `op_enter` zeroes callee locals: `new_sp = cfr - numCalleeLocals*8`,
  then a `cmp sp,new_sp; b.eq; sub; str xzr,[..]; b` loop. A corrupt `UnlinkedCodeBlock` with
  `numCalleeLocals == 0` makes `new_sp = cfr` sit **above** `old_sp`, so the loop runs away
  zeroing ~134MB of stack down into the unmapped gap → fault.
- **Hunt:** ruled out (in order) concurrency (global guest lock — still crashes ⇒
  single-thread), stale-TLB (`mmu_translate` agreed the value was genuinely 0), instruction
  fusion, "born-zero", a JIT load miscompile, the file mmap loading zero (`pread`-copy didn't
  help), and the periodic timer. Force-TLB-flush "fixed" it but only by perturbing timing and
  was unusably slow.
- **Decisive step:** built the deterministic harness (Section 2) → addresses became constant
  → lldb HW watchpoint (Section 3) on the corrupted field's host backing caught
  **`_platform_memset` with `guest_pc=0`** zeroing it *after it had loaded correctly as 0x78*.
  Engine C, not the JIT.
- **Root cause:** `sys_madvise` (`kernel/mmap.c`) zeroed **every** page for
  `MADV_DONTNEED`/`MADV_FREE`. bun's bmalloc scavenger madvises ranges overlapping the
  binary's 145MB `MAP_PRIVATE` RW segment (which also backs JSC heap/metadata), so the engine
  zeroed loaded JSC code/data → `numCalleeLocals=0` → runaway.
- **Fix:** `MADV_DONTNEED` zeros **only `P_ANONYMOUS`** pages (file pages left intact);
  `MADV_FREE` is a no-op. ~80% crash → 0/8 (op_enter crash gone). See ios-linuxkit
  `kernel/mmap.c` / `kernel/memory.c` (`mem_page_state` helper) and PR
  `DCMMC/ios-linuxkit#6`.

### Reusable tooling produced (recreate from this doc if absent)
- Deterministic harness: `ISH_DET_RANDOM` (fixed `get_random`) + monotonic clock in
  `kernel/time.c`.
- Env-gated C probes in `emu/tlb.c`: force-miss page watchpoint, `[SRC]` constructor probe
  (reads `frame->cpu` regs + `mmu_translate`), `watch_arm_hook` for lldb.
- `kernel/memory.c mem_page_state()` — query a page's `pt_entry` flags/backing.
- lldb HW-watchpoint driver `/tmp/wp_cb.py` (`arm` + `wphit`).
- Crash-site dump in `kernel/calls.c` INT_GPF path.

All probes are **env-gated and off by default**. The richer per-session trail (every ruled-out
hypothesis with evidence) lives in the agent memory note `ios-linuxkit-bun-segv-stackzero`.
