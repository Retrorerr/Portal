/* Cross-process virtual identity beacon — public API.
 *
 * Virtual identity lives in each process's memory, so a server asking
 * SO_PEERCRED about a client only learns the shared real app uid. PRoot
 * answers from its tracee table; tawcroot has none. Each guest process
 * instead holds a reserved memfd named "tawc-id:<ruid>:<euid>:<rgid>:
 * <egid>", which any same-uid peer reads from /proc/<pid>/fd. */

#pragma once

#include <stddef.h>
#include <stdint.h>

/* Arm the beacon and publish the current identity. Called once the
 * process's identity is final for this image (fresh start or the
 * --exec-child restore); before that, identity writes publish nothing. */
void tawcroot_id_beacon_arm(void);

/* Republish after a set*id change (no-op until armed or when the
 * published ids already match). */
void tawcroot_id_beacon_update(void);

/* Read process `pid`'s beacon. Returns 1 and fills ids with ruid, euid,
 * rgid, egid, or 0 when the process has none (not a tawcroot guest, or
 * unreadable). */
int tawcroot_id_beacon_peer(int pid, uint32_t ids[4]);

/* Same, plus the peer's supplementary groups: up to max_groups into
 * groups (either may be NULL/0). Returns the peer's group count, or -1
 * when it has no beacon. */
int tawcroot_id_beacon_peer_groups(int pid, uint32_t ids[4],
				   uint32_t *groups, int max_groups);

/* Read process `pid`'s guest executable path (published at arm time)
 * into out (cap >= 272). Returns its length, or 0 when unavailable. */
size_t tawcroot_exe_beacon_peer(int pid, char *out, size_t cap);

/* Root beacon: the host path of this process's root ("tawc-root:<path>"),
 * so a peer opening /proc/<pid>/root lands on our view's root (a
 * sandbox's tmpfs, not the app's "/"). Republished on chroot/pivot_root. */
void tawcroot_root_beacon_update(void);
size_t tawcroot_root_beacon_peer(int pid, char *out, size_t cap);
