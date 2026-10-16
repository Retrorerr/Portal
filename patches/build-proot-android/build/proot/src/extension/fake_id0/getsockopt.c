#include <stdio.h>       /* snprintf, sscanf */
#include <unistd.h>      /* readlink */
#include <errno.h>       /* ERANGE */
#include <sys/socket.h>  /* SOL_SOCKET,SO_PEERCRED */

#include "tracee/reg.h"
#include "tracee/mem.h"
#include "extension/extension.h"
#include "extension/fake_id0/config.h"
#include "extension/fake_id0/getsockopt.h"

#ifndef SO_PEERGROUPS
#define SO_PEERGROUPS 59
#endif

/**
 * Get fake_id0 Config for given pid
 *
 * If pid isn't under fake_id0 returns NULL
 */
static Config *get_fake_id_for_pid(pid_t pid)
{
	Tracee *tracee = get_tracee(NULL, pid, false);
	if (tracee == NULL)
		return NULL;
	Extension *extension = get_extension(tracee, fake_id0_callback);
	if (extension == NULL)
		return NULL;
	return talloc_get_type_abort(extension->config, Config);
}

/**
 * Inode of the socket @fd of @tracee, or 0 if it is not a socket.
 */
static unsigned long socket_inode(const Tracee *tracee, int fd)
{
	char link[64];
	char target[64];
	unsigned long inode;
	ssize_t length;

	snprintf(link, sizeof(link), "/proc/%d/fd/%d", tracee->pid, fd);
	length = readlink(link, target, sizeof(target) - 1);
	if (length <= 0)
		return 0;
	target[length] = '\0';
	return sscanf(target, "socket:[%lu]", &inode) == 1 ? inode : 0;
}

/**
 * Note the room the caller gives SO_PEERGROUPS: the kernel replaces it
 * with the length of the real list before the exit stage.
 */
int handle_getsockopt_enter_end(Tracee *tracee, Config *config)
{
	socklen_t room;

	if (   peek_reg(tracee, ORIGINAL, SYSARG_2) != SOL_SOCKET
	    || peek_reg(tracee, ORIGINAL, SYSARG_3) != SO_PEERGROUPS)
		return 0;

	if (read_data(tracee, &room, peek_reg(tracee, ORIGINAL, SYSARG_5), sizeof(room)) < 0)
		room = 0;
	config->peergroups_room = room;
	return 0;
}

int handle_getsockopt_exit_end(Tracee *tracee, Config *config) {
	int fd = (int) peek_reg(tracee, ORIGINAL, SYSARG_1);
	word_t name = peek_reg(tracee, ORIGINAL, SYSARG_3);
	int result = (int) peek_reg(tracee, CURRENT, SYSARG_RESULT);

	if (peek_reg(tracee, ORIGINAL, SYSARG_2) != SOL_SOCKET)
		return 0;

	if (name == SO_PEERCRED && result == 0) {
		struct ucred cred;
		word_t cred_addr = peek_reg(tracee, ORIGINAL, SYSARG_4);
		int status = read_data(tracee, &cred, cred_addr, sizeof(struct ucred));
		if (status) return 0;

		/* SO_PEERGROUPS names no process: remember this one for
		 * it (D-Bus asks for both, credentials first).  */
		config->peercred_fd = fd;
		config->peercred_pid = cred.pid;
		config->peercred_inode = socket_inode(tracee, fd);

		Config *peer_config = get_fake_id_for_pid(cred.pid);
		if (peer_config == NULL) return 0;
		cred.uid = peer_config->euid;
		cred.gid = peer_config->egid;
		write_data(tracee, cred_addr, &cred, sizeof(struct ucred));
		return 0;
	}

	/* The kernel answers with the tracee's real supplementary groups,
	 * Android's (inet, sdcard_rw, ...): give the peer's fake ones, so
	 * polkit sees a guest user's "sudo" membership.  */
	if (name == SO_PEERGROUPS && (result == 0 || result == -ERANGE)) {
		word_t list = peek_reg(tracee, ORIGINAL, SYSARG_4);
		word_t length_addr = peek_reg(tracee, ORIGINAL, SYSARG_5);
		unsigned long inode = socket_inode(tracee, fd);
		Config *peer_config;
		socklen_t length;

		if (   inode == 0
		    || config->peercred_fd != fd
		    || config->peercred_inode != inode)
			return 0;

		peer_config = get_fake_id_for_pid(config->peercred_pid);
		if (peer_config == NULL)
			return 0;

		length = peer_config->ngroups * sizeof(gid_t);
		if (length > config->peergroups_room) {
			result = -ERANGE;
		}
		else {
			if (length > 0 && write_data(tracee, list, peer_config->groups, length) < 0)
				return 0;
			result = 0;
		}
		if (write_data(tracee, length_addr, &length, sizeof(length)) < 0)
			return 0;
		poke_reg(tracee, SYSARG_RESULT, (word_t) result);
	}
	return 0;
}
