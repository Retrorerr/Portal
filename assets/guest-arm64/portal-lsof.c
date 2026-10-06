/*
 * portal-lsof: answers the TCP ownership queries Steam makes, without
 * /proc/net/tcp.
 *
 * The Steam client checks who is on the far side of its local websocket
 * connections (steamwebhelper, overlay) with
 *     lsof -P -F upnR -i TCP@127.0.0.1:<port>
 * Android denies untrusted apps /proc/net/{tcp,tcp6} and sock_diag, so the
 * real lsof finds nothing and Steam aborts with "Unexpected Transport Error"
 * (0x3008). This tool walks /proc/<pid>/fd, duplicates each socket with
 * pidfd_getfd(2) (same uid, allowed in the app sandbox) and reads its
 * addresses with getsockname/getpeername.
 *
 * Installed as /usr/sbin/lsof, the first path Steam probes. Any invocation
 * that is not an "-i TCP@host:port" query is handed to the real
 * /usr/bin/lsof unchanged.
 *
 * Build (guest, Debian arm64): gcc -O2 -s -o portal-lsof portal-lsof.c
 */
#define _GNU_SOURCE
#include <arpa/inet.h>
#include <ctype.h>
#include <dirent.h>
#include <errno.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#ifndef SYS_pidfd_open
#define SYS_pidfd_open 434
#endif
#ifndef SYS_pidfd_getfd
#define SYS_pidfd_getfd 438
#endif

#define REAL_LSOF "/usr/bin/lsof"

struct query {
	int family;              /* AF_INET or AF_INET6 */
	unsigned char addr[16];
	int port;
};

static int fields_has(const char *fields, char f)
{
	return fields == NULL || strchr(fields, f) != NULL;
}

static int parse_query(const char *spec, struct query *q)
{
	/* TCP@host:port, TCP@[v6]:port */
	if (strncasecmp(spec, "TCP@", 4) != 0)
		return -1;
	spec += 4;
	char host[64];
	const char *colon;
	if (*spec == '[') {
		const char *end = strchr(spec, ']');
		if (end == NULL || end[1] != ':' || (size_t)(end - spec - 1) >= sizeof host)
			return -1;
		memcpy(host, spec + 1, end - spec - 1);
		host[end - spec - 1] = '\0';
		colon = end + 1;
	} else {
		colon = strrchr(spec, ':');
		if (colon == NULL || (size_t)(colon - spec) >= sizeof host)
			return -1;
		memcpy(host, spec, colon - spec);
		host[colon - spec] = '\0';
	}
	char *end;
	long port = strtol(colon + 1, &end, 10);
	if (*end != '\0' || port <= 0 || port > 65535)
		return -1;
	q->port = (int)port;
	if (inet_pton(AF_INET, host, q->addr) == 1)
		q->family = AF_INET;
	else if (inet_pton(AF_INET6, host, q->addr) == 1)
		q->family = AF_INET6;
	else
		return -1;
	return 0;
}

static int addr_matches(const struct sockaddr_storage *ss, const struct query *q)
{
	if (ss->ss_family == AF_INET && q->family == AF_INET) {
		const struct sockaddr_in *in = (const void *)ss;
		return ntohs(in->sin_port) == q->port && memcmp(&in->sin_addr, q->addr, 4) == 0;
	}
	if (ss->ss_family == AF_INET6) {
		const struct sockaddr_in6 *in6 = (const void *)ss;
		if (ntohs(in6->sin6_port) != q->port)
			return 0;
		if (q->family == AF_INET6)
			return memcmp(&in6->sin6_addr, q->addr, 16) == 0;
		/* v4-mapped */
		static const unsigned char prefix[12] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff};
		return memcmp(&in6->sin6_addr, prefix, 12) == 0 &&
		       memcmp((const unsigned char *)&in6->sin6_addr + 12, q->addr, 4) == 0;
	}
	return 0;
}

static void format_addr(const struct sockaddr_storage *ss, char *out, size_t len)
{
	char host[INET6_ADDRSTRLEN] = "*";
	int port = 0;
	if (ss->ss_family == AF_INET) {
		const struct sockaddr_in *in = (const void *)ss;
		inet_ntop(AF_INET, &in->sin_addr, host, sizeof host);
		port = ntohs(in->sin_port);
		snprintf(out, len, "%s:%d", host, port);
	} else if (ss->ss_family == AF_INET6) {
		const struct sockaddr_in6 *in6 = (const void *)ss;
		inet_ntop(AF_INET6, &in6->sin6_addr, host, sizeof host);
		port = ntohs(in6->sin6_port);
		snprintf(out, len, "[%s]:%d", host, port);
	} else {
		snprintf(out, len, "*:*");
	}
}

static int read_status_field(int pid, const char *key, int column)
{
	char path[64], line[256];
	snprintf(path, sizeof path, "/proc/%d/status", pid);
	FILE *f = fopen(path, "r");
	if (f == NULL)
		return -1;
	int value = -1;
	size_t keylen = strlen(key);
	while (fgets(line, sizeof line, f) != NULL) {
		if (strncmp(line, key, keylen) != 0)
			continue;
		char *p = line + keylen;
		for (int i = 0; i <= column; i++) {
			while (*p && !isdigit((unsigned char)*p))
				p++;
			if (i == column)
				value = atoi(p);
			while (isdigit((unsigned char)*p))
				p++;
		}
		break;
	}
	fclose(f);
	return value;
}

/* /proc/<pid>/status shows the real Android uid; PRoot fakes getuid() for
 * the guest. Steam compares the u field with its own getuid(), so report
 * processes that share our real uid with our guest uid. */
static int guest_uid(int pid)
{
	int uid = read_status_field(pid, "Uid:", 0);
	if (uid >= 0 && uid == read_status_field(getpid(), "Uid:", 0))
		return (int)getuid();
	return uid;
}

static int query_tcp(const struct query *q, const char *fields)
{
	DIR *proc = opendir("/proc");
	if (proc == NULL)
		return 1;
	int found = 0;
	struct dirent *pe;
	while ((pe = readdir(proc)) != NULL) {
		if (!isdigit((unsigned char)pe->d_name[0]))
			continue;
		int pid = atoi(pe->d_name);
		char fddir[64];
		snprintf(fddir, sizeof fddir, "/proc/%d/fd", pid);
		DIR *fds = opendir(fddir);
		if (fds == NULL)
			continue;
		int pidfd = -1;
		int printed_process = 0;
		struct dirent *fe;
		while ((fe = readdir(fds)) != NULL) {
			if (!isdigit((unsigned char)fe->d_name[0]))
				continue;
			char link[320], target[64];
			snprintf(link, sizeof link, "%s/%s", fddir, fe->d_name);
			ssize_t n = readlink(link, target, sizeof target - 1);
			if (n <= 0)
				continue;
			target[n] = '\0';
			if (strncmp(target, "socket:", 7) != 0)
				continue;
			if (pidfd < 0) {
				pidfd = (int)syscall(SYS_pidfd_open, pid, 0);
				if (pidfd < 0)
					break;
			}
			int targetfd = atoi(fe->d_name);
			int fd = (int)syscall(SYS_pidfd_getfd, pidfd, targetfd, 0);
			if (fd < 0)
				continue;
			struct sockaddr_storage local = {0}, peer = {0};
			socklen_t llen = sizeof local, plen = sizeof peer;
			int type = 0;
			socklen_t tlen = sizeof type;
			int ok = getsockopt(fd, SOL_SOCKET, SO_TYPE, &type, &tlen) == 0 && type == SOCK_STREAM &&
				 getsockname(fd, (struct sockaddr *)&local, &llen) == 0 &&
				 (local.ss_family == AF_INET || local.ss_family == AF_INET6);
			int connected = ok && getpeername(fd, (struct sockaddr *)&peer, &plen) == 0;
			close(fd);
			if (!ok || !(addr_matches(&local, q) || (connected && addr_matches(&peer, q))))
				continue;
			if (!printed_process) {
				printf("p%d\n", pid);
				if (fields_has(fields, 'R'))
					printf("R%d\n", read_status_field(pid, "PPid:", 0));
				if (fields_has(fields, 'u'))
					printf("u%d\n", guest_uid(pid));
				printed_process = 1;
			}
			printf("f%d\n", targetfd);
			if (fields_has(fields, 'n')) {
				char a[80], b[80];
				format_addr(&local, a, sizeof a);
				if (connected) {
					format_addr(&peer, b, sizeof b);
					printf("n%s->%s\n", a, b);
				} else {
					printf("n%s\n", a);
				}
			}
			found = 1;
		}
		if (pidfd >= 0)
			close(pidfd);
		closedir(fds);
	}
	closedir(proc);
	fflush(stdout);
	return found ? 0 : 1;
}

int main(int argc, char **argv)
{
	const char *fields = NULL;
	const char *spec = NULL;
	int understood = 1;
	for (int i = 1; i < argc; i++) {
		const char *a = argv[i];
		if (strcmp(a, "-P") == 0 || strcmp(a, "-n") == 0 || strcmp(a, "-nP") == 0 ||
		    strcmp(a, "-Pn") == 0 || strcmp(a, "-w") == 0) {
			continue;
		} else if (strcmp(a, "-F") == 0 && i + 1 < argc) {
			fields = argv[++i];
		} else if (strncmp(a, "-F", 2) == 0 && a[2] != '\0') {
			fields = a + 2;
		} else if (strcmp(a, "-i") == 0 && i + 1 < argc) {
			spec = argv[++i];
		} else if (strncmp(a, "-i", 2) == 0 && a[2] != '\0') {
			spec = a + 2;
		} else {
			understood = 0;
		}
	}
	struct query q;
	if (understood && spec != NULL && fields != NULL && parse_query(spec, &q) == 0)
		return query_tcp(&q, fields);
	argv[0] = (char *)REAL_LSOF;
	execv(REAL_LSOF, argv);
	fprintf(stderr, "portal-lsof: cannot run %s: %s\n", REAL_LSOF, strerror(errno));
	return 1;
}
