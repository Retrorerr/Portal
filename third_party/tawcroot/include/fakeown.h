/* Fake-owner store + owner-view decoration — public API.
 *
 * PRoot forensic parity for the `--root-id` fake-root world of the
 * LocalDesktop PRoot fork (`libproot.so`): the two engines run against
 * the same rootfs, so a fake chown issued under either must be visible
 * to the other. Record layout and the caller-identity decoration rule
 * were probed on-device against a live rootfs; see fakeown.c's front
 * comment for the findings and the exact file format.
 */

#pragma once

#include <stddef.h>
#include <stdint.h>

/* Decoration "keep the kernel's mode" marker for fakeown's mode output
 * field. A record's mode rides a different layout (struct stat mode vs
 * statx mode vs legacy), so callers translate the override locally. */
#define FAKEOWN_MODE_KEEP 0xFFFFFFFFu

/* dirfd + dirfd-relative path of a translated target. `dirfd < 0` or a
 * NULL/empty `rel` disables record lookup (fd-only calls like fstat
 * pass NULL — the record exists only path-side). */
struct fakeown_path_ctx {
	int         dirfd;
	const char *rel;
};

/* The parsed record file: one bare octal line, then two decimal lines. */
struct fakeown_rec {
	unsigned int mode;   /* permission bits as %o ("644", "4755") */
	unsigned int uid;
	unsigned int gid;
};

/* The engine's own host uid — the "app-owned" marker on the rootfs
 * tree. Cached; the raw syscall runs once. */
uint32_t tawcroot_owner_host_uid(void);

/* Persist a fake chown/chmod of `dirfd:rel` as the record sibling.
 * Files only at the call sites (PRoot stores no dir records). */
long tawcroot_fakeown_record(int dirfd, const char *rel,
			     unsigned int mode, unsigned int uid,
			     unsigned int gid);

/* Read the record if present. Returns 1 + fills rec, or 0 (absent or
 * unreadable — "nothing recorded" is indistinguishable from absent). */
long tawcroot_fakeown_lookup(int dirfd, const char *rel,
			     struct fakeown_rec *rec);

/* Move/drop records across a guest rename (NOREPLACE / plain clobber /
 * EXCHANGE handled — EXCHANGE copies the two records across). */
long tawcroot_fakeown_move(int olddirfd, const char *oldrel,
			   int newdirfd, const char *newrel,
			   unsigned int rflags);

/* Drop the record after an unlink of the named path. */
long tawcroot_fakeown_drop(int dirfd, const char *rel);

/* rmdir of a directory whose only entries are records (the guest sees
 * it empty) hit ENOTEMPTY: delete those records so the retry can
 * succeed. Returns 1 when records were removed, 0 when the directory
 * holds anything else (left untouched), or -errno. */
long tawcroot_fakeown_clear_dir(int dirfd, const char *rel);

/* Is this dirent/readdir name one of the store's? Shared by the
 * readdir filter and the path translator's invisibility guard. */
int tawcroot_fakeown_name_is_meta(const char *name, size_t len);

/* Reject a translated result whose dirfd-relative leaf (`rel`, ""
 * allowed) is a record name — the store is invisible to the guest, in
 * every direction (lookup, create, rename-into, unlink). Returns
 * -ENOENT-shaped failure when the leaf is a record name. */
long tawcroot_fakeown_guard_leaf(const char *rel);

/* Decoration policy. Fills uid/gid outputs unconditionally (kernel
 * values when nothing applies); *mode_out is either FAKEOWN_MODE_KEEP
 * (kernel mode stays) or the record's bare permission bits. */
void tawcroot_fakeown_decorate(const struct fakeown_path_ctx *ctx,
			       unsigned int disk_uid, unsigned int disk_gid,
			       unsigned int disk_mode,
			       unsigned int *uid_out, unsigned int *gid_out,
			       unsigned int *mode_out);
