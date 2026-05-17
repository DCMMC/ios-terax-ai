#include <stdbool.h>
#include <stdlib.h>

#include <linux/threads.h>
#include <asm/ptrace.h>
#include <emu/exec.h>
#include <emu/kernel.h>

#include "irq_user.h"

#include "emu/cpu.h"
#include "emu/tlb.h"
#include "emu/interrupt.h"
#define ENGINE_ASBESTOS 1
#include "asbestos/asbestos.h"

struct emu_mm_ctx {
    struct mmu mmu;
    struct emu_mm *emu_mm;
};

static __thread struct tlb the_tlb;
static bool poke[NR_CPUS];

static void *terax_translate(struct mmu *mem, addr_t addr, int type) {
    struct emu_mm *emu_mm = container_of(mem, struct emu_mm_ctx, mmu)->emu_mm;
    bool writable;
    void *ptr = user_to_kernel_emu(emu_mm, addr, &writable);
    if (ptr && type == MEM_WRITE && !writable) {
        ptr = NULL;
    }
    return ptr;
}

static struct mmu_ops terax_mmu_ops = {
    .translate = terax_translate,
};

static void regs_to_cpu(struct pt_regs *regs, struct cpu_state *cpu) {
    struct emu_mm_ctx *mm_ctx = cpu->mmu ? container_of(cpu->mmu, struct emu_mm_ctx, mmu) : NULL;
    cpu->x0 = regs->trap_nr == INT_SYSCALL ? regs->ax : regs->bx;
    cpu->x1 = regs->cx;
    cpu->x2 = regs->dx;
    cpu->x3 = regs->si;
    cpu->x4 = regs->di;
    cpu->x5 = regs->bp;
    cpu->x8 = regs->trap_nr == INT_SYSCALL ? regs->orig_ax : regs->ax;
    cpu->sp = regs->sp;
    cpu->pc = regs->ip;
    cpu->nzcv = (uint32_t) regs->flags;
    arm64_set_nzcv(cpu, cpu->nzcv);
    cpu->tls_ptr = regs->tls;
    if (mm_ctx) {
        cpu->mmu = &mm_ctx->mmu;
    }
}

static void cpu_to_regs(struct cpu_state *cpu, struct pt_regs *regs) {
    collapse_flags(cpu);
    regs->ax = cpu->x8;
    regs->bx = cpu->x0;
    regs->cx = cpu->x1;
    regs->dx = cpu->x2;
    regs->si = cpu->x3;
    regs->di = cpu->x4;
    regs->bp = cpu->x5;
    regs->sp = cpu->sp;
    regs->ip = cpu->pc;
    regs->flags = cpu->nzcv;
    regs->tls = cpu->tls_ptr;
}

static void emu_run_to_interrupt(struct emu *emu, struct cpu_state *cpu) {
    struct pt_regs *regs = emu_pt_regs(emu);
    struct emu_mm_ctx *mm_ctx = emu->mm->ctx;

    cpu->mmu = &mm_ctx->mmu;
    regs_to_cpu(regs, cpu);
    cpu->poked_ptr = &poke[get_smp_processor_id()];

    int interrupt = cpu_run_to_interrupt(cpu, &the_tlb);
    cpu_to_regs(cpu, regs);

    if (interrupt == INT_GPF) {
        regs->cr2 = cpu->segfault_addr;
        regs->error_code = cpu->segfault_was_write << 1;
    } else {
        regs->cr2 = regs->error_code = 0;
    }
    regs->trap_nr = interrupt;
}

void emu_run(struct emu *emu) {
    struct cpu_state cpu = {};
    if (emu->snapshot) {
        struct cpu_state *snapshot = emu->snapshot;
        cpu = *snapshot;
        free(snapshot);
        emu->snapshot = NULL;
    }
    emu->ctx = &cpu;
    for (;;) {
        emu_run_to_interrupt(emu, &cpu);
        handle_cpu_trap(emu);
    }
}

void emu_finish_fork(struct emu *emu) {
    struct cpu_state *cpu = emu->ctx;
    struct cpu_state *snapshot = emu->snapshot = calloc(1, sizeof(*snapshot));
    *snapshot = *cpu;
    emu->ctx = NULL;
}

void emu_destroy(struct emu *emu) {
    (void) emu;
}

void emu_poke_cpu(int cpu) {
    __atomic_store_n(&poke[cpu], true, __ATOMIC_SEQ_CST);
}

void emu_flush_tlb_local(struct emu_mm *mm, unsigned long start, unsigned long end) {
    if (the_tlb.mmu == NULL) {
        return;
    }
    tlb_flush(&the_tlb);
    struct emu_mm_ctx *mm_ctx = mm->ctx;
    if (mm_ctx->mmu.asbestos != NULL) {
        asbestos_invalidate_range(mm_ctx->mmu.asbestos, start / PAGE_SIZE, (end + PAGE_SIZE - 1) / PAGE_SIZE);
    }
}

void emu_mmu_init(struct emu_mm *mm) {
    struct emu_mm_ctx *mm_ctx = mm->ctx = calloc(1, sizeof(*mm_ctx));
    mm_ctx->emu_mm = mm;
    mm_ctx->mmu.asbestos = asbestos_new(&mm_ctx->mmu);
    mm_ctx->mmu.ops = &terax_mmu_ops;
}

void emu_mmu_destroy(struct emu_mm *mm) {
    struct emu_mm_ctx *mm_ctx = mm->ctx;
    asbestos_free(mm_ctx->mmu.asbestos);
    mm_ctx->mmu.asbestos = NULL;
}

void emu_switch_mm(struct emu *emu, struct emu_mm *mm) {
    (void) emu;
    struct emu_mm_ctx *mm_ctx = mm->ctx;
    tlb_refresh(&the_tlb, &mm_ctx->mmu);
}
