/* aarch64 arch helpers. Kernel-syscall ABI: x8 (nr) + x0..x5 (args).
 *
 * `siginfo_t::si_call_addr` and `seccomp_data::instruction_pointer` both point
 * to the POST-svc PC — the address of the instruction immediately after the
 * 4-byte `svc #0` — because the kernel populates them from `pt_regs->pc`,
 * which the syscall-entry asm has already advanced past the svc. This is the
 * same value as `mcontext_t::pc` for a SECCOMP_RET_TRAP delivery. The IP
 * allowlist in filter.c therefore matches against the `_ret` label, not the
 * `_insn` label. `mcontext.regs[0]` is x0 — both the first arg and the
 * return slot. Empirically verified — see `src/filter.c` and the foundation
 * smoke driver.
 */

#pragma once

#include <stdint.h>
#include <signal.h>
#include <ucontext.h>
#include <linux/audit.h>

#define TAWCROOT_AUDIT_ARCH AUDIT_ARCH_AARCH64

static inline void tawcroot_arch_read_args(const ucontext_t *uc,
					   tawcroot_syscall_args *out)
{
	const __u64 *r = uc->uc_mcontext.regs;
	out->nr = (long)uc->uc_mcontext.regs[8];
	out->a  = (long)r[0];
	out->b  = (long)r[1];
	out->c  = (long)r[2];
	out->d  = (long)r[3];
	out->e  = (long)r[4];
	out->f  = (long)r[5];
}

static inline void tawcroot_arch_write_return(ucontext_t *uc, long rv)
{
	uc->uc_mcontext.regs[0] = (__u64)rv;
}

static inline uintptr_t tawcroot_arch_sp(const ucontext_t *uc)
{
	return (uintptr_t)uc->uc_mcontext.sp;
}

static inline uintptr_t tawcroot_arch_resume_pc(const ucontext_t *uc)
{
	return (uintptr_t)uc->uc_mcontext.pc;
}

/* Three-argument syscall issued with SP temporarily at `sp`. The kernel
 * reads the caller's SP for some checks (SELinux execstack). The caller
 * must block signals: nothing may run on `sp`. */
static inline long tawcroot_arch_syscall3_on_sp(long nr, long a, long b,
						long c, uintptr_t sp)
{
	register long x0 __asm__("x0") = a;
	register long x1 __asm__("x1") = b;
	register long x2 __asm__("x2") = c;
	register long x8 __asm__("x8") = nr;
	__asm__ volatile("mov x16, sp\n\t"
			 "mov sp, %[sp]\n\t"
			 "svc #0\n\t"
			 "mov sp, x16"
			 : "+r"(x0)
			 : "r"(x1), "r"(x2), "r"(x8), [sp] "r"(sp)
			 : "x16", "memory");
	return x0;
}
