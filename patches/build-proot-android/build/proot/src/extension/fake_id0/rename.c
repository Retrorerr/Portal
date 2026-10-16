#include <linux/limits.h>
#include <string.h>
#include <unistd.h>
#include <sys/stat.h>
#include <dirent.h>
#include <libgen.h>

#ifndef RENAME_NOREPLACE
#define RENAME_NOREPLACE (1 << 0)
#endif
#ifndef RENAME_EXCHANGE
#define RENAME_EXCHANGE (1 << 1)
#endif

#include "extension/fake_id0/rename.h"
#include "extension/fake_id0/helper_functions.h"

/** Whether the directory @path has entries besides "." and "..".  */
static bool dir_has_entries(const char *path)
{
	struct dirent *entry;
	bool found = false;
	DIR *dir;

	dir = opendir(path);
	if (dir == NULL)
		return false;
	while (!found && (entry = readdir(dir)) != NULL)
		found = strcmp(entry->d_name, ".") != 0 && strcmp(entry->d_name, "..") != 0;
	closedir(dir);
	return found;
}

/** Whether the kernel would fail rename(@oldpath, @newpath) with
 *  @flags, as far as the files' types and devices tell: the meta files
 *  change before the kernel renames, and must stay as they are then.  */
static bool rename_will_fail(const char *oldpath, const char *newpath, word_t flags)
{
	struct stat old_stat;
	struct stat new_stat;
	char parent[PATH_MAX];
	bool new_exists;

	if ((flags & RENAME_NOREPLACE) != 0 && (flags & RENAME_EXCHANGE) != 0)
		return true;
	if (lstat(oldpath, &old_stat) < 0)
		return true;
	new_exists = lstat(newpath, &new_stat) == 0;
	if ((flags & RENAME_NOREPLACE) != 0 && new_exists)
		return true;
	if ((flags & RENAME_EXCHANGE) != 0 && !new_exists)
		return true;

	if (!new_exists) {
		strcpy(parent, newpath);
		return stat(dirname(parent), &new_stat) == 0
			&& new_stat.st_dev != old_stat.st_dev;
	}
	if (new_stat.st_dev != old_stat.st_dev)
		return true;
	/* Hard links of one file: the kernel does nothing.  */
	if (new_stat.st_ino == old_stat.st_ino)
		return true;
	if ((flags & RENAME_EXCHANGE) != 0)
		return false;
	if (S_ISDIR(old_stat.st_mode) != S_ISDIR(new_stat.st_mode))
		return true;
	return S_ISDIR(new_stat.st_mode) && dir_has_entries(newpath);
}

/** Handles rename, renameat and renameat2 (@flags) syscalls. If a meta file
 *  matching the file to be renamed exists, renames the meta file as well;
 *  the meta file of a file it replaces goes with that file. See rename(2)
 *  for returned permission errors.
 */
int handle_rename_enter_end(Tracee *tracee, Reg oldfd_sysarg, Reg oldpath_sysarg, 
	Reg newfd_sysarg, Reg newpath_sysarg, word_t flags, Config *config)
{
	char new_meta_path[PATH_MAX];
	bool new_has_meta;
	int status;
	uid_t uid;
	gid_t gid;
	mode_t mode;
	char oldpath[PATH_MAX];
	char newpath[PATH_MAX];
	char rel_oldpath[PATH_MAX];
	char rel_newpath[PATH_MAX];
	char meta_path[PATH_MAX];

	status = read_sysarg_path(tracee, oldpath, oldpath_sysarg, CURRENT); 
	if(status < 0)
		return status;
	if(status == 1)
		return 0;

	status = read_sysarg_path(tracee, newpath, newpath_sysarg, CURRENT); 
	if(status < 0)
		return status;
	if(status == 1)
		return 0;

	status = get_fd_path(tracee, rel_oldpath, oldfd_sysarg, CURRENT);
	if(status < 0)
		return status;

	status = get_fd_path(tracee, rel_newpath, newfd_sysarg, CURRENT);
	if(status < 0)
		return status;

	status = check_dir_perms(tracee, 'w', oldpath, rel_oldpath, config);
	if(status < 0)
		return status;

	status = check_dir_perms(tracee, 'w', newpath, rel_newpath, config);
	if(status < 0)
		return status;

	/* The rename is about to fail: leave the meta files be.  */
	if (rename_will_fail(oldpath, newpath, flags))
		return 0;

	status = get_meta_path(oldpath, meta_path);
	if(status < 0)
		return status;

	status = get_meta_path(newpath, new_meta_path);
	if(status < 0)
		return status;
	new_has_meta = path_exists(new_meta_path) == 0;

	/* renameat2(RENAME_EXCHANGE): the files trade meta files.  */
	if ((flags & RENAME_EXCHANGE) != 0) {
		mode_t new_mode = 0;
		uid_t new_uid = 0;
		gid_t new_gid = 0;
		bool old_has_meta = path_exists(meta_path) == 0;

		if (old_has_meta)
			read_meta_file(meta_path, &mode, &uid, &gid, config);
		if (new_has_meta)
			read_meta_file(new_meta_path, &new_mode, &new_uid, &new_gid, config);
		unlink(meta_path);
		unlink(new_meta_path);
		if (new_has_meta)
			write_meta_file(meta_path, new_mode, new_uid, new_gid, 0, config);
		if (old_has_meta)
			return write_meta_file(new_meta_path, mode, uid, gid, 0, config);
		return 0;
	}

	/* No meta file of its own: the renamed file must not take on
	 * the one of the file it replaces (glnx commits ostree objects
	 * through renameat2, and a stale mode there fails their
	 * checksums).  */
	if(path_exists(meta_path) != 0) {
		if (new_has_meta)
			unlink(new_meta_path);
		return 0;
	}

	// If a meta file exists, "copy" it to the new path.
	read_meta_file(meta_path, &mode, &uid, &gid, config);
	unlink(meta_path);

	return write_meta_file(new_meta_path, mode, uid, gid, 0, config); 
}
