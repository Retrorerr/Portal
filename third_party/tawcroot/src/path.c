/* Path translation.
 *
 * Today's policy:
 *   - Absolute guest path:
 *       Tokenize on `/`, fold `.` and `..` (clamp at root if they would
 *       escape — proot-compatible behavior described in
 *       notes/tawcroot/path-translation.md "..&symlink escapes are blocked"). The result
 *       is a rootfs-relative suffix used with the rootfs O_PATH fd via
 *       *at syscalls.
 *   - Relative guest path:
 *       Resolve against the kernel cwd via raw `getcwd`. Strip the host
 *       rootfs prefix (so a kernel cwd of `<rootfs>/foo` becomes guest
 *       `/foo`). Then concatenate the relative remainder and re-run the
 *       absolute translator on the joined path.
 *       If the kernel cwd is outside the rootfs, return -ENOENT (don't
 *       leak host paths through guest-visible errors).
 *   - Empty result ("/") → suffix = "", caller passes AT_EMPTY_PATH.
 *
 * No allocations, no libc, async-signal-safe. Path buffers live on the
 * caller's stack (4 KB PATH_MAX is the hard cap). The `..`-fold lives
 * in path_fold.c; symlink resolution (parameterized per syscall
 * semantics: follow / no-follow / parent-for-create / parent-for-remove
 * — see notes/tawcroot/path-translation.md §"Translation rules") is wired up through the
 * oracle in the translate ctx and implemented in path_resolve.c.
 */

#include <stddef.h>
#include <stdint.h>

#include <sys/stat.h>

#include "errno_neg.h"
#include "fdtab.h"
#include "io.h"
#include "linkstore.h"
#include "path.h"
#include "path_oracle.h"
#include "path_orchestrate.h"
#include "path_resolve.h"
#include "path_scratch.h"
#include "proc_shadow.h"
#include "raw_sys.h"
#include "rescue.h"
#include "sysnr.h"
#include "tawc_string.h"
#include "tawc_uapi.h"
#include "usercopy.h"

int    tawcroot_rootfs_fd               = -1;
int    tawcroot_root_ro                 = 0;
char   tawcroot_rootfs_host_path[4096]  = {0};
size_t tawcroot_rootfs_host_path_len    = 0;

char   tawcroot_guest_exe_path[4096]    = {0};
size_t tawcroot_guest_exe_path_len      = 0;

char   tawcroot_self_host_path[4096]    = {0};
size_t tawcroot_self_host_path_len      = 0;

void tawcroot_set_guest_exe_path(const char *path)
{
	if (!path) {
		tawcroot_guest_exe_path[0] = 0;
		tawcroot_guest_exe_path_len = 0;
		return;
	}

	/* Canonicalize through the rootfs symlink view so /proc/self/exe
	 * synthesis returns what the kernel would have under a real chroot:
	 * the post-symlink real-path of the executable. Without this,
	 * $ORIGIN-rooted RPATH/RUNPATH lookups break when the exec target
	 * goes through a symlink — e.g. Firefox launched via /usr/sbin/firefox
	 * (→ /usr/bin/firefox → /usr/lib/firefox/firefox) would have $ORIGIN
	 * resolve to /usr/sbin and fail to dlopen libxul.so ("Couldn't load
	 * XPCOM"). Only applies when the resolved path stays in the rootfs
	 * view (base_fd == rootfs_fd); paths that route through a bind
	 * fall through to verbatim because the translator's suffix is
	 * relative to the bind src, not guest-absolute, and reconstructing
	 * the guest path from base_fd matters too little to justify the
	 * extra plumbing — exec'd binaries live in the rootfs proper in
	 * every flow we care about. */
	if (tawcroot_rootfs_fd >= 0) {
		TAWCROOT_PATH_SCRATCH_AUTO(scratch);
		char *suffix = scratch->buf[0];
		tawcroot_path_result r = tawcroot_path_translate(
			path, suffix, TAWCROOT_PATH_SCRATCH_SIZE,
			TAWCROOT_PATH_FOLLOW, TAWCROOT_PATH_INTENT_READ);
		if (r.err == 0 && r.base_fd == tawcroot_rootfs_fd) {
			size_t pos = 0;
			(void)tawc_str_append(tawcroot_guest_exe_path,
					      sizeof tawcroot_guest_exe_path,
					      &pos, "/");
			(void)tawc_str_append(tawcroot_guest_exe_path,
					      sizeof tawcroot_guest_exe_path,
					      &pos, suffix);
			tawcroot_guest_exe_path_len = pos;
			return;
		}
	}

	long n = tawc_str_copy(tawcroot_guest_exe_path,
			       sizeof tawcroot_guest_exe_path, path);
	tawcroot_guest_exe_path_len = n < 0 ? 0 : (size_t)n;
}

void tawcroot_init_self_host_path(void)
{
	long n = tawc_readlinkat(AT_FDCWD, "/proc/self/exe",
				 tawcroot_self_host_path,
				 sizeof tawcroot_self_host_path - 1);
	/* Exact-fit means possibly truncated; a truncated path would
	 * false-match prefixes in the /proc/self/exe substitution, so
	 * treat it like failure (substitution just never fires). */
	if (n < 0 || (size_t)n >= sizeof tawcroot_self_host_path - 1) {
		tawcroot_self_host_path[0]   = 0;
		tawcroot_self_host_path_len  = 0;
		return;
	}
	tawcroot_self_host_path[n]   = 0;
	tawcroot_self_host_path_len  = (size_t)n;
}

struct tawcroot_bind tawcroot_binds[TAWCROOT_MAX_BINDS];
size_t               tawcroot_n_binds = 0;

long tawcroot_proc_fd_to_host_path(int fd, char *out, size_t out_cap)
{
	if (!out || out_cap < 2) return TAWC_EINVAL;

	char link[64];
	if (tawc_proc_fd_path(link, sizeof link, fd, 0) < 0)
		return TAWC_EINVAL;

	long n = tawc_readlinkat(AT_FDCWD, link, out, out_cap - 1);
	if (n < 0) return n;
	if ((size_t)n >= out_cap - 1) return TAWC_ENAMETOOLONG;
	/* Empty / non-absolute readlink result is a /proc-not-mounted style
	 * failure. Surface as -EINVAL so callers can distinguish it from a
	 * "path is too long" overflow. In practice we're already broken if
	 * /proc isn't mounted (chroot init also depends on it), so the
	 * precise errno doesn't matter much. */
	if (n == 0 || out[0] != '/') return TAWC_EINVAL;
	while (n > 1 && out[n - 1] == '/') n--;
	out[n] = 0;
	return n;
}

/* Helper: copy `host[start..n)` into `out` as a guest-absolute path
 * with leading '/' (preserving / collapsing internal '/' runs). Used by
 * both the rootfs-prefix and bind-src branches of tawcroot_fd_to_guest_abs.
 * Returns the written length, or -ENAMETOOLONG. */
static long write_guest_abs_suffix(char *out, size_t out_cap,
                                   const char *host, size_t start, size_t n,
                                   const char *bind_dst, size_t bind_dst_len)
{
	size_t off = 0;
	if (off + 1 >= out_cap) return TAWC_ENAMETOOLONG;
	out[off++] = '/';
	for (size_t i = 0; i < bind_dst_len; i++) {
		if (off + 1 >= out_cap) return TAWC_ENAMETOOLONG;
		out[off++] = bind_dst[i];
	}
	for (size_t i = start; i < n; i++) {
		if (host[i] == '/' && off > 0 && out[off - 1] == '/') continue;
		if (off + 1 >= out_cap) return TAWC_ENAMETOOLONG;
		out[off++] = host[i];
	}
	out[off] = 0;
	return (long)off;
}

/* True iff `host[0..n)` starts with `prefix[0..pl)` and the next byte (if
 * any) is `/` — i.e. the prefix matches at a component boundary. Uses
 * tawcroot_host_path_under (see include/path.h) so a rootfs of "/"
 * also counts as a covering prefix. */
static int host_prefix_match(const char *host, size_t n,
                             const char *prefix, size_t pl)
{
	return tawcroot_host_path_under(host, n, prefix, pl);
}

long tawcroot_host_path_to_guest_abs(const char *host, size_t n,
				     char *out, size_t out_cap)
{
	if (!host || !out) return TAWC_EFAULT;
	if (out_cap < 2)   return TAWC_ENAMETOOLONG;

	const struct tawcroot_bind *best_bind = 0;
	size_t best_pl = 0;

	int in_rootfs = host_prefix_match(host, n, tawcroot_rootfs_host_path,
	                                  tawcroot_rootfs_host_path_len);
	if (in_rootfs)
		best_pl = tawcroot_rootfs_host_path_len;

	for (size_t bi = 0; bi < tawcroot_n_binds; bi++) {
		const struct tawcroot_bind *b = &tawcroot_binds[bi];
		if (!b->active || b->src_len == 0) continue;
		if (!host_prefix_match(host, n, b->src, b->src_len))
			continue;
		/* A bind of a rootfs directory (Portal's <rootfs>/tmp at
		 * /dev/shm) is an alias: the file's own guest path wins.
		 * Otherwise every cwd under /tmp read back as /dev/shm, and
		 * a hard link there crossed into the bind (EXDEV). */
		if (in_rootfs && !b->leaf[0] &&
		    host_prefix_match(b->src, b->src_len,
		                                   tawcroot_rootfs_host_path,
		                                   tawcroot_rootfs_host_path_len))
			continue;
		if (b->src_len > best_pl) {
			best_bind = b;
			best_pl = b->src_len;
		}
	}

	if (best_bind)
		return write_guest_abs_suffix(out, out_cap, host,
		                              best_pl, n,
		                              best_bind->dst, best_bind->dst_len);
	if (best_pl > 0)
		return write_guest_abs_suffix(out, out_cap, host,
		                              best_pl, n, 0, 0);
	return TAWC_ENOENT;
}

int tawcroot_host_path_in_ro_bind(const char *host, size_t n)
{
	if (!host || n == 0) return 0;

	/* Same longest-prefix walk as tawcroot_host_path_to_guest_abs so
	 * the two can't disagree about which bind (or the rootfs) owns a
	 * host path — an RW bind nested under an RO bind src stays
	 * writable here exactly like it does in translation. */
	int    best_ro = 0;
	size_t best_pl = 0;

	if (host_prefix_match(host, n, tawcroot_rootfs_host_path,
	                      tawcroot_rootfs_host_path_len)) {
		best_pl = tawcroot_rootfs_host_path_len;
		best_ro = tawcroot_root_ro;
	}
	for (size_t bi = 0; bi < tawcroot_n_binds; bi++) {
		const struct tawcroot_bind *b = &tawcroot_binds[bi];
		if (!b->active || b->src_len == 0) continue;
		if (!host_prefix_match(host, n, b->src, b->src_len)) continue;
		if (b->src_len > best_pl) {
			best_pl = b->src_len;
			best_ro = b->read_only;
		}
	}
	return best_ro;
}

long tawcroot_path_route_to_guest(int base_fd, const char *suffix,
                                char *out, size_t out_cap)
{
	const char *prefix = "";
	if (base_fd != tawcroot_rootfs_fd) {
		const struct tawcroot_bind *bind = 0;
		for (size_t i = 0; i < tawcroot_n_binds; ++i)
			if (tawcroot_binds[i].active && tawcroot_binds[i].src_fd == base_fd) {
				bind = &tawcroot_binds[i];
				break;
			}
		if (!bind) return TAWC_ENOENT;
		prefix = bind->dst;
		if (bind->leaf[0]) suffix = "";
	}
	size_t n = 0;
	long e = tawc_str_append(out, out_cap, &n, "/");
	if (!e) e = tawc_str_append(out, out_cap, &n, prefix);
	if (!e && *prefix && *suffix) e = tawc_str_append(out, out_cap, &n, "/");
	if (!e) e = tawc_str_append(out, out_cap, &n, suffix);
	return e ? e : (long)n;
}

/* Names of fds opened through a bind. A host directory bound at two
 * guest places (Portal's <rootfs>/tmp at /tmp and /dev/shm; a sandbox's
 * rbind copies under /newroot and /oldroot) has no single guest name,
 * and the reverse walk picks one. Sandboxes mount onto such fds
 * (bubblewrap: mount(..., "/proc/self/fd/N")), so the name they opened
 * it by is kept here, checked against the inode on every lookup. */
#define FDNAME_SLOTS 64
static struct {
	int      fd;
	uint64_t dev, ino;
	char     path[256];
} g_fdname[FDNAME_SLOTS];
static uint32_t g_fdname_lock;

static int same_inode(int fd, const char *guest, uint64_t dev, uint64_t ino)
{
	struct stat st;
	if (TAWC_RAW(TAWC_SYS_fstat, fd, (long)&st, 0, 0, 0, 0) != 0 ||
	    (uint64_t)st.st_dev != dev || (uint64_t)st.st_ino != ino)
		return 0;
	if (!guest) return 1;
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *name = scratch->buf[0];
	if (tawc_str_copy(name, TAWCROOT_PATH_SCRATCH_SIZE, guest) < 0) return 0;
	tawcroot_path_result r = tawcroot_path_translate(name, scratch->buf[1],
		TAWCROOT_PATH_SCRATCH_SIZE, TAWCROOT_PATH_NOFOLLOW,
		TAWCROOT_PATH_INTENT_READ);
	if (r.err) return 0;
	const char *s = scratch->buf[1];
	if (TAWC_RAW(TAWC_SYS_fstatat, r.base_fd, (long)(*s ? s : "."),
		     (long)&st, AT_SYMLINK_NOFOLLOW, 0, 0) != 0)
		return 0;
	return (uint64_t)st.st_dev == dev && (uint64_t)st.st_ino == ino;
}

void tawcroot_fd_name_note(int fd, const char *guest_abs)
{
	if (fd < 0 || !guest_abs || guest_abs[0] != '/' ||
	    tawc_strlen(guest_abs) >= sizeof g_fdname[0].path)
		return;
	struct stat st;
	if (TAWC_RAW(TAWC_SYS_fstat, fd, (long)&st, 0, 0, 0, 0) != 0) return;
	while (__atomic_exchange_n(&g_fdname_lock, 1, __ATOMIC_ACQUIRE)) {}
	unsigned i = (unsigned)fd % FDNAME_SLOTS;
	g_fdname[i].fd = fd;
	g_fdname[i].dev = (uint64_t)st.st_dev;
	g_fdname[i].ino = (uint64_t)st.st_ino;
	(void)tawc_str_copy(g_fdname[i].path, sizeof g_fdname[i].path, guest_abs);
	__atomic_store_n(&g_fdname_lock, 0, __ATOMIC_RELEASE);
}

static long fd_name_lookup(int fd, char *out, size_t out_cap)
{
	char path[256];
	uint64_t dev, ino;
	unsigned i = (unsigned)fd % FDNAME_SLOTS;
	while (__atomic_exchange_n(&g_fdname_lock, 1, __ATOMIC_ACQUIRE)) {}
	int hit = g_fdname[i].path[0] && g_fdname[i].fd == fd;
	if (hit) {
		dev = g_fdname[i].dev;
		ino = g_fdname[i].ino;
		(void)tawc_str_copy(path, sizeof path, g_fdname[i].path);
	}
	__atomic_store_n(&g_fdname_lock, 0, __ATOMIC_RELEASE);
	if (!hit || !same_inode(fd, path, dev, ino)) return 0;
	long n = tawc_str_copy(out, out_cap, path);
	return n < 0 ? 0 : n;
}

long tawcroot_fd_to_guest_abs(int fd, char *out, size_t out_cap)
{
	if (fd < 0 || fd == AT_FDCWD) return TAWC_EINVAL;
	if (out_cap < 2) return TAWC_ENAMETOOLONG;

	long cached = fd_name_lookup(fd, out, out_cap);
	if (cached > 0) return cached;

	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *host = scratch->buf[0];
	long n = tawcroot_proc_fd_to_host_path(
		fd, host, TAWCROOT_PATH_SCRATCH_SIZE);
	if (n < 0) return n;

	return tawcroot_host_path_to_guest_abs(host, (size_t)n, out, out_cap);
}

/* Well-known-symlink memoization. After fold_absolute, the orchestrator
 * checks each memoized src against the path's leading components
 * (component boundary required). If matched, the prefix is replaced
 * with the target and the fold re-runs so any `..`/`.` in the target
 * collapses. Stored targets are always rootfs-root-anchored: a
 * relative symlink target is relative to the symlink's PARENT
 * directory, so memo_one prepends the src's parent ("usr/sbin" → "bin"
 * is stored as "usr/bin"; "var/run" → "../run" as "var/../run", which
 * the re-fold collapses to "run").
 *
 * The hit set is the typical glibc-rootfs symlink layout. Bumps go
 * through this table, not run-time discovery — finder cost is paid
 * once at init. The match logic itself lives in path_orchestrate.c so
 * tests can supply their own table. */
#define MEMO_MAX 16

static struct tawcroot_symlink_memo g_memo[MEMO_MAX];
static size_t                       g_n_memo = 0;

static void memo_one(const char *prefix)
{
	if (g_n_memo >= MEMO_MAX) return;
	size_t plen = tawc_strlen(prefix);
	if (plen + 1 > sizeof g_memo[0].src) return;

	char buf[256];
	long n = TAWC_RAW(TAWC_SYS_readlinkat, tawcroot_rootfs_fd,
			  (long)prefix, (long)buf, (long)sizeof buf,
			  0, 0);
	if (n <= 0) return;
	if ((size_t)n == sizeof buf) return;  /* truncated — skip memo */
	buf[n] = 0;

	struct tawcroot_symlink_memo *m = &g_memo[g_n_memo++];
	for (size_t i = 0; i < plen; i++) m->src[i] = prefix[i];
	m->src[plen] = 0;
	m->src_len = plen;

	int abs = (buf[0] == '/');
	const char *t = abs ? buf + 1 : buf;
	while (*t == '/') t++;
	size_t tlen = tawc_strlen(t);

	/* Root-anchor the stored target. A relative symlink target is
	 * relative to the symlink's parent directory, so prepend src's
	 * parent (everything up to and including the last '/'); an
	 * absolute target is already root-anchored once the leading '/'
	 * is stripped. */
	size_t off = 0;
	if (!abs) {
		size_t parent_len = plen;
		while (parent_len > 0 && prefix[parent_len - 1] != '/')
			parent_len--;
		if (parent_len + tlen + 1 > sizeof m->target) {
			g_n_memo--;
			return;
		}
		for (size_t i = 0; i < parent_len; i++)
			m->target[i] = prefix[i];
		off = parent_len;
	}
	if (off + tlen + 1 > sizeof m->target) {
		g_n_memo--;
		return;
	}
	for (size_t i = 0; i < tlen; i++) m->target[off + i] = t[i];
	m->target[off + tlen] = 0;
	m->target_len = off + tlen;
}

void tawcroot_path_memoize_well_known(void)
{
	g_n_memo = 0;
	memo_one("lib");
	memo_one("lib64");
	memo_one("usr/lib64");
	memo_one("bin");
	memo_one("sbin");
	memo_one("usr/sbin");
	memo_one("var/run");
}

static int fd_is_proc_bind(int fd);

/* dst (no leading '/') lies strictly below `outer` (also no '/'). */
static int dst_below(const char *dst, const char *outer, size_t outer_len)
{
	if (!outer_len) return dst[0] != 0;
	return memcmp(dst, outer, outer_len) == 0 && dst[outer_len] == '/';
}

/* A mount at `dst` covers what was mounted at or below it, as the
 * kernel stacks mounts: those binds stop routing (and stop showing in
 * the synthesized mountinfo). */
/* Drop inactive binds (covered, unmounted, chrooted away) once the
 * table runs low: a sandbox mounts dozens of binds over the session's
 * own, and covered ones would otherwise hold their slots for good.
 * Order is kept (mountinfo parents, first-match ties); runs before a
 * mount operation reads any index. */
static int kern_root_fd;  /* defined with the pseudo root below */
static void compact_binds(void)
{
	if (tawcroot_n_binds + 32 < TAWCROOT_MAX_BINDS) return;
	size_t out = 0;
	for (size_t i = 0; i < tawcroot_n_binds; ++i) {
		struct tawcroot_bind *b = &tawcroot_binds[i];
		if (b->active) {
			if (out != i) tawcroot_binds[out] = *b;
			out++;
			continue;
		}
		int shared = b->src_fd == kern_root_fd ||
			     b->src_fd == tawcroot_rootfs_fd;
		for (size_t j = 0; j < tawcroot_n_binds && !shared; ++j)
			shared = j != i && tawcroot_binds[j].src_fd == b->src_fd &&
				 (tawcroot_binds[j].active || j < i);
		if (!shared && b->src_fd >= 0) {
			tawcroot_fd_forget_reserved(b->src_fd);
			tawc_close(b->src_fd);
		}
	}
	for (size_t i = out; i < tawcroot_n_binds; ++i) {
		tawcroot_binds[i].active = 0;
		tawcroot_binds[i].pinned = 0;
		tawcroot_binds[i].src_fd = -1;
	}
	tawcroot_n_binds = out;
}

static void cover_binds(const char *dst)
{
	size_t n = tawc_strlen(dst);
	for (size_t i = 0; i < tawcroot_n_binds; ++i) {
		struct tawcroot_bind *b = &tawcroot_binds[i];
		if (b->active && !b->pinned &&
		    (tawc_streq(b->dst, dst) || dst_below(b->dst, dst, n)))
			b->active = 0;
	}
}

/* Resolve a mount(2) target in the current view to its bind destination
 * ("/dst") in `destination`; *is_dir reports its type. */
static long mount_target(const char *target, char *name, char *suffix,
			 char *destination, int *is_dir)
{
	long e = tawc_copy_string_from_guest(name, 4096, target);
	if (e < 0) return e;
	int self_fd = tawcroot_proc_self_fd_num(name);
	tawcroot_path_result r = tawcroot_path_translate(name, suffix, 4096,
		TAWCROOT_PATH_FOLLOW, TAWCROOT_PATH_INTENT_READ);
	if (r.err) return r.err;
	long fd = tawc_openat(r.base_fd, *suffix ? suffix : ".",
		O_PATH | O_CLOEXEC, 0);
	if (fd < 0) return fd;
	struct stat st;
	e = TAWC_RAW(TAWC_SYS_fstat, fd, (long)&st, 0, 0, 0, 0);
	if (e < 0) { tawc_close((int)fd); return e; }
	*is_dir = S_ISDIR(st.st_mode) != 0;
	/* Through /proc (bubblewrap 0.12 mounts onto /proc/self/fd/N) the
	 * target is wherever the magic link leads in the view. */
	if (fd_is_proc_bind(r.base_fd))
		e = tawcroot_fd_to_guest_abs(self_fd >= 0 ? self_fd : (int)fd,
					     destination, 4096);
	else
		e = tawcroot_path_route_to_guest(r.base_fd, suffix, destination, 4096);
	tawc_close((int)fd);
	if (e < 0) return e;
	/* route_to_guest joins without folding; mount points compare folded
	 * (the fold drops the leading '/'). */
	e = tawcroot_path_fold_absolute(destination, name, 4096);
	if (e < 0) return e;
	size_t n = 0;
	if (tawc_str_append(destination, 4096, &n, "/") ||
	    tawc_str_append(destination, 4096, &n, name))
		return TAWC_ENAMETOOLONG;
	return (long)n;
}

/* mount --rbind: a recursive bind brings the source's submounts along.
 * Binds [0, upto) mounted below `src_guest` reappear below `dst`. */
static void replicate_below(const char *src_guest, const char *dst, size_t upto)
{
	size_t sl = tawc_strlen(src_guest);
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *child = scratch->buf[0];
	for (size_t i = 0; i < upto && i < tawcroot_n_binds; ++i) {
		struct tawcroot_bind *b = &tawcroot_binds[i];
		if (!b->active || !dst_below(b->dst, src_guest, sl)) continue;
		/* b->dst + sl starts with '/' unless the source is "/". */
		size_t n = 0;
		if (tawc_str_append(child, 4096, &n, dst) ||
		    (!sl && tawc_str_append(child, 4096, &n, "/")) ||
		    tawc_str_append(child, 4096, &n, b->dst + sl))
			continue;
		(void)tawcroot_path_add_bind(b->src, child, b->read_only);
	}
}

long tawcroot_path_bind_guest(const char *source, const char *target,
			      int recursive)
{
	compact_binds();
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *name = scratch->buf[0], *suffix = scratch->buf[1];
	char *host = scratch->buf[2], *destination = scratch->buf[3];
	long e = tawc_copy_string_from_guest(name, 4096, source);
	if (e < 0) return e;
	int self_fd = tawcroot_proc_self_fd_num(name);
	tawcroot_path_result r = tawcroot_path_translate(name, suffix, 4096,
		TAWCROOT_PATH_FOLLOW, TAWCROOT_PATH_INTENT_READ);
	if (r.err) return r.err;
	long fd = tawc_openat(r.base_fd, *suffix ? suffix : ".",
		O_PATH | O_CLOEXEC, 0);
	if (fd < 0) return fd;
	struct stat source_stat;
	e = TAWC_RAW(TAWC_SYS_fstat, fd, (long)&source_stat, 0, 0, 0, 0);
	/* SELinux denies apps getattr on some device nodes (/dev/full,
	 * which bwrap's --dev binds) that open O_PATH fine. Those are no
	 * directories; the kernel needs no stat to bind them either. */
	if (e == TAWC_EACCES) source_stat.st_mode = S_IFCHR;
	else if (e < 0) { tawc_close((int)fd); return e; }
	int read_only = r.ro;
	e = tawcroot_proc_fd_to_host_path((int)fd, host, 4096);
	/* The source's own place in the view, for an rbind's submounts
	 * (through a /proc magic link: where the link leads). */
	long ge = recursive ? (fd_is_proc_bind(r.base_fd)
		? tawcroot_fd_to_guest_abs(self_fd >= 0 ? self_fd : (int)fd,
					   destination, 4096)
		: tawcroot_path_route_to_guest(r.base_fd, suffix, destination, 4096)) : 0;
	tawc_close((int)fd);
	if (e < 0) return e;
	char src_guest[256];
	src_guest[0] = 0;
	if (recursive && ge > 0 &&
	    tawcroot_path_fold_absolute(destination, name, 4096) >= 0 &&
	    tawc_strlen(name) < sizeof src_guest)
		(void)tawc_str_copy(src_guest, sizeof src_guest, name);
	else recursive = 0;
	int is_dir;
	e = mount_target(target, name, suffix, destination, &is_dir);
	if (e < 0) return e;
	if (!!S_ISDIR(source_stat.st_mode) != is_dir) return TAWC_ENOTDIR;
	if (!destination[1]) return TAWC_EBUSY;  /* onto "/" */
	cover_binds(destination + 1);
	size_t before = tawcroot_n_binds;
	e = tawcroot_path_add_bind(host, destination, read_only);
	if (e < 0) return e;
	if (recursive) replicate_below(src_guest, destination + 1, before);
	return 0;
}

long tawcroot_path_bind_host(const char *src_host, const char *target,
			     int read_only)
{
	compact_binds();
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	int is_dir;
	long e = mount_target(target, scratch->buf[0], scratch->buf[1],
			      scratch->buf[2], &is_dir);
	if (e < 0) return e;
	if (!scratch->buf[2][1]) return TAWC_EBUSY;
	cover_binds(scratch->buf[2] + 1);
	return tawcroot_path_add_bind(src_host, scratch->buf[2], read_only);
}

long tawcroot_path_bind_set_ro(const char *target, int read_only)
{
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	int is_dir;
	long e = mount_target(target, scratch->buf[0], scratch->buf[1],
			      scratch->buf[2], &is_dir);
	if (e < 0) return 0;
	const char *dst = scratch->buf[2] + 1;
	if (!*dst) { tawcroot_root_ro = read_only; return 0; }
	for (size_t i = tawcroot_n_binds; i-- > 0;) {
		struct tawcroot_bind *b = &tawcroot_binds[i];
		if (b->active && !b->pinned && tawc_streq(b->dst, dst)) {
			b->read_only = read_only;
			break;
		}
	}
	return 0;
}

void tawcroot_path_pin_store_bind(void)
{
	if (!tawcroot_store_host_path_len) return;
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *link = scratch->buf[0];
	size_t n = 0;
	if (tawc_str_append(link, TAWCROOT_PATH_SCRATCH_SIZE, &n,
			    tawcroot_store_host_path) ||
	    tawc_str_append(link, TAWCROOT_PATH_SCRATCH_SIZE, &n, "/link"))
		return;
	for (size_t i = 0; i < tawcroot_n_binds; ++i) {
		struct tawcroot_bind *b = &tawcroot_binds[i];
		if (b->active && !b->leaf[0] && tawc_streq(b->src, link)) {
			b->pinned = 1;
			return;
		}
	}
	size_t i = tawcroot_n_binds;
	if (tawcroot_path_add_bind(link, ".tawcroot-link", 1) == 0 &&
	    tawcroot_n_binds > i)
		tawcroot_binds[i].pinned = 1;
}

/* The rootfs's PRoot hard-link directory, pinned at /.proot.l2s like the
 * store so rebased link2symlink targets (path_resolve.c) still resolve
 * after a sandbox pivots onto another root. A sandbox's own exec finds
 * the inherited pin; a rootfs PRoot never touched has none. */
void tawcroot_path_pin_l2s_bind(void)
{
	for (size_t i = 0; i < tawcroot_n_binds; ++i) {
		struct tawcroot_bind *b = &tawcroot_binds[i];
		if (b->active && !b->leaf[0] &&
		    tawc_streq(b->dst, ".proot.l2s")) {
			b->pinned = 1;
			return;
		}
	}
	if (!tawcroot_rootfs_host_path_len) return;
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *dir = scratch->buf[0];
	size_t n = 0;
	if (tawc_str_append(dir, TAWCROOT_PATH_SCRATCH_SIZE, &n,
			    tawcroot_rootfs_host_path) ||
	    tawc_str_append(dir, TAWCROOT_PATH_SCRATCH_SIZE, &n,
			    "/.proot.l2s"))
		return;
	struct stat st;
	if (TAWC_RAW(TAWC_SYS_fstatat, AT_FDCWD, (long)dir, (long)&st, 0, 0,
		     0) != 0 || !S_ISDIR(st.st_mode))
		return;
	size_t i = tawcroot_n_binds;
	if (tawcroot_path_add_bind(dir, ".proot.l2s", 0) == 0 &&
	    tawcroot_n_binds > i)
		tawcroot_binds[i].pinned = 1;
}

void tawcroot_path_unmount(const char *dst, int detach)
{
	size_t n = tawc_strlen(dst);
	for (size_t i = 0; i < tawcroot_n_binds; ++i) {
		struct tawcroot_bind *b = &tawcroot_binds[i];
		if (b->active && !b->pinned &&
		    (tawc_streq(b->dst, dst) ||
		     (detach && dst_below(b->dst, dst, n))))
			b->active = 0;
	}
}

/* bubblewrap's --ro-bind-data binds a temp file and unlinks it at once;
 * the kernel keeps the inode alive under the mount. Our file binds
 * re-open by name, so the name moves aside instead. Returns 1 when
 * (dirfd, name) was such a source and has been renamed. */
int tawcroot_path_bind_unlink_guard(int dirfd, const char *name)
{
	const char *base = name;
	for (const char *c = name; *c; c++) if (*c == '/') base = c + 1;
	int any = 0;
	for (size_t i = 0; i < tawcroot_n_binds && !any; ++i)
		any = tawcroot_binds[i].active && tawcroot_binds[i].leaf[0] &&
		      tawc_streq(tawcroot_binds[i].leaf, base);
	if (!any) return 0;
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *host = scratch->buf[0];
	long fd = tawc_openat(dirfd, name, O_PATH | O_NOFOLLOW | O_CLOEXEC, 0);
	if (fd < 0) return 0;
	long hn = tawcroot_proc_fd_to_host_path((int)fd, host, 4096);
	tawc_close((int)fd);
	if (hn <= 0) return 0;
	for (size_t i = 0; i < tawcroot_n_binds; ++i) {
		struct tawcroot_bind *b = &tawcroot_binds[i];
		char tok[TAWCROOT_LINK_TOKEN_MAX];
		if (!b->active || !b->leaf[0] || !tawc_streq(b->src, host) ||
		    tawcroot_link_host_path_token(host, tok, sizeof tok))
			continue;
		char aside[64];
		size_t n = 0;
		if (tawc_str_append(aside, sizeof aside, &n, ".tawcroot-bound-") ||
		    tawc_str_append_dec(aside, sizeof aside, &n, tawc_getpid()) ||
		    tawc_str_append(aside, sizeof aside, &n, "-") ||
		    tawc_str_append_dec(aside, sizeof aside, &n, (long)i))
			return 0;
		if (TAWC_RAW(TAWC_SYS_renameat, b->src_fd, (long)b->leaf,
			     b->src_fd, (long)aside, 0, 0) < 0)
			return 0;
		(void)tawc_str_copy(b->leaf, sizeof b->leaf, aside);
		size_t slash = b->src_len;
		while (slash && b->src[slash - 1] != '/') --slash;
		b->src[slash] = 0;
		b->src_len = slash;
		n = slash;
		if (tawc_str_append(b->src, sizeof b->src, &n, aside)) return 0;
		b->src_len = n;
		/* Every other bind of the same file follows it. */
		for (size_t j = 0; j < tawcroot_n_binds; ++j) {
			struct tawcroot_bind *o = &tawcroot_binds[j];
			if (j == i || !o->active || !o->leaf[0] || !tawc_streq(o->src, host)) continue;
			(void)tawc_str_copy(o->leaf, sizeof o->leaf, aside);
			(void)tawc_str_copy(o->src, sizeof o->src, b->src);
			o->src_len = b->src_len;
		}
		return 1;
	}
	return 0;
}

/* ---------- Pseudo-object fd-bind anchor ---------- */

/* The reserved fd of the KERNEL root '/' used as the base of every
 * pseudo-object file bind (openat(root, "proc/self/fd/N") re-resolves
 * /proc/self in the accessing task). Created lazily by
 * tawcroot_pseudo_root_ensure(); the fd is reserved (guest sees EBADF)
 * and deliberately NON-CLOEXEC so exec_state can hand the number
 * across the --exec-child re-entry, where a fresh image cannot
 * re-open it: the seccomp filter is already active there, and the
 * openat() would be trapped and translated against a view that does
 * not exist yet, folding the path into the guest rootfs. */
static int kern_root_fd = -1;

long tawcroot_pseudo_root_ensure(void)
{
	if (kern_root_fd < 0) {
		long rfd = tawc_openat(AT_FDCWD, "/proc/self/root",
				       O_PATH | O_DIRECTORY | O_CLOEXEC, 0);
		if (rfd < 0) return rfd;
		/* Reserved AND non-cloexec (exec-inheritable): the shm
		 * trick, with fdtab's reserve table for the EBADF rule. */
		long r = tawcroot_fd_dup_zone((int)rfd, 0);
		if (r < 0) { tawc_close((int)rfd); return r; }
		long rec = tawcroot_fd_record_reserved((int)r);
		if (rec < 0) { tawc_close((int)r); tawc_close((int)rfd); return rec; }
		tawc_close((int)rfd);
		kern_root_fd = (int)r;
	}
	return kern_root_fd;
}

int tawcroot_pseudo_root_get(void)
{
	return kern_root_fd;
}

void tawcroot_pseudo_root_adopt(int fd)
{
	if (fd < 0 || kern_root_fd >= 0) return;
	kern_root_fd = fd;
	/* Re-record in this image's fresh reserve table: the dup
	 * survived execve, the reserve list did not. */
	(void)tawcroot_fd_record_reserved(fd);
}

long tawcroot_path_add_bind(const char *src_host, const char *dst_guest,
                            int read_only)
{
	if (tawcroot_n_binds >= TAWCROOT_MAX_BINDS) return TAWC_ENOSPC;
	struct tawcroot_bind *b = &tawcroot_binds[tawcroot_n_binds];
	if (tawc_strlen(src_host) >= sizeof b->src) return TAWC_ENAMETOOLONG;

	/* Normalize dst through the lexical fold so it matches folded
	 * suffixes at lookup time — a dst with a trailing '/', '//' runs,
	 * or '.'/'..' components would otherwise be stored verbatim and
	 * never match (route_through_binds compares against folded
	 * suffixes). Empty after folding means root, which we don't allow
	 * as a bind target — that's just "the rootfs itself" and is
	 * handled by tawcroot_rootfs_fd. */
	{
		TAWCROOT_PATH_SCRATCH_AUTO(scratch);
		char *abs = scratch->buf[0];
		size_t off = 0;
		long e = tawc_str_append(abs, TAWCROOT_PATH_SCRATCH_SIZE,
					 &off, "/");
		if (!e) e = tawc_str_append(abs, TAWCROOT_PATH_SCRATCH_SIZE,
					    &off, dst_guest);
		if (e) return e;
		long fr = tawcroot_path_fold_absolute(abs, b->dst, sizeof b->dst);
		if (fr < 0) return fr;
	}
	size_t n = tawc_strlen(b->dst);
	if (n == 0) return TAWC_EINVAL;

	long fd = tawc_openat(AT_FDCWD, src_host,
			      O_PATH | O_DIRECTORY | O_CLOEXEC, 0);
	int is_file = fd == TAWC_ENOTDIR;
	if (is_file) fd = tawc_openat(AT_FDCWD, src_host, O_PATH | O_CLOEXEC, 0);
	if (fd < 0) return fd;
	b->leaf[0] = 0;
	long pn = tawcroot_proc_fd_to_host_path((int)fd, b->src, sizeof b->src);
	if (pn < 0) {
		if (!is_file) { tawc_close((int)fd); return pn; }
	} else {
		b->src_len = (size_t)pn;
	}
	if (is_file) {
		TAWCROOT_PATH_SCRATCH_AUTO(scratch);
		char *parent = scratch->buf[0];
		size_t slash;
		int pseudo = 0;
		if (pn < 0) {
			/* The strict label failed: either a /proc-less
			 * breakage, or a pseudo-object fd whose readlink
			 * label ("pipe:[7]", "socket:[9]", "anon_inode:x")
			 * is not a host path at all. Do the raw readlink to
			 * distinguish: any label at all means the fd points
			 * at a non-filesystem object and is still bindable
			 * (PRoot accepts exactly these specs for
			 * /dev/std{in,out,err} on every spawn, so the CLI
			 * must take them for parity). No label at all:
			 * keep the original failure (fail closed). */
			char link[64];
			char raw[64];
			long ln = -1;
			if (tawc_proc_fd_path(link, sizeof link,
					      (int)fd, 0) >= 0)
				ln = tawc_readlinkat(AT_FDCWD, link,
						     raw, sizeof raw - 1);
			if (ln < 0) { tawc_close((int)fd); return pn; }
			raw[ln] = 0;
			/* Detection done — now record the LITERAL source
			 * path in b->src, not the label. exec_state packs
			 * bind arrays straight off the bind table, so the
			 * next --exec-child re-add runs add_bind on this
			 * string, and "/proc/self/fd/N" is a path that
			 * resolves to the inheriting task's own fd N both
			 * raw (top level) and through b->src_fd (this
			 * pseudo route). Consistent across every exec. */
			(void)tawc_str_copy(b->src, sizeof b->src, src_host);
			b->src_len = (size_t)tawc_strlen(src_host);
			pseudo = 1;
		}
		if (pseudo) {
			/* Anchor the bind at the KERNEL root directory so
			 * the file-bind's per-access re-open runs
			 * openat(kernel-root, "proc/self/fd/N"): the kernel
			 * resolves /proc/self in the ACCESSING task, which
			 * is exactly the per-process fd-bind semantics
			 * PRoot gives. The label stays in b->src for
			 * reverse translation (proot's readlink of
			 * /dev/stdout shows the same "pipe:[N]" shape). */
			tawc_close((int)fd);
			long kr = tawcroot_pseudo_root_ensure();
			if (kr < 0) return kr;
			b->src_fd    = (int)kr;
			b->read_only = read_only ? 1 : 0;
			b->active    = 1;
			b->dst_len   = n;
			/* Leaf = the literal source path under the root:
			 * "/proc/self/fd/0" becomes "proc/self/fd/0". */
			(void)tawc_str_copy(b->leaf, sizeof b->leaf,
					    src_host + 1);
			tawcroot_n_binds++;
			return 0;
		}
		(void)tawc_str_copy(parent, TAWCROOT_PATH_SCRATCH_SIZE, b->src);
		slash = b->src_len;
		while (slash && parent[slash - 1] != '/') --slash;
		if (!slash) { tawc_close((int)fd); return TAWC_EINVAL; }
		(void)tawc_str_copy(b->leaf, sizeof b->leaf, parent + slash);
		parent[slash == 1 ? 1 : slash - 1] = 0;
		tawc_close((int)fd);
		fd = tawc_openat(AT_FDCWD, parent, O_PATH | O_DIRECTORY | O_CLOEXEC, 0);
		if (fd < 0) return fd;
	}

	long resv = tawcroot_fd_reserve((int)fd);
	if (resv < 0) {
		tawc_close((int)fd);
		return resv;
	}

	b->src_fd    = (int)resv;
	b->read_only = read_only ? 1 : 0;
	b->active    = 1;
	b->dst_len   = n;   /* b->dst already holds the folded form */

	/* Keep the full canonical source for reverse translation and exec;
	 * src_fd anchors either that directory or the file's parent. */
	tawcroot_n_binds++;
	return 0;
}

/* `tawcroot_path_fold_absolute` is implemented in `path_fold.c` (pure,
 * no syscalls — also linked into the cleat unit-test orchestrator).
 * The post-fold orchestration (bind → memo → resolver → bind) is in
 * `path_orchestrate.c`. */

/* Open a guest path read-only through the current root view. Falls
 * back to the host fs when no rootfs is configured (the legacy --exec /
 * --exec-via-handler diagnostics run before any rootfs exists).
 * Returns an O_RDONLY|O_CLOEXEC fd or -errno; a path that resolves to
 * the rootfs/bind directory itself is -EISDIR (exec/read of a dir).
 * Shared by the loader and the exec handler's pre-commit probe. */
long tawcroot_open_in_view(const char *guest_path)
{
	if (tawcroot_rootfs_fd < 0)
		return tawc_openat(AT_FDCWD, guest_path,
				   O_RDONLY | O_CLOEXEC, 0);
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *suffix = scratch->buf[0];
	tawcroot_path_result r = tawcroot_path_translate(
		guest_path, suffix, TAWCROOT_PATH_SCRATCH_SIZE,
		TAWCROOT_PATH_FOLLOW, TAWCROOT_PATH_INTENT_READ);
	if (r.err) return r.err;
	if (suffix[0] == 0) return TAWC_EISDIR;
	return tawc_openat(r.base_fd, suffix, O_RDONLY | O_CLOEXEC, 0);
}

/* ---------------------------------------------------------------- */
/* Production oracle + cwd source for the orchestration.             */

/* Production oracle: readlink against tawcroot_rootfs_fd via raw
 * syscall. The empty-suffix case (rootfs root itself) is handled
 * locally — readlinkat would return -EINVAL too, but skipping the
 * syscall is cheaper and clearer. */
static long prod_readlink(void *ctx, const char *suffix,
			  char *out, size_t out_cap)
{
	(void)ctx;
	if (!suffix || suffix[0] == 0) return TAWC_EINVAL;
	if (tawcroot_rootfs_fd < 0)    return TAWC_EBADF;
	const struct tawcroot_bind *best = 0;
	size_t len = tawc_strlen(suffix);
	for (size_t i = 0; i < tawcroot_n_binds; ++i) {
		const struct tawcroot_bind *b = &tawcroot_binds[i];
		/* A strict ancestor of a bind dst is a directory on the way to
		 * that bind, never a symlink (PRoot parity): the fake-DRM binds
		 * sit under /sys/dev/char/<maj>:<min>, a real sysfs symlink
		 * that would otherwise lead the walk away from them. */
		if (b->active && b->dst_len > len &&
		    b->dst[len] == '/' && tawc_starts_with(b->dst, suffix))
			return TAWC_EINVAL;
		if (!b->active || b->dst_len > len ||
		    !tawc_starts_with(suffix, b->dst) ||
		    (len > b->dst_len && suffix[b->dst_len] != '/')) continue;
		if (!best || b->dst_len > best->dst_len) best = b;
	}
	int fd = tawcroot_rootfs_fd;
	if (best) {
		/* proc magic links retain kernel semantics; path_translate below
		 * handles guest-root and visibility checks for those separately. */
		if (tawc_streq(best->src, "/proc")) return TAWC_EINVAL;
		fd = best->src_fd;
		suffix += best->dst_len;
		while (*suffix == '/') ++suffix;
		if (!*suffix) return TAWC_EINVAL;
	}
	long n = TAWC_RAW(TAWC_SYS_readlinkat, fd,
			  (long)suffix, (long)out, (long)out_cap, 0, 0);
	return n;
}

/* Hardlink emulation: readlink a link object by token (only consulted
 * on token hits — a hardlinked SYMLINK's target must continue the
 * guest-side walk instead of terminating in the store). A missing
 * store fd here means a token was met while LATENT: upgrade in place
 * (cold — token hits only) so the symlink-object splice, and the
 * orchestrator's rebase after us, both see the live store. */
static long prod_readlink_store(void *ctx, const char *token,
				char *out, size_t out_cap)
{
	(void)ctx;
	int fd = tawcroot_store_link_fd;
	if (fd < 0) fd = tawcroot_linkstore_latent_upgrade();
	if (fd < 0) return TAWC_ENOENT;
	return TAWC_RAW(TAWC_SYS_readlinkat, fd,
			(long)token, (long)out, (long)out_cap, 0, 0);
}

static const struct tawcroot_path_oracle prod_oracle = {
	.ctx            = 0,
	.readlink       = prod_readlink,
	.readlink_store = prod_readlink_store,
};

/* Canonical guest names stop at a hardlink's name, not its hidden
 * backing object. Hardlinked symlinks still follow their actual target. */
static long name_readlink(void *ctx, const char *suffix, char *out, size_t cap)
{
	long n = prod_readlink(ctx, suffix, out, cap);
	if (n <= 0 || (size_t)n >= cap) return n;
	out[n] = 0;
	const char *token;
	if (!tawcroot_link_target_is_token(out, &token)) return n;
	char saved[TAWCROOT_LINK_TOKEN_MAX];
	if (tawc_str_copy(saved, sizeof saved, token) < 0) return TAWC_ENAMETOOLONG;
	return prod_readlink_store(ctx, saved, out, cap);
}

static const struct tawcroot_path_oracle name_oracle = {
	.readlink = name_readlink,
	.readlink_store = prod_readlink_store,
};

/* Kernel-cwd-to-guest-absolute resolver: read the kernel cwd via raw
 * `getcwd` and reverse-translate through the shared longest-prefix
 * walk (rootfs AND bind srcs — a `cd` into a bind dst leaves the
 * kernel cwd at the bind src; matching only the rootfs prefix here
 * made getcwd and every relative path fail ENOENT after such a cd).
 * Returns the written length, or -ENOENT if the cwd is outside the
 * view (we deliberately don't leak host paths through guest errors).
 * Public — the getcwd handler reverse-translates through the same
 * function so the two can't drift. */
long tawcroot_cwd_to_guest_abs(char *out, size_t out_cap)
{
	if (tawcroot_rootfs_host_path_len == 0) return TAWC_ENOENT;

	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *cwd = scratch->buf[0];
	long r = TAWC_RAW(TAWC_SYS_getcwd, (long)cwd,
			  TAWCROOT_PATH_SCRATCH_SIZE,
			  0, 0, 0, 0);
	if (r < 0) return r;
	if ((size_t)r == 0) return TAWC_ENOENT;

	/* getcwd returns the length INCLUDING the NUL on aarch64+x86_64
	 * Linux. Older kernels may return without NUL — the manpage is
	 * fuzzy; we treat trailing NULs defensively. */
	size_t cwd_len = (size_t)r;
	while (cwd_len > 0 && cwd[cwd_len - 1] == 0) cwd_len--;

	return tawcroot_host_path_to_guest_abs(cwd, cwd_len, out, out_cap);
}

static long prod_cwd_to_guest_abs(void *ctx, char *out, size_t out_cap)
{
	(void)ctx;
	return tawcroot_cwd_to_guest_abs(out, out_cap);
}

/* ---------------------------------------------------------------- */
/* Read-only binds: impure companions to the orchestrator's central   */
/* check. Both run in tawcroot_path_translate below — the wrapper     */
/* every production translation passes through (handlers via          */
/* translate_local, sockets, chroot, exec/loader) — so a future call  */
/* site can't forget them. They stay out of the pure                  */
/* tawcroot_path_translate_with_ctx: each needs kernel ground truth   */
/* (a readlink / a stat) the ctx-driven unit tests don't model.       */

/* Fast gate: any RO surface in the current view at all? Skips the
 * per-call kernel probes for the (universal today) all-RW
 * configuration. Shared with the fd-based stage-2 checks in
 * syscalls_fs.c. */
int tawcroot_view_has_ro(void)
{
	if (tawcroot_root_ro) return 1;
	for (size_t i = 0; i < tawcroot_n_binds; i++)
		if (tawcroot_binds[i].active && tawcroot_binds[i].read_only)
			return 1;
	return 0;
}

/* 1 iff `fd` is the src fd of an active bind of host /proc — the one
 * place where a suffix can be a kernel magic link that re-opens an
 * inode with fresh access mode or resolves through to a host dir. */
static int fd_is_proc_bind(int fd)
{
	for (size_t i = 0; i < tawcroot_n_binds; i++) {
		const struct tawcroot_bind *b = &tawcroot_binds[i];
		if (!b->active || b->src_fd != fd) continue;
		return b->src_len == 5 && tawc_starts_with(b->src, "/proc");
	}
	return 0;
}

/* Stage 2, /proc magic links: refuse write-intent translations whose
 * final route is a /proc-bind magic link naming (or resolving through
 * to) an RO part of the view. A guest holding an RO-bind file open
 * O_RDONLY can ask for open("/proc/self/fd/<n>", O_RDWR) and the
 * KERNEL re-opens the inode writable (the host mount is RW, so it
 * won't refuse like a real RO mount); chmod through the same link, or
 * any write through self/cwd/<name> after a cd into an RO bind, walks
 * the same hole. This is the one check that cannot live in the
 * orchestrator — only the kernel knows the link target. Readlink
 * exactly the link prefix, join the resolve-through remainder, and
 * RO-prefix-check the final host path (so an RW bind nested under an
 * RO src still wins by longest prefix). */
static long ro_check_proc_magic_link(tawcroot_path_mode mode,
				     tawcroot_path_intent intent,
				     int base_fd, const char *suffix)
{
	if (intent == TAWCROOT_PATH_INTENT_READ &&
	    mode != TAWCROOT_PATH_PARENT_CREATE &&
	    mode != TAWCROOT_PATH_PARENT_REMOVE)
		return 0;
	if (!tawcroot_view_has_ro() || !fd_is_proc_bind(base_fd))
		return 0;
	size_t ml = tawcroot_proc_magic_link_prefix(suffix);
	char lnk[64];
	if (ml == 0 || ml >= sizeof lnk) return 0;
	for (size_t i = 0; i < ml; i++) lnk[i] = suffix[i];
	lnk[ml] = 0;
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *tgt = scratch->buf[0];
	long ln = tawc_readlinkat(base_fd, lnk, tgt,
				  TAWCROOT_PATH_SCRATCH_SIZE);
	if (ln <= 0 || ln >= (long)TAWCROOT_PATH_SCRATCH_SIZE) return 0;
	size_t pos = (size_t)ln;
	if (suffix[ml]) {
		if (tgt[pos - 1] == '/') pos--;  /* self/root is "/" */
		size_t joined = pos;
		if (tawc_str_append(tgt, TAWCROOT_PATH_SCRATCH_SIZE, &joined,
				    suffix + ml) == 0)
			pos = joined;
		/* Overlong join: judge the link target alone. */
	}
	return tawcroot_host_path_in_ro_bind(tgt, pos) ? TAWC_EROFS : 0;
}

/* Does a translation ACT ON what a magic link points at, rather than on
 * the link itself? True when the path resolves THROUGH the link
 * (self/root/etc/hostname) and, for a bare link, when the mode follows
 * the leaf. NOFOLLOW / PARENT_* on a bare link target the link object —
 * readlink and lstat stay kernel-faithful, and unlink/mkdir keep the
 * kernel's own refusals. */
static int magic_link_traversed(tawcroot_path_mode mode, const char *suffix,
				size_t ml)
{
	return suffix[ml] != 0 || mode == TAWCROOT_PATH_FOLLOW;
}

/* Stage 2, containment: a /proc magic link is resolved by the KERNEL,
 * which knows nothing about the guest's view. The bind lookup takes the
 * early return in tawcroot_path_translate_with_ctx ("Bind first … skip
 * memo and resolver"), so the suffix reaches the kernel with no symlink
 * resolution at all — and `/proc/self/root` is the real HOST root
 * (tawcroot never chroots). `cat /proc/1/root/etc/hostname` then reads
 * the host's, and a write through the same shape lands on the host fs.
 * No exploit, no resolver bug: an ordinary open through a supported
 * bind.
 *
 * So: readlink exactly the link prefix, join the resolve-through
 * remainder, and require the result to be somewhere the guest can name
 * (tawcroot_host_path_to_guest_abs succeeds). Out of view → -ENOENT,
 * which is what the guest should believe about a path its world doesn't
 * contain. A target that isn't an absolute path at all ("pipe:[…]",
 * "socket:[…]", "anon_inode:…") is nothing to contain — the kernel's own
 * open of it is correct — so it passes.
 *
 * Not applied to our own fd/map_files links, and own-root is rewritten
 * instead of refused; see TAWCROOT_PROC_MAGIC_* in proc_shadow.h.
 *
 * Cost: one readlink per magic-link-prefixed path under the /proc bind.
 * Those are rare outside self/fd/<n>, which is exempt, so the hot path
 * is untouched. `proc_bind` is the caller's already-computed
 * fd_is_proc_bind(base_fd). */
static long contain_proc_magic_link(tawcroot_path_mode mode, int proc_bind,
				    int base_fd, const char *suffix)
{
	if (!proc_bind) return 0;
	int kind;
	size_t ml = tawcroot_proc_magic_link_classify(suffix, &kind);
	if (ml == 0 || kind != TAWCROOT_PROC_MAGIC_CONTAIN) return 0;
	if (!magic_link_traversed(mode, suffix, ml)) return 0;

	char lnk[64];
	if (ml >= sizeof lnk) return 0;
	for (size_t i = 0; i < ml; i++) lnk[i] = suffix[i];
	lnk[ml] = 0;

	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *tgt = scratch->buf[0];
	long ln = tawc_readlinkat(base_fd, lnk, tgt, TAWCROOT_PATH_SCRATCH_SIZE);
	/* Unreadable link (gone, EACCES) or a non-path target: leave it to
	 * the kernel. */
	if (ln <= 0 || ln >= (long)TAWCROOT_PATH_SCRATCH_SIZE || tgt[0] != '/')
		return 0;
	/* A peer's memfd or unlinked file ("/memfd:x (deleted)") names no
	 * host path: opening the link itself reaches only that inode, as
	 * bubblewrap's --args/--file fds and bash's <(...) rely on. */
	static const char deleted[] = " (deleted)";
	const size_t dl = sizeof deleted - 1;
	if (!suffix[ml] && (size_t)ln > dl &&
	    memcmp(tgt + ln - dl, deleted, dl) == 0)
		return 0;
	size_t pos = (size_t)ln;
	if (suffix[ml]) {
		if (tgt[pos - 1] == '/') pos--;  /* a root link is "/" */
		size_t joined = pos;
		if (tawc_str_append(tgt, TAWCROOT_PATH_SCRATCH_SIZE, &joined,
				    suffix + ml) == 0)
			pos = joined;
		/* Overlong join: judge the link target alone. */
	}
	char *guest = scratch->buf[1];
	if (tawcroot_host_path_to_guest_abs(tgt, pos, guest,
					    TAWCROOT_PATH_SCRATCH_SIZE) < 0)
		return TAWC_ENOENT;
	return 0;
}

/* Central-EROFS errno fidelity: for FOLLOW/NOFOLLOW leaf-must-exist
 * writes (chmod/chown/utimensat/truncate/setxattr/open-for-write/
 * access(W_OK)) the kernel looks the target up BEFORE mnt_want_write,
 * so a missing target earns ENOENT/ENOTDIR, not EROFS. Probe with the
 * walk's own follow semantics. CREATE intent and the PARENT_* modes
 * keep EROFS — there the kernel wants write access before the leaf
 * lookup. Relies on the refusal being the orchestrator's LAST step,
 * with (base_fd, out_suffix) fully built. Cold by definition. */
static long ro_refusal_errno(int base_fd, const char *suffix,
			     tawcroot_path_mode mode,
			     tawcroot_path_intent intent)
{
	if (intent != TAWCROOT_PATH_INTENT_WRITE ||
	    (mode != TAWCROOT_PATH_FOLLOW && mode != TAWCROOT_PATH_NOFOLLOW))
		return TAWC_EROFS;
	struct stat probe;
	long pr = TAWC_RAW(TAWC_SYS_fstatat, base_fd,
			   (long)(suffix[0] ? suffix : "."), (long)&probe,
			   mode == TAWCROOT_PATH_NOFOLLOW
				   ? AT_SYMLINK_NOFOLLOW : 0,
			   0, 0);
	if (pr == TAWC_ENOENT || pr == TAWC_ENOTDIR) return pr;
	return TAWC_EROFS;
}

/* ---------------------------------------------------------------- */
/* Public API                                                        */

static tawcroot_path_result path_translate(const char *guest_path,
					     char *out_suffix, size_t out_cap,
					     tawcroot_path_mode mode,
					     tawcroot_path_intent intent, int preserve_name)
{
	tawcroot_path_result r;
	r.err     = 0;
	r.base_fd = -1;
	r.ro      = 0;

	if (!guest_path || out_cap == 0) {
		r.err = TAWC_EFAULT;
		return r;
	}
	if (tawcroot_rootfs_fd < 0) {
		r.err = TAWC_ENOENT;
		return r;
	}

	/* No lazy-reopen needed: handle_close fake-succeeds for reserved
	 * fds, so the guest cannot kill rootfs_fd or bind src_fds. The
	 * globals read below are written post-init only by the chroot(2)
	 * handler (chroot.c). A concurrent multi-threaded chroot races
	 * against the snapshot we take here — see notes/tawcroot/path-translation.md
	 * §"chroot emulation" for the bounded-failure analysis. */

	struct tawcroot_path_translate_ctx ctx = {
		.rootfs_base_fd   = tawcroot_rootfs_fd,
		.rootfs_ro        = tawcroot_root_ro,
		.binds            = tawcroot_binds,
		.n_binds          = tawcroot_n_binds,
		.memos            = g_memo,
		.n_memos          = g_n_memo,
		.store_link_fd    = tawcroot_store_link_fd,
		.store_upgrade    = tawcroot_linkstore_latent_upgrade,
		.oracle           = preserve_name ? &name_oracle : &prod_oracle,
		.cwd_to_guest_abs = prod_cwd_to_guest_abs,
		.cwd_ctx          = 0,
	};
	/* Own-root rewrite. `/proc/self/root/X` means `/X` to a chroot'd
	 * process; we never chroot, so the kernel would hand the guest the
	 * HOST root. Re-drive translation on the remainder against the
	 * current root view instead, which is both what a real chroot
	 * answers and inside the view by construction. Cold: the loop body
	 * only runs for a path that actually names our own root link, and
	 * each pass strictly shortens the path (nesting like
	 * /proc/self/root/proc/self/root/x unwinds one link per pass), so
	 * the hop bound is a backstop, not a policy. Exhausting it must
	 * NOT fall through to the kernel — that is the escape. */
	struct tawcroot_path_scratch *rw = 0;
	const char *path = guest_path;
	int proc_bind = 0;
	for (int hop = 0; ; hop++) {
		r = tawcroot_path_translate_with_ctx(
			&ctx, path, out_suffix, out_cap, mode, intent);
		if (r.err) break;
		proc_bind = fd_is_proc_bind(r.base_fd);
		if (!proc_bind) break;
		int kind;
		size_t ml = tawcroot_proc_magic_link_classify(out_suffix, &kind);
		int directory_fd = kind == TAWCROOT_PROC_MAGIC_FD_OWN && out_suffix[ml] == '/';
		if (ml == 0 || (kind != TAWCROOT_PROC_MAGIC_ROOT_OWN && !directory_fd)) break;
		if (!magic_link_traversed(mode, out_suffix, ml)) break;
		if (hop >= 8) { r.err = TAWC_ELOOP; break; }

		if (!rw) rw = tawcroot_path_scratch_acquire();
		char  *next = rw->buf[0];
		size_t pos  = 0;
		if (directory_fd) {
			/* A directory-fd traversal must see guest symlinks and link
			 * store tokens, just like openat(fd, suffix). Bare fd links
			 * retain kernel semantics (including unlinked open files). */
			char *prefix = rw->buf[1], *host = rw->buf[2];
			for (size_t i = 0; i < ml; ++i) prefix[i] = out_suffix[i];
			prefix[ml] = 0;
			long n = tawc_readlinkat(r.base_fd, prefix, host, TAWCROOT_PATH_SCRATCH_SIZE);
			if (n <= 0 || n >= TAWCROOT_PATH_SCRATCH_SIZE || host[0] != '/') break;
			long g = tawcroot_host_path_to_guest_abs(host, (size_t)n, next, TAWCROOT_PATH_SCRATCH_SIZE);
			if (g < 0) break;
			pos = (size_t)g;
		} else {
			next[pos++] = '/';
			next[pos] = 0;
		}
		if (tawc_str_append(next, TAWCROOT_PATH_SCRATCH_SIZE, &pos,
				    out_suffix + ml)) {
			r.err = TAWC_ENAMETOOLONG;
			break;
		}
		path = next;
	}
	tawcroot_path_scratch_release(rw);

	/* Impure companions (see block comment above). The orchestrator's
	 * EROFS is the only error that reaches here with (base_fd,
	 * out_suffix) valid — every earlier error returns before the
	 * central check populates them. Containment runs before the RO
	 * check: an out-of-view target's read-only-ness is not a question
	 * the guest gets to ask. */
	if (r.err == TAWC_EROFS)
		r.err = ro_refusal_errno(r.base_fd, out_suffix, mode, intent);
	else if (r.err == 0) {
		r.err = contain_proc_magic_link(mode, proc_bind, r.base_fd,
						out_suffix);
		if (r.err == 0)
			r.err = ro_check_proc_magic_link(mode, intent, r.base_fd,
							 out_suffix);
	}
	/* Hand the finished route to the rescue wrapper (rescue.h). This
	 * is THE recording point: every path-bearing handler, the AF_UNIX
	 * sockaddr translation and the exec open all funnel through here,
	 * so lazy DAC override needs no per-handler wiring. A no-op unless
	 * a wrapper-owned slot is live on this thread. */
	if (r.err == 0) tawcroot_rescue_note(r.base_fd, out_suffix);
	return r;
}

tawcroot_path_result tawcroot_path_translate(const char *path, char *out,
		size_t cap, tawcroot_path_mode mode, tawcroot_path_intent intent)
{
	return path_translate(path, out, cap, mode, intent, 0);
}

long tawcroot_path_guest_name(const char *path, char *out, size_t cap)
{
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *suffix = scratch->buf[0];
	tawcroot_path_result r = path_translate(path, suffix,
		TAWCROOT_PATH_SCRATCH_SIZE, TAWCROOT_PATH_FOLLOW,
		TAWCROOT_PATH_INTENT_READ, 1);
	if (r.err) return r.err;
	return tawcroot_path_route_to_guest(r.base_fd, suffix, out, cap);
}
