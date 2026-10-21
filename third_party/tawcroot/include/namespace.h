/* Startup compatibility only; Android UID/SELinux remain the security boundary. */
#pragma once
#include <stdint.h>
struct tawc_namespace {
    int32_t init_pid;
    uint32_t user;
    uint32_t capabilities[6];
    int32_t maps[3];
    uint32_t pid_pending;  /* unshare(CLONE_NEWPID): next child is init */
    uint32_t outer_ids[4]; /* ruid, euid, rgid, egid outside the user ns */
};
extern struct tawc_namespace tawcroot_namespace;
void tawcroot_namespace_register(void);
void tawcroot_namespace_restore(void);
long tawcroot_namespace_open(const char *path, int flags);
int tawcroot_namespace_probe(const char *path);
