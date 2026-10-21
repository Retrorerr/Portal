/* Pure cBPF builder. See filter_build.h for the API contract.
 *
 * Lives in PROD_C_FOR_TESTS so the cleat orchestrator can build the
 * program in its own address space (without touching seccomp(2)) and
 * walk it through a tiny BPF interpreter for table-driven testing.
 *
 * This file MUST stay free of `raw_sys.h`, `fdtab.h`, or any header
 * that pulls in our process-globals — otherwise the hosted glibc test
 * build clashes with `_start`/raw-syscall expectations. The dispatch-
 * table source-of-truth is upstream of this builder; trap_nrs comes
 * in as a plain array.
 */

#include <stddef.h>
#include <stdint.h>

#include <linux/filter.h>
#include <linux/seccomp.h>
#include <linux/sched.h>

#include "errno_neg.h"
#include "filter_build.h"
#include "sysnr.h"

/* sock_filter constructors. Plain brace initializers (not compound
 * literals with an explicit cast — clang rejects those in array-
 * initializer position with `-Werror -Wmissing-braces`).
 *
 * sock_filter layout: { __u16 code, __u8 jt, __u8 jf, __u32 k }.
 *
 * <linux/filter.h> reuses `BPF_A` as an accumulator-source flag,
 * hence the TAWC_ prefix on these. */
#define TAWC_BPF_S(_code, _k)            { (_code), 0, 0, (_k) }
#define TAWC_BPF_J(_code, _k, _jt, _jf)  { (_code), (_jt), (_jf), (_k) }

long tawcroot_build_filter(struct sock_filter *prog, size_t prog_cap,
			   const int *trap_nrs, size_t n_traps,
			   uint64_t stub_ret_addr,
			   int reserved_fd_floor,
			   uint32_t audit_arch)
{
	if (!prog || prog_cap == 0) return TAWC_EINVAL;
	if (n_traps > 1900) return TAWC_E2BIG;  /* kernel cap is 4096 */

	const uint32_t stub_lo = (uint32_t)(stub_ret_addr & 0xffffffffu);
	const uint32_t stub_hi = (uint32_t)(stub_ret_addr >> 32);
	size_t i = 0;

	/* Cap-check helper. The fixed-size sequences below are short
	 * enough that we just check up front per emit; clearer than
	 * threading a "did we overflow" flag. */
#define EMIT_OR_FAIL(_ins) do {                                            \
		if (i >= prog_cap) return TAWC_E2BIG;                      \
		prog[i++] = (struct sock_filter)_ins;                      \
	} while (0)

	/* Prologue: arch check, IP allowlist (low/high 32). */
	EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 4));
	EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JEQ | BPF_K, audit_arch, 1, 0));
	EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS));

	EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 8));
	EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JEQ | BPF_K, stub_lo, 0, 3));
	EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 12));
	EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JEQ | BPF_K, stub_hi, 0, 1));
	EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));

	/* Load syscall_nr once, then linear JEQ + TRAP for each trap_nr.
	 * `close` is special-cased to a RANGE compare: TRAP only when
	 * args[0] >= reserved_fd_floor. The range is a deliberate superset
	 * of the reserved table — the handler makes the exact membership
	 * decision and forwards the real close for anything else — so fds
	 * reserved AFTER filter install (shm_open, post-chroot root fd)
	 * are protected too. The earlier per-fd JEQ list baked in only
	 * the install-time set and let a guest close() of a runtime-
	 * reserved fd through to the kernel (silent state corruption; see
	 * issues/tawcroot-close-fastpath-misses-runtime-reserved-fds.md).
	 * The fast path still skips the handler for the common closefrom
	 * loop's low fds; the high ones TRAP and get forwarded.
	 *
	 * args[0] is a u64 fd; we compare the low 32 bits only (fds never
	 * exceed INT_MAX, and a negative fd's 0xFFFF.... low word is ≥
	 * floor → traps → handler forwards the real close → kernel EBADF,
	 * which is correct). */
	EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 0));
	/* Guest signal handlers can edit ucontext directly, bypassing
	 * sigprocmask. Sanitize their restored mask; our own returns use the
	 * allowlisted raw stub and do not recursively trap. */
	EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JEQ | BPF_K, TAWC_SYS_rt_sigreturn, 0, 1));
	EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_TRAP));

	for (size_t t = 0; t < n_traps; t++) {
		/* Never route shared-VM thread stack switches through a C handler. */
		if (trap_nrs[t] == TAWC_SYS_clone) {
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JEQ | BPF_K, (uint32_t)trap_nrs[t], 0, 4));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 16));
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JSET | BPF_K,
				CLONE_VM, 1, 0));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_TRAP));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 0));
			continue;
		}
		/* fd-shape syscalls whose handlers only act on reserved fds
		 * (close/dup: our own fds; write: the namespace id-map memfds,
		 * which are reserved fds too) TRAP only at or above the
		 * floor. `write` and `dup` are among the hottest syscalls a
		 * desktop makes; below the floor they run natively. */
		if ((trap_nrs[t] == TAWC_SYS_close || trap_nrs[t] == TAWC_SYS_write ||
		     trap_nrs[t] == TAWC_SYS_dup) && reserved_fd_floor > 0) {
			/* not-nr jf skips: LD args, JGE, RET TRAP,
			 * RET ALLOW = 4, landing on the LD-nr reload. */
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JEQ | BPF_K,
					(uint32_t)trap_nrs[t], 0, 4));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 16));
			/* args[0] >= floor → fall through to RET TRAP;
			 * else jump +1 to RET ALLOW. */
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JGE | BPF_K,
					(uint32_t)reserved_fd_floor, 0, 1));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_TRAP));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
			/* Re-load nr so subsequent JEQs see A == nr. The
			 * not-nr jf above lands here. */
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 0));
			continue;
		}
		/* fcntl: reserved fds, plus the shm seal emulation, which
		 * applies to guest fds (F_ADD_SEALS 1033 / F_GET_SEALS 1034).
		 * Everything else (O_NONBLOCK, FD_CLOEXEC, locks) is native. */
		if (trap_nrs[t] == TAWC_SYS_fcntl && reserved_fd_floor > 0) {
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JEQ | BPF_K,
					(uint32_t)trap_nrs[t], 0, 7));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 16));
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JGE | BPF_K,
					(uint32_t)reserved_fd_floor, 3, 0));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 24));
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JGE | BPF_K, 1033, 0, 2));
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JGT | BPF_K, 1034, 1, 0));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_TRAP));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 0));
			continue;
		}
		/* ioctl: the handler answers only evdev requests (type 'E',
		 * emulated game pads) and the termios2 fallbacks
		 * (TCGETS2..TCSETSF2, 'T' 0x2A-0x2D). Every GPU, DRM, sound
		 * and terminal ioctl runs natively. */
		if (trap_nrs[t] == TAWC_SYS_ioctl) {
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JEQ | BPF_K,
					(uint32_t)trap_nrs[t], 0, 9));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 24));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_ALU | BPF_AND | BPF_K, 0xff00));
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JEQ | BPF_K, 'E' << 8, 4, 0));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 24));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_ALU | BPF_AND | BPF_K, 0xffff));
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JGE | BPF_K, 0x542A, 0, 2));
			EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JGT | BPF_K, 0x542D, 1, 0));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_TRAP));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));
			EMIT_OR_FAIL(TAWC_BPF_S(BPF_LD | BPF_W | BPF_ABS, 0));
			continue;
		}
		EMIT_OR_FAIL(TAWC_BPF_J(BPF_JMP | BPF_JEQ | BPF_K,
				(uint32_t)trap_nrs[t], 0, 1));
		EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_TRAP));
	}

	EMIT_OR_FAIL(TAWC_BPF_S(BPF_RET | BPF_K, SECCOMP_RET_ALLOW));

#undef EMIT_OR_FAIL
	return (long)i;
}
