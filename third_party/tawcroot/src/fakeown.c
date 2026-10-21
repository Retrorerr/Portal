/* Fake-owner store + owner-view decoration — PRoot forensic parity.
 *
 * The LocalDesktop PRoot fork (`libproot.so`, `--root-id`) and this
 * engine run against the SAME rootfs on disk, and coverage requires
 * that a fake chown issued under either engine be visible to the
 * other. The kernel forbids an untrusted_app from chown(), so both
 * engines fake success; the PRoot fork records each FILE chown in a
 * sibling of the target,
 *
 *     <dir>/.proot-meta-file.<name>.meta
 *
 * holding three bare lines — the guest-visible mode as bare octal
 * ("644", "4755"), then uid, then gid — one per line, trailing
 * newline. Directory chowns are NOT recorded (spectra-verified: a
 * dir-rechown inside proot left no sibling); directory owners fall out
 * of the identity-view rule below, which is why
 * localdesktop-prepare-login's root chown of /home/desktop plus a
 * runuser-stat validation passes in production without any record.
 *
 * Decoration policy (proot fork, observed on-device):
 *   1. record present            → record's uid/gid (+mode), honored
 *                                  for EVERY calling identity (probe:
 *                                  root-stat saw 1000 on a file the
 *                                  desktop session had chown'ed)
 *   2. disk owner == engine host uid
 *                                → the caller's identity view:
 *                                  fake-root visitor sees 0:0; a
 *                                  dropped identity (runuser'd
 *                                  desktop) sees (euid, egid) — probe:
 *                                  /etc/passwd (app-owned, no record)
 *                                  read 0:0 under root and 1000:1000
 *                                  via `runuser -u desktop` — exactly
 *                                  the production prepare-login flow
 *   3. setgid/uid file under a dropped identity, or any other inode
 *                                  under a fake-root caller
 *                                → 0:0 (vanilla `proot -0` view)
 *   4. otherwise                 → kernel values untouched (dropped
 *                                  identity + foreign owner)
 *
 * Meta files are engine-internal and must be invisible to the guest
 * (proot hides them): the guest path resolver rejects the name shape
 * and readdir strips the entries. The engine itself touches records
 * through RAW host syscalls only, which never pass through the guest
 * path resolver.
 *
 * Composition: the hardlink emulation's stat fixups rewrite nlink
 * only; fakeown owns uid/gid/mode. chmod is NOT stored (the kernel
 * chmod on an app-owned inode really succeeds; a record's mode is
 * captured fresh at chown time, matching the probe). Records ride
 * dirfd-relative names, so writes/read/moves target the SAME handling
 * the kernel uses for the underlying file — cross-directory moves go
 * through the dirfd offsets, not an absolute path in our buffers.
 */

#include <stddef.h>
#include <stdint.h>
#include <sys/stat.h>

#include "errno_neg.h"
#include "fakeown.h"
#include "identity.h"
#include "io.h"
#include "path_scratch.h"
#include "raw_sys.h"
#include "sysnr.h"
#include "tawc_string.h"
#include "tawc_uapi.h"

#define FAKEOWN_META_PREFIX ".proot-meta-file."
#define FAKEOWN_META_SUFFIX ".meta"
#define FAKEOWN_META_LEAF_MAX 200

/* The engine's own host uid ("app-owned" marker on the rootfs tree).
 * Every inode inside the app's data dir carries this uid. Benign-race
 * init: two computing threads both store the identical value. */
static uint32_t g_host_uid;

uint32_t tawcroot_owner_host_uid(void)
{
	static int known;
	if (!known) {
		g_host_uid = (uint32_t)tawc_getuid();
		known = 1;
	}
	return g_host_uid;
}

/* Split a dirfd-relative rel into (dirname length, leaf pointer).
 * Returns the dirname length (0 = dirfd itself), or -ENOENT for the
 * root-denotation shapes ("", "x/") that name no leaf. */
static long fakeown_split(const char *rel, const char **leaf_out)
{
	const char *slash = NULL;
	for (const char *p = rel; *p; p++)
		if (*p == '/') slash = p;
	const char *leaf = slash ? slash + 1 : rel;
	if (*leaf == '\0') return TAWC_ENOENT;
	*leaf_out = leaf;
	return (long)(leaf - rel);
}

/* "<dirname>.proot-meta-file.<leaf>.meta", relative to dirfd. */
static long fakeown_meta_name(const char *rel, const char *leaf,
			      long dirlen, char *out, size_t cap)
{
	size_t pos      = 0;
	size_t leaf_len = tawc_strlen(leaf);
	if ((size_t)dirlen >= cap || leaf_len > FAKEOWN_META_LEAF_MAX)
		return TAWC_ENAMETOOLONG;
	if (dirlen > 0) {
		memcpy(out, rel, (size_t)dirlen);
		pos = (size_t)dirlen;
	}
	long rc = tawc_str_append(out, cap, &pos, FAKEOWN_META_PREFIX);
	if (rc) return rc;
	if (pos + leaf_len + sizeof(FAKEOWN_META_SUFFIX) > cap)
		return TAWC_ENAMETOOLONG;
	memcpy(out + pos, leaf, leaf_len);
	pos += leaf_len;
	rc = tawc_str_append(out, cap, &pos, FAKEOWN_META_SUFFIX);
	(void)rc;
	return 0;
}

/* Bare octal digits of m, NUL-free (MSB-first), returns length. */
static size_t fakeown_octal(char *out, unsigned int m)
{
	char    digits[12];
	size_t  n = 0;
	m &= 07777u;
	do {
		digits[n++] = (char)('0' + (m & 7u));
		m >>= 3;
	} while (m != 0);
	for (size_t i = 0; i < n / 2; i++) {
		char t = digits[i];
		digits[i] = digits[n - 1 - i];
		digits[n - 1 - i] = t;
	}
	for (size_t i = 0; i < n; i++) out[i] = digits[i];
	return n;
}

/* Scan one numeric field (base 8 or 10), skipping separators. */
static unsigned int fakeown_scan_field(const char **p, int base)
{
	unsigned long v   = 0;
	const char   *s   = *p;
	unsigned int  top = (base == 8) ? 7u : 9u;
	while (*s != '\0' && (*s < '0' || *s > (char)('0' + top))) s++;
	while (*s >= '0' && *s <= (char)('0' + top)) {
		v = v * (unsigned long)base + (unsigned long)(*s - '0');
		if (v > 0xFFFFFFFFul) v = 0xFFFFFFFFul;
		s++;
	}
	*p = s;
	return (unsigned int)v;
}

long tawcroot_fakeown_record(int dirfd, const char *rel,
			     unsigned int mode, unsigned int uid,
			     unsigned int gid)
{
	if (!rel) return 0;
	const char *leaf = NULL;
	long dirlen = fakeown_split(rel, &leaf);
	if (dirlen < 0) return dirlen;
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *meta = scratch->buf[0];
	long rc = fakeown_meta_name(rel, leaf, dirlen, meta,
				    TAWCROOT_PATH_SCRATCH_SIZE);
	if (rc) return rc;

	char payload[48];
	size_t n = 0;
	/* mode line: bare octal — the shape "%o" produced in the
	 * probe; uid/gid follow as bare decimals, one per line. */
	n += fakeown_octal(payload + n, mode);
	payload[n++] = '\n';
	n += (size_t)tawc_long_to_str(payload + n, sizeof payload - n,
				      (long)uid);
	payload[n++] = '\n';
	n += (size_t)tawc_long_to_str(payload + n, sizeof payload - n,
				      (long)gid);
	payload[n++] = '\n';

	int fd = tawc_openat(dirfd, meta,
			     O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0600);
	if (fd < 0) return fd;
	size_t off = 0;
	while (off < n) {
		long w = TAWC_RAW(TAWC_SYS_write, fd, (long)(payload + off),
				  (long)(n - off), 0, 0, 0);
		if (w <= 0) {
			tawc_close(fd);
			return w ? w : TAWC_EIO;
		}
		off += (size_t)w;
	}
	tawc_close(fd);
	return 0;
}

long tawcroot_fakeown_lookup(int dirfd, const char *rel,
			     struct fakeown_rec *rec)
{
	if (!rel) return 0;
	const char *leaf = NULL;
	long dirlen = fakeown_split(rel, &leaf);
	if (dirlen < 0) return 0;
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *meta = scratch->buf[0];
	if (fakeown_meta_name(rel, leaf, dirlen, meta,
			      TAWCROOT_PATH_SCRATCH_SIZE))
		return 0;

	int fd = tawc_openat(dirfd, meta, O_RDONLY | O_CLOEXEC, 0);
	if (fd < 0) return 0;
	char buf[96];
	size_t got = 0;
	while (got < sizeof buf - 1) {
		long r = tawc_read(fd, buf + got, sizeof buf - 1 - got);
		if (r <= 0) break;
		got += (size_t)r;
	}
	tawc_close(fd);
	buf[got] = '\0';
	if (got == 0) return 0;

	/* Three numeric fields in order; separators tolerated (the
	 * producer writes one per line with a trailing newline). */
	const char *p = buf;
	rec->mode = fakeown_scan_field(&p, 8);
	rec->uid  = fakeown_scan_field(&p, 10);
	rec->gid  = fakeown_scan_field(&p, 10);
	return 1;
}

long tawcroot_fakeown_drop(int dirfd, const char *rel)
{
	if (!rel) return 0;
	const char *leaf = NULL;
	long dirlen = fakeown_split(rel, &leaf);
	if (dirlen < 0) return 0;
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *meta = scratch->buf[0];
	if (fakeown_meta_name(rel, leaf, dirlen, meta,
			      TAWCROOT_PATH_SCRATCH_SIZE))
		return 0;
	(void)TAWC_RAW(TAWC_SYS_unlinkat, dirfd, (long)meta, 0, 0, 0, 0);
	return 0;
}

long tawcroot_fakeown_move(int olddirfd, const char *oldrel,
			   int newdirfd, const char *newrel,
			   unsigned int rflags)
{
	if (!oldrel || !newrel) return 0;
	struct fakeown_rec ro, rn;
	int ho = tawcroot_fakeown_lookup(olddirfd, oldrel, &ro);
	int hn = tawcroot_fakeown_lookup(newdirfd, newrel, &rn);
	if (!ho && !hn) return 0;

	if (rflags & RENAME_EXCHANGE) {
		/* Names swap, records follow the files: old's record
		 * becomes new's record and vice versa. Copy across (the
		 * record is authoritative state, not a lock target — an
		 * in-place tmp-rename dance adds nothing). */
		if (hn)
			(void)tawcroot_fakeown_record(olddirfd, oldrel,
						      rn.mode, rn.uid, rn.gid);
		else
			(void)tawcroot_fakeown_drop(olddirfd, oldrel);
		if (ho)
			(void)tawcroot_fakeown_record(newdirfd, newrel,
						      ro.mode, ro.uid, ro.gid);
		else
			(void)tawcroot_fakeown_drop(newdirfd, newrel);
		return 0;
	}

	/* Plain move, clobbering edition included: dst's record is
	 * (re)placed with src's, or dropped when src carried none (the
	 * clobbered name loses its record with the file). NOREPLACE can
	 * only arrive on a success path, where dst didn't exist. */
	if (ho)
		(void)tawcroot_fakeown_record(newdirfd, newrel, ro.mode,
					      ro.uid, ro.gid);
	else
		(void)tawcroot_fakeown_drop(newdirfd, newrel);
	(void)tawcroot_fakeown_drop(olddirfd, oldrel);
	return 0;
}

int tawcroot_fakeown_name_is_meta(const char *name, size_t len)
{
	const size_t plen = sizeof(FAKEOWN_META_PREFIX) - 1;
	const size_t slen = sizeof(FAKEOWN_META_SUFFIX) - 1;
	if (len < plen + slen + 1) return 0;  /* needs middle byte(s) */
	if (memcmp(name, FAKEOWN_META_PREFIX, plen)) return 0;
	if (memcmp(name + len - slen, FAKEOWN_META_SUFFIX, slen)) return 0;
	return 1;
}

/* Two passes over the directory: the first proves every entry is a
 * record (a live sibling's record must never be lost), the second
 * unlinks them. */
long tawcroot_fakeown_clear_dir(int dirfd, const char *rel)
{
	long dfd = tawc_openat(dirfd, rel, O_RDONLY | O_DIRECTORY | O_CLOEXEC |
				   O_NOFOLLOW, 0);
	if (dfd < 0) return dfd;
	TAWCROOT_PATH_SCRATCH_AUTO(scratch);
	char *buf = scratch->buf[0];
	long result = 0;
	for (int pass = 0; pass < 2; pass++) {
		if (TAWC_RAW(TAWC_SYS_lseek, dfd, 0, 0 /*SEEK_SET*/,
			     0, 0, 0) != 0) {
			result = TAWC_EINVAL;
			break;
		}
		int found = 0, foreign = 0;
		for (;;) {
			long n = TAWC_RAW(TAWC_SYS_getdents64, dfd, (long)buf,
					  TAWCROOT_PATH_SCRATCH_SIZE, 0, 0, 0);
			if (n < 0) { result = n; foreign = 1; }
			if (n <= 0) break;
			long off = 0;
			/* linux_dirent64: d_reclen u16 at +16, d_name at +19. */
			while (off + 19 < n) {
				unsigned short reclen;
				memcpy(&reclen, buf + off + 16, sizeof reclen);
				if (reclen < 20 || off + reclen > n) break;
				const char *nm = buf + off + 19;
				off += reclen;
				if (nm[0] == '.' && (nm[1] == '\0' ||
				    (nm[1] == '.' && nm[2] == '\0')))
					continue;
				if (!tawcroot_fakeown_name_is_meta(nm, strlen(nm))) {
					foreign = 1;
					continue;
				}
				found = 1;
				if (pass == 1)
					(void)TAWC_RAW(TAWC_SYS_unlinkat, dfd, (long)nm,
						       0, 0, 0, 0);
			}
		}
		if (foreign || !found) break;
		if (pass == 1) result = 1;
	}
	tawc_close((int)dfd);
	return result;
}

long tawcroot_fakeown_guard_leaf(const char *rel)
{
	if (!rel || rel[0] == '\0') return 0;
	const char *slash = NULL;
	for (const char *p = rel; *p; p++)
		if (*p == '/') slash = p;
	const char *leaf = slash ? slash + 1 : rel;
	if (tawcroot_fakeown_name_is_meta(leaf, tawc_strlen(leaf)))
		return TAWC_ENOENT;
	return 0;
}

void tawcroot_fakeown_decorate(const struct fakeown_path_ctx *ctx,
			       unsigned int disk_uid, unsigned int disk_gid,
			       unsigned int disk_mode,
			       unsigned int *uid_out, unsigned int *gid_out,
			       unsigned int *mode_out)
{
	*mode_out = FAKEOWN_MODE_KEEP;
	struct fakeown_rec rec;
	if (ctx && ctx->dirfd >= 0 && ctx->rel && ctx->rel[0] != '\0' &&
	    tawcroot_fakeown_lookup(ctx->dirfd, ctx->rel, &rec) == 1) {
		*uid_out  = rec.uid;
		*gid_out  = rec.gid;
		*mode_out = rec.mode;  /* record also pins guest mode */
		return;
	}

	tawc_identity id;
	tawcroot_identity_get(&id);
	if (disk_mode & 06000) {
		/* Setuid/setgid safety rule for dropped callers
		 * (pre-existing behaviour, preserved verbatim). */
		*uid_out = 0;
		*gid_out = 0;
		return;
	}
	if (disk_uid == tawcroot_owner_host_uid()) {
		/* App-owned: the visitor's identity view. Fake root sees
		 * root-owned; a dropped identity (runuser'd desktop)
		 * sees its own ids — an app-owned tree reads as the
		 * visitor's, matching the prepare-login contract. */
		if (id.euid == 0) {
			*uid_out = 0;
			*gid_out = 0;
		} else {
			*uid_out = id.euid;
			*gid_out = id.egid;
		}
		return;
	}
	if (id.euid == 0) {
		/* Vanilla `proot -0` view for a root caller over a
		 * foreign-owned inode (bind-hosted /dev et al). */
		*uid_out = 0;
		*gid_out = 0;
		return;
	}
	/* Dropped identity + foreign owner + no record: kernel truth. */
	*uid_out = disk_uid;
	*gid_out = disk_gid;
}
