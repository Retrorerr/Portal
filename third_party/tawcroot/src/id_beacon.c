/* Cross-process virtual identity beacon. See include/id_beacon.h.
 *
 * The beacon is an empty memfd: its name carries the ids and the kernel
 * shows it as the /proc/<pid>/fd link target "/memfd:tawc-id:... (deleted)".
 * It sits in the reserved fd zone (CLOEXEC, hidden from the guest's fd
 * listings, guarded against close/dup2), so every exec drops it and the
 * --exec-child restore publishes a fresh one. Forked children share the
 * parent's until they change identity themselves. */

#include <fcntl.h>
#include <stddef.h>
#include <stdint.h>

#include "fdtab.h"
#include "id_beacon.h"
#include "identity.h"
#include "namespace.h"
#include "path.h"
#include "io.h"
#include "raw_sys.h"

#define BEACON_PREFIX "tawc-id:"
#define BEACON_NAME_MAX 240
#define EXE_PREFIX    "tawc-exe:"
#define EXE_NAME_MAX  240  /* memfd names cap at 249 bytes */
#define MFD_CLOEXEC_FLAG 1u

static int      g_armed;
static uint32_t g_lock;
static int      g_fd = -1;
static int      g_exe_fd = -1;

static size_t put_u32(char *p, uint32_t v)
{
	char tmp[10];
	size_t n = 0;
	do { tmp[n++] = (char)('0' + v % 10); v /= 10; } while (v);
	for (size_t i = 0; i < n; i++) p[i] = tmp[n - 1 - i];
	return n;
}

/* Name: "tawc-id:<ruid>:<euid>:<rgid>:<egid>[:<g1>,<g2>,...]". The
 * supplementary groups answer SO_PEERGROUPS (dbus-daemon hands them to
 * polkit, whose admin rule checks the sudo group); a list too long for
 * a memfd name is cut short. */
static char g_name[BEACON_NAME_MAX + 1];

static void publish(void)
{
	tawc_identity id;
	tawcroot_identity_get(&id);
	uint32_t ids[4] = { id.ruid, id.euid, id.rgid, id.egid };
	if (tawcroot_namespace.user)
		for (int i = 0; i < 4; i++) ids[i] = tawcroot_namespace.outer_ids[i];

	char name[BEACON_NAME_MAX + 1];
	size_t n = 0;
	for (const char *s = BEACON_PREFIX; *s; s++) name[n++] = *s;
	for (int i = 0; i < 4; i++) {
		if (i) name[n++] = ':';
		n += put_u32(name + n, ids[i]);
	}
	uint32_t ng = id.ngroups < TAWC_IDENTITY_NGROUPS ? id.ngroups
						       : TAWC_IDENTITY_NGROUPS;
	for (uint32_t i = 0; i < ng && n + 12 <= BEACON_NAME_MAX; i++) {
		name[n++] = i ? ',' : ':';
		n += put_u32(name + n, id.groups[i]);
	}
	name[n] = 0;
	if (g_fd >= 0 && tawc_streq(name, g_name)) return;

	long fd = tawc_memfd_create(name, MFD_CLOEXEC_FLAG);
	if (fd < 0) return;
	long resv = tawcroot_fd_reserve((int)fd);
	if (resv < 0) {
		tawc_close((int)fd);
		return;
	}
	if (g_fd >= 0) {
		tawcroot_fd_forget_reserved(g_fd);
		tawc_close(g_fd);
	}
	g_fd = (int)resv;
	for (size_t i = 0; i <= n; i++) g_name[i] = name[i];
}

static void locked_publish(void)
{
	while (__atomic_exchange_n(&g_lock, 1, __ATOMIC_ACQUIRE)) {}
	publish();
	__atomic_store_n(&g_lock, 0, __ATOMIC_RELEASE);
}

/* Guest exe beacon: /proc/<pid>/exe of every guest names libtawcroot.so,
 * so peers (ps, task managers matching windows to launchers) read the
 * guest path from a memfd named "tawc-exe:<path>". Too-long paths
 * publish nothing and peers fall back to the kernel link. */
static void publish_exe(void)
{
	if (g_exe_fd >= 0) {
		tawcroot_fd_forget_reserved(g_exe_fd);
		tawc_close(g_exe_fd);
		g_exe_fd = -1;
	}
	size_t len = tawcroot_guest_exe_path_len;
	if (!len || len > EXE_NAME_MAX - sizeof EXE_PREFIX) return;
	char name[EXE_NAME_MAX + 1];
	size_t n = 0;
	for (const char *s = EXE_PREFIX; *s; s++) name[n++] = *s;
	for (size_t i = 0; i < len; i++) name[n++] = tawcroot_guest_exe_path[i];
	name[n] = 0;
	long fd = tawc_memfd_create(name, MFD_CLOEXEC_FLAG);
	if (fd < 0) return;
	long resv = tawcroot_fd_reserve((int)fd);
	if (resv < 0) {
		tawc_close((int)fd);
		return;
	}
	g_exe_fd = (int)resv;
}

void tawcroot_id_beacon_arm(void)
{
	__atomic_store_n(&g_armed, 1, __ATOMIC_RELEASE);
	while (__atomic_exchange_n(&g_lock, 1, __ATOMIC_ACQUIRE)) {}
	publish();
	publish_exe();
	__atomic_store_n(&g_lock, 0, __ATOMIC_RELEASE);
}

void tawcroot_id_beacon_update(void)
{
	if (__atomic_load_n(&g_armed, __ATOMIC_ACQUIRE))
		locked_publish();
}

static int parse_u32(const char **p, uint32_t *out)
{
	const char *s = *p;
	uint64_t v = 0;
	if (*s < '0' || *s > '9') return 0;
	while (*s >= '0' && *s <= '9') {
		v = v * 10 + (uint64_t)(*s - '0');
		if (v > 0xffffffffu) return 0;
		s++;
	}
	*out = (uint32_t)v;
	*p = s;
	return 1;
}

/* Find a reserved fd of `pid` whose link starts with "/memfd:<prefix>"
 * and copy the rest (minus " (deleted)") to out. Returns its length,
 * or 0 when absent/unreadable. */
static size_t scan_peer(int pid, const char *prefix, char *out, size_t cap)
{
	if (pid <= 0) return 0;
	char path[40] = "/proc/";
	size_t n = 6;
	n += put_u32(path + n, (uint32_t)pid);
	const char *tail = "/fd";
	while (*tail) path[n++] = *tail++;
	path[n] = 0;

	long dir = tawc_openat(AT_FDCWD, path, O_RDONLY | O_DIRECTORY | O_CLOEXEC, 0);
	if (dir < 0) return 0;
	size_t found = 0;
	char buf[320];  /* signal stack: keep the frame small */
	long got;
	while (!found && (got = tawc_getdents64((int)dir, buf, sizeof buf)) > 0) {
		for (long off = 0; off < got && !found;) {
			unsigned short reclen = *(unsigned short *)(void *)(buf + off + 16);
			const char *d_name = buf + off + 19;
			off += reclen;
			const char *q = d_name;
			uint32_t fdnum;
			if (!parse_u32(&q, &fdnum) || *q ||
			    fdnum < TAWCROOT_RESERVED_FD_BASE)
				continue;
			long len = tawc_readlinkat((int)dir, d_name, out, cap - 1);
			if (len <= 0) continue;
			out[len] = 0;
			const char *want = "/memfd:";
			const char *s = out;
			while (*want && *s == *want) { s++; want++; }
			for (want = prefix; *want && *s == *want; s++, want++) {}
			if (*want) continue;
			size_t rest = (size_t)(out + len - s);
			const char *del = " (deleted)";
			if (rest >= 10 && tawc_streq(s + rest - 10, del)) rest -= 10;
			for (size_t i = 0; i < rest; i++) out[i] = s[i];
			out[rest] = 0;
			found = rest;
		}
	}
	tawc_close((int)dir);
	return found;
}

int tawcroot_id_beacon_peer_groups(int pid, uint32_t ids[4],
				   uint32_t *groups, int max_groups)
{
	char link[BEACON_NAME_MAX + 32];
	if (!scan_peer(pid, BEACON_PREFIX, link, sizeof link)) return -1;
	const char *s = link;
	uint32_t v[4];
	for (int i = 0; i < 4; i++) {
		if (i && *s++ != ':') return -1;
		if (!parse_u32(&s, &v[i])) return -1;
	}
	if (ids)
		for (int i = 0; i < 4; i++) ids[i] = v[i];
	int ng = 0;
	if (*s == ':') {
		do {
			s++;
			uint32_t g;
			if (!parse_u32(&s, &g)) break;
			if (groups && ng < max_groups) groups[ng] = g;
			ng++;
		} while (*s == ',');
	}
	return ng;
}

int tawcroot_id_beacon_peer(int pid, uint32_t out[4])
{
	return tawcroot_id_beacon_peer_groups(pid, out, 0, 0) >= 0;
}

size_t tawcroot_exe_beacon_peer(int pid, char *out, size_t cap)
{
	if (cap < EXE_NAME_MAX + 32) return 0;
	return scan_peer(pid, EXE_PREFIX, out, cap);
}
