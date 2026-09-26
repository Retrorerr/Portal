/*
 * Regression cases for PRoot's -H (hidden files) getdents filter, run under
 * `proot -H -r /` by run.sh.
 *
 *   getdents deleted   a directory removed while open: the kernel returns
 *                      ENOENT, which must reach the caller unchanged
 *   getdents notdir    getdents on a regular file: ENOTDIR, likewise
 *   list <dir> <size>  list <dir> with a <size>-byte buffer, one name per
 *                      line, "." and ".." omitted
 *
 * Before the fix, an error result was read as a ~4 GiB byte count and copied
 * onto the tracer's stack, and the buffer size sized two stack arrays, so
 * either case killed PRoot and every process it traced.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>

struct dirent64_record {
    unsigned long long d_ino;
    long long d_off;
    unsigned short d_reclen;
    unsigned char d_type;
    char d_name[];
};

static int getdents_error(const char *mode)
{
    char buffer[4096];
    int fd;

    if (strcmp(mode, "deleted") == 0) {
        char dir[] = "/tmp/proot-getdents-XXXXXX";
        if (mkdtemp(dir) == NULL) {
            perror("mkdtemp");
            return 2;
        }
        fd = open(dir, O_RDONLY | O_DIRECTORY);
        if (fd < 0 || rmdir(dir) != 0) {
            perror("open/rmdir");
            return 2;
        }
    } else if (strcmp(mode, "notdir") == 0) {
        fd = open("/proc/self/exe", O_RDONLY);
        if (fd < 0) {
            perror("open");
            return 2;
        }
    } else {
        fprintf(stderr, "unknown getdents mode %s\n", mode);
        return 2;
    }

    long result = syscall(SYS_getdents64, fd, buffer, sizeof(buffer));
    printf("%s: %s\n", mode, result < 0 ? strerror(errno) : "no error");
    return 0;
}

static int list(const char *path, size_t size)
{
    char *buffer = malloc(size);
    int fd = open(path, O_RDONLY | O_DIRECTORY);
    long result;

    if (buffer == NULL || fd < 0) {
        perror("list");
        return 2;
    }
    while ((result = syscall(SYS_getdents64, fd, buffer, size)) > 0) {
        for (long offset = 0; offset < result;) {
            struct dirent64_record *record = (struct dirent64_record *) (buffer + offset);
            if (strcmp(record->d_name, ".") != 0 && strcmp(record->d_name, "..") != 0)
                printf("%s\n", record->d_name);
            offset += record->d_reclen;
        }
    }
    if (result < 0) {
        perror("getdents64");
        return 1;
    }
    return 0;
}

int main(int argc, char **argv)
{
    if (argc == 3 && strcmp(argv[1], "getdents") == 0)
        return getdents_error(argv[2]);
    if (argc == 4 && strcmp(argv[1], "list") == 0)
        return list(argv[2], strtoul(argv[3], NULL, 0));
    fprintf(stderr, "usage: %s getdents deleted|notdir | list <dir> <buffer-size>\n", argv[0]);
    return 2;
}
