/* chroot(2) changes the runtime's path view, not the kernel root.
 * Resolve and pin the target, re-anchor binds in guest coordinates, then
 * replace the root fd/path and rebuild symlink memoization. Retain the selected
 * guest route: reverse host lookup can choose a different alias of that directory.
 *
 * CLONE_FS peers synchronize the root path through shared_root below. Ordinary
 * fork keeps an independent snapshot; exec transports the active bind table.
 * Startup view changes are expected to be serialized by the caller. Old root
 * and inactive bind fds remain reserved until exec or process exit, bounded
 * by TAWCROOT_MAX_RESERVED_FDS. No kernel mount isolation is provided.
 */

#include <stddef.h>
#include <stdint.h>
#include <ucontext.h>
#include <sys/mman.h>
#include <stdatomic.h>

#include "chroot.h"
#include "dispatch.h"
#include "errno_neg.h"
#include "fdtab.h"
#include "id_beacon.h"
#include "io.h"
#include "path.h"
#include "path_scratch.h"
#include "raw_sys.h"
#include "syscalls_control.h"
#include "sysnr.h"
#include "tawc_string.h"
#include "tawc_uapi.h"
#include "usercopy.h"

/* Only CLONE_FS peers share this mapping. Ordinary fork snapshots the local
 * view, unshare detaches, and exec serializes the synchronized local view.
 * Store host and guest paths: peers need both the fd and its original route. */
struct shared_root {
    atomic_uint sequence;
    int ro;
    char path[4096];
    char guest[4096];
};
static struct shared_root *shared;
static unsigned seen;

long tawcroot_fs_share(void)
{
    if (shared) return 0;
    long p = tawc_mmap(0, sizeof(*shared), PROT_READ | PROT_WRITE,
                      MAP_SHARED | MAP_ANONYMOUS, -1, 0);
    if (p < 0 && p >= -4095) return p;
    shared = (void *)p;
    shared->ro = tawcroot_root_ro;
    memcpy(shared->path, tawcroot_rootfs_host_path, tawcroot_rootfs_host_path_len + 1);
    atomic_store(&shared->sequence, 0);
    seen = 0;
    return 0;
}

void tawcroot_fs_detach(void)
{
    if (shared) tawc_munmap(shared, sizeof(*shared));
    shared = 0;
    seen = 0;
}

/* Translate the guest's chroot target to (base_fd, suffix) and open
 * an O_PATH dirfd for it. On success returns >=0 (the new fd, not yet
 * reserved) and sets *ro_out to the translation's RO bit; on failure
 * returns -errno. Chroot itself is a read operation and is ALLOWED
 * into an RO bind (like the kernel); the bit is what makes the whole
 * root view read-only after the swap. */
static long open_chroot_target(const char *guest_path, int *ro_out, char *guest)
{
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *path_buf = scratch->buf[0];
	long n = tawc_copy_string_from_guest(
		path_buf, TAWCROOT_PATH_SCRATCH_SIZE, guest_path);
	if (n < 0) return n;

	char *suffix = scratch->buf[1];
	tawcroot_path_result r = tawcroot_path_translate(
		path_buf, suffix, TAWCROOT_PATH_SCRATCH_SIZE,
		TAWCROOT_PATH_FOLLOW, TAWCROOT_PATH_INTENT_READ);
	/* An inherited directory fd may retain access outside the old root,
	 * just as on Linux. Preserve ordinary bind RO checks when in view. */
	if (r.err == TAWC_ENOENT && tawc_streq(path_buf, ".")) {
		*ro_out = 0;
		guest[0] = 0;
		return tawc_openat(AT_FDCWD, ".", O_PATH | O_DIRECTORY | O_CLOEXEC, 0);
	}
	if (r.err) return r.err;
	*ro_out = r.ro;
	long gr = tawcroot_path_route_to_guest(r.base_fd, suffix, guest, 4096);
	if (gr < 0) return gr;

	/* tawcroot_path_translate writes "" into suffix when the request
	 * resolves to the directory base_fd already refers to (e.g.,
	 * chroot("/") with no binds). openat() on "" returns -ENOENT on
	 * kernels < 6.6 even with AT_EMPTY_PATH; openat on "." resolves
	 * to "the directory base_fd refers to" on every kernel we
	 * target, which is what we want. */
	const char *p = suffix[0] ? suffix : ".";
	int flags = O_PATH | O_DIRECTORY | O_CLOEXEC;
	return tawc_openat(r.base_fd, p, flags, 0);
}

static long apply_root(long new_fd, int new_root_ro, const char *guest)
{
	long resv = tawcroot_fd_reserve((int)new_fd);
	if (resv < 0) {
		tawc_close((int)new_fd);
		return resv;
	}
	int new_root_fd = (int)resv;

	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *new_host = scratch->buf[0];
	long hp = tawcroot_proc_fd_to_host_path(new_root_fd, new_host,
	                                        TAWCROOT_PATH_SCRATCH_SIZE);
	if (hp < 0) {
		/* Can't recover the host path — abort. The reserved fd
		 * stays in tawcroot_reserved_fds, leaking until process
		 * exit, but the guest sees the rootfs view unchanged. */
		return hp;
	}
	size_t new_host_len = (size_t)hp;

	/* Bind destinations are guest coordinates, not host paths. In particular,
	 * /tmp may itself be a bind outside the rootfs. Re-anchor in guest space
	 * so nested mappings survive chroot into that directory. */
	char *new_guest = scratch->buf[1];
	long gp;
	if (guest && *guest) {
		gp = (long)strlen(guest);
		memcpy(new_guest, guest, (size_t)gp + 1);
	} else gp = tawcroot_fd_to_guest_abs(new_root_fd, new_guest,
	                                   TAWCROOT_PATH_SCRATCH_SIZE);
	if (gp < 0) return gp;
	if (!tawc_streq(new_guest, "/"))
		(void)tawcroot_path_binds_reanchor(tawcroot_binds, tawcroot_n_binds,
		                                  "", 0, new_guest, (size_t)gp);

	/* Swap rootfs_fd + host_path.
	 *
	 * Update order: clear `_len = 0` first so a concurrent reader
	 * doing the standard "read len, then iterate len bytes" pattern
	 * sees either the OLD complete view or a transient zero-length
	 * (interpreted by every reader as "cwd outside the rootfs"
	 * → -ENOENT). Then overwrite the path bytes (only the first
	 * new_host_len + 1 are written; tail bytes from any longer prior
	 * value retain stale data but are unreachable since every reader
	 * bounds on `_len`). Finally publish the new length and swap fd.
	 *
	 * This is NOT a full memory ordering guarantee — without
	 * explicit barriers, an aarch64 reader could observe the new
	 * `_len` before the path bytes — but the failure mode is bounded
	 * (the reader returns -ENOENT and the guest retries, which
	 * works post-update). See notes/tawcroot/path-translation.md §"chroot emulation"
	 * for the full race analysis. */
	tawcroot_rootfs_host_path_len = 0;
	for (size_t i = 0; i < new_host_len; i++)
		tawcroot_rootfs_host_path[i] = new_host[i];
	tawcroot_rootfs_host_path[new_host_len] = 0;
	tawcroot_rootfs_host_path_len = new_host_len;
	tawcroot_rootfs_fd = new_root_fd;
	/* Chroot into an RO bind dst makes the whole view read-only:
	 * after the swap, routing goes through rootfs_fd rather than the
	 * (now re-anchored / deactivated) bind, so the flag must ride the
	 * root-view globals. Chroot into an RW target from an RO root
	 * un-sets it, like the kernel's mount flags would. An RW bind
	 * nested under the RO dst stays writable via the surviving
	 * re-anchored bind entry — longest-prefix match, no extra code. */
	tawcroot_root_ro = new_root_ro;

	/* Re-memoize against the new root. The old memos (lib → usr/lib
	 * etc.) point at the OUTER root's symlinks; rebuilding picks up
	 * whatever the inner root's well-known directories actually
	 * resolve to. Empty (no symlinks at the well-known names) is
	 * fine — the symlink resolver covers everything memos miss. */
	tawcroot_path_memoize_well_known();
	tawcroot_root_beacon_update();

	return 0;
}

long tawcroot_fs_sync(void)
{
    if (!shared) return 0;
    unsigned version = atomic_load_explicit(&shared->sequence, memory_order_acquire);
    if (version == seen) return 0;
    if (version & 1) return TAWC_EAGAIN;
    TAWCROOT_PATH_SCRATCH_AUTO(scratch);
    memcpy(scratch->buf[0], shared->path, sizeof(shared->path));
    memcpy(scratch->buf[1], shared->guest, sizeof(shared->guest));
    int ro = shared->ro;
    if (atomic_load_explicit(&shared->sequence, memory_order_acquire) != version)
        return TAWC_EAGAIN;
    long fd = tawc_openat(AT_FDCWD, scratch->buf[0], O_PATH | O_DIRECTORY | O_CLOEXEC, 0);
    if (fd < 0) return fd;
    long result = apply_root(fd, ro, scratch->buf[1]);
    if (!result) seen = version;
    return result;
}

static long handle_chroot(const tawcroot_syscall_args *args, ucontext_t *uc)
{
    (void)uc;
    if (!args->a) return TAWC_EFAULT;
    int ro = 0;
    TAWCROOT_PATH_SCRATCH_AUTO(scratch);
    char *guest = scratch->buf[0];
    long fd = open_chroot_target((void *)(uintptr_t)args->a, &ro, guest);
    if (fd < 0) return fd;
    unsigned version = seen;
    if (shared && !atomic_compare_exchange_strong(&shared->sequence, &version, seen + 1)) {
        tawc_close((int)fd);
        return TAWC_EAGAIN;
    }
    long result = apply_root(fd, ro, guest);
    if (shared) {
        if (!result) {
            memcpy(shared->path, tawcroot_rootfs_host_path, tawcroot_rootfs_host_path_len + 1);
            memcpy(shared->guest, guest, strlen(guest) + 1);
            shared->ro = ro;
            seen += 2;
        }
        atomic_store_explicit(&shared->sequence, seen, memory_order_release);
    }
    return result;
}

/* pivot_root(new_root, put_old), as sandboxes build their root
 * (bubblewrap: pivot into a tmpfs with the old root at "oldroot", bind
 * from /oldroot into /newroot, detach oldroot, then pivot_root(".",
 * ".") into newroot). The view's root becomes new_root, as chroot; the
 * old view (its root and every bind outside new_root) moves to put_old.
 * put_old == new_root stacks the old root on the new one, which the
 * caller detaches at once: that is a chroot. Not supported between
 * CLONE_FS peers. */
static long handle_pivot_root(const tawcroot_syscall_args *args, ucontext_t *uc)
{
	(void)uc;
	if (!args->a || !args->b) return TAWC_EFAULT;
	if (shared) return TAWC_EINVAL;
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *guest = scratch->buf[0], *put = scratch->buf[1];
	char *rel = scratch->buf[2], *abs = scratch->buf[3];
	const size_t cap = TAWCROOT_PATH_SCRATCH_SIZE;
	int ro = 0;
	long fd = open_chroot_target((void *)(uintptr_t)args->a, &ro, guest);
	if (fd < 0) return fd;
	long e = tawc_copy_string_from_guest(rel, cap, (const char *)args->b);
	size_t pos = 0;
	if (e >= 0 && rel[0] != '/') {
		e = tawcroot_cwd_to_guest_abs(abs, cap);
		if (e >= 0) {
			pos = (size_t)e;
			e = tawc_str_append(abs, cap, &pos, "/");
		}
	}
	if (e >= 0) e = tawc_str_append(abs, cap, &pos, rel);
	if (e >= 0) e = tawcroot_path_fold_absolute(abs, put, cap);
	if (!guest[0] || guest[0] != '/') e = TAWC_EINVAL;
	if (e < 0) {
		tawc_close((int)fd);
		return e;
	}
	/* put (folded) and nr carry no leading '/'. */
	const char *nr = guest + 1;
	size_t nl = strlen(nr);
	if (tawc_streq(put, nr)) return apply_root(fd, ro, guest);

	/* put_old must lie below new_root. */
	if (!put[0] || (nl && (memcmp(put, nr, nl) != 0 || put[nl] != '/'))) {
		tawc_close((int)fd);
		return TAWC_EINVAL;
	}
	/* Move the old view under put_old (still in old coordinates, so
	 * apply_root's re-anchoring keeps it): binds outside new_root... */
	for (size_t i = 0; i < tawcroot_n_binds; i++) {
		struct tawcroot_bind *b = &tawcroot_binds[i];
		if (!b->active || b->pinned) continue;
		if (nl && b->dst_len >= nl && memcmp(b->dst, nr, nl) == 0 &&
		    (b->dst[nl] == '/' || b->dst[nl] == 0))
			continue;
		size_t n = 0;
		char *d = abs;
		if (tawc_str_append(d, cap, &n, put) ||
		    tawc_str_append(d, cap, &n, "/") ||
		    tawc_str_append(d, cap, &n, b->dst) || n >= sizeof b->dst) {
			b->active = 0;
			continue;
		}
		memcpy(b->dst, d, n + 1);
		b->dst_len = n;
	}
	/* ...and the old root itself. */
	e = tawcroot_path_add_bind(tawcroot_rootfs_host_path, put, tawcroot_root_ro);
	if (e < 0) {
		tawc_close((int)fd);
		return e;
	}
	/* The pinned link-store view stays at the new root and also shows
	 * under put_old, where the caller reopens its sources from. */
	for (size_t i = 0; i < tawcroot_n_binds; i++) {
		struct tawcroot_bind *b = &tawcroot_binds[i];
		if (!b->active || !b->pinned) continue;
		size_t n = 0;
		if (!tawc_str_append(abs, cap, &n, put) &&
		    !tawc_str_append(abs, cap, &n, "/") &&
		    !tawc_str_append(abs, cap, &n, b->dst))
			(void)tawcroot_path_add_bind(b->src, abs, 1);
		break;
	}
	return apply_root(fd, ro, guest);
}

void tawcroot_chroot_register(void)
{
	tawcroot_dispatch_install(TAWC_SYS_chroot, handle_chroot);
	tawcroot_dispatch_install(TAWC_SYS_pivot_root, handle_pivot_root);
}
