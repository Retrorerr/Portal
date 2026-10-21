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
    uint32_t net;          /* unshared a network namespace: rtnl changes are acked */
    /* user.max_user_namespaces as set inside the user ns: nested user
     * namespaces still allowed (bubblewrap --disable-userns writes 1). */
    uint32_t userns_limited;
    uint32_t userns_left;
    int32_t sysctl_fd1;    /* 1 + reserved dup of the written sysctl memfd */
};
extern struct tawc_namespace tawcroot_namespace;
void tawcroot_namespace_register(void);
void tawcroot_namespace_restore(void);
long tawcroot_namespace_open(const char *path, int flags);
int tawcroot_namespace_probe(const char *path);
