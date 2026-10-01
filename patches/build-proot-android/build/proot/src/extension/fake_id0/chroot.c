#include <errno.h>       /* E*, */
#include <sys/stat.h>    /* stat, */
#include <linux/capability.h> /* CAP_SYS_CHROOT, */

#include "tracee/reg.h"
#include "tracee/mem.h"
#include "path/path.h"
#include "path/binding.h"
#include "extension/fake_id0/chroot.h"

#include "syscall/fakens.h"

int handle_chroot_exit_end(Tracee *tracee, Config *config, bool from_sigsys) {
	char path[PATH_MAX];
	char path_guest[PATH_MAX];
	char path_host_absolute[PATH_MAX];
	word_t input;
	int status;
	word_t result;
	struct stat statbuf;
	bool seen_bind_under_new_root = false;

	/* Root, or CAP_SYS_CHROOT in an emulated user namespace.  */
	if (config->euid != 0 && !fakens_has_cap(tracee, CAP_SYS_CHROOT))
		return from_sigsys ? -EPERM : 0;

	if (from_sigsys) {
		/* Fetch reg early and from CURRENT RegVersion
		 * if this call is from SIGSYS handler.  */
		input = peek_reg(tracee, CURRENT, SYSARG_1);

		/* Set default error if we're here due to SIGSYS.  */
		poke_reg(tracee, SYSARG_RESULT, -EPERM);
	} else {
		/* Override only permission errors.
		 * (If we're here due to SIGSYS we don't check that
		 * we know syscall couldn't even be made.  */
		result = peek_reg(tracee, CURRENT, SYSARG_RESULT);
		if ((int) result != -EPERM)
			return 0;
	}

	/* Get chroot target path translated to host.  */
	if (!from_sigsys) {
		input = peek_reg(tracee, MODIFIED, SYSARG_1);

		status = read_path(tracee, path, input);
	} else {
		status = read_path(tracee, path_guest, input);
		if (status < 0)
			return status;
		status = translate_path(tracee, path, AT_FDCWD, path_guest, true);
	}

	if (status < 0)
		return status;

	realpath(path, path_host_absolute);

	/* Handle "new rootfs == current rootfs" case.  */
	status = compare_paths(get_root(tracee), path_host_absolute);
	if (status == PATHS_ARE_EQUAL) {
		/* Force success.  */
		if (from_sigsys) return 1;
		poke_reg(tracee, SYSARG_RESULT, 0);
		return 0;
	}

	/* Validate chroot target.  */
	status = stat(path_host_absolute, &statbuf);
	if (status < 0)
		return -errno;

	if (!S_ISDIR(statbuf.st_mode))
		return -ENOTDIR;

	/* Fetch guest path if we didn't already.  */
	if (!from_sigsys) {
		input = peek_reg(tracee, ORIGINAL, SYSARG_1);
		status = read_path(tracee, path_guest, input);
		if (status < 0)
			return -errno;
	}

	/* Check if chroot target has bind mounts inside.
	 * (Those are not supported currently).  */
	Binding *binding;
	for (binding = CIRCLEQ_FIRST(tracee->fs->bindings.guest);
	     binding != (void *) tracee->fs->bindings.guest;
	     binding = CIRCLEQ_NEXT(binding, link.guest)) {

		bool is_guest_root = binding == CIRCLEQ_LAST(tracee->fs->bindings.guest);

		if (!is_guest_root && compare_paths(path_guest, binding->guest.path) == PATH1_IS_PREFIX) {
			seen_bind_under_new_root = true;
			break;
		}
	}

	/* Change tracee root binding if supported.  */
	if (!seen_bind_under_new_root) {
		/* Save current dir.  */
		status = translate_path(tracee, path, AT_FDCWD, tracee->fs->cwd, true);
		if (status < 0)
			return status;

		/* Replace tracee bindings, for every process sharing
		 * them through CLONE_FS too.  */
		FileSystemNameSpace *old_fs = tracee->fs;
		Tracee *sharer;

		tracee->fs = talloc_zero(tracee, FileSystemNameSpace);
		binding = new_binding(tracee, path_host_absolute, "/", true);
		initialize_bindings(tracee);
		tracee->fs->chrooted = true;

		LIST_FOREACH(sharer, get_tracees_list_head(), link) {
			if (sharer == tracee || sharer->fs != old_fs)
				continue;
			sharer->fs = talloc_reference(sharer, tracee->fs);
			talloc_unlink(sharer, old_fs);
		}
		talloc_unlink(tracee, old_fs);

		/* Restore current dir.  */
		status = detranslate_path(tracee, path, NULL);
		if (status <= 0) {
			tracee->fs->cwd = talloc_strdup(tracee->fs, "/");
		} else {
			tracee->fs->cwd = talloc_strdup(tracee->fs, path);
		}

		/* Force success.  */
		if (from_sigsys) return 1;
		poke_reg(tracee, SYSARG_RESULT, 0);
		return 0;
	}

	/* Unsupported chroot() variant.  */
	return from_sigsys ? -ENOSYS : 0;
}
