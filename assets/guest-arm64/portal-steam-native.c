/*
 * Steam computes the runtime chain before launching a game. Freshly
 * downloaded Linux games can still get the Valve FEX chain even though
 * Portal has a Box64 tool. Redirect that handoff inside Steam's reaper,
 * once the command and its actual game executable are known.
 *
 * Build with Debian ARM64 GCC:
 *   aarch64-linux-gnu-gcc -O2 -shared -fPIC -Wall -Wextra \
 *       -Wl,-z,max-page-size=16384,-soname,portal-steam-native.so \
 *       -o portal-steam-native.so portal-steam-native.c -ldl
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <elf.h>
#include <errno.h>
#include <fcntl.h>
#include <stdbool.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#ifndef BOX64_TOOL
#define BOX64_TOOL "/usr/share/steam/compatibilitytools.d/portal-box64/portal-box64"
#endif

static bool valve_fex(const char *file)
{
    const char suffix[] = "/steamapps/common/FEX-Emu/fex-compat-tool";
    size_t length = strlen(file);
    return length >= sizeof suffix - 1
        && strcmp(file + length - (sizeof suffix - 1), suffix) == 0;
}

static bool runtime_layer(const char *file)
{
    const char prefix[] = "/steamapps/common/SteamLinuxRuntime";
    const char *runtime = strstr(file, prefix);
    if (!runtime)
        return false;
    const char *entry = strchr(runtime + sizeof prefix - 1, '/');
    return entry && (strcmp(entry, "/_v2-entry-point") == 0
        || strcmp(entry, "/scout-on-soldier-entry-point-v2") == 0
        || strcmp(entry, "/run") == 0 || strncmp(entry, "/run-in-", 8) == 0);
}

static bool linux_game_file(const char *path, const char *install)
{
    char *directory = realpath(install, NULL);
    if (!directory)
        return false;
    char *game = realpath(path, NULL);
    size_t length = strlen(directory);
    bool inside = game && strncmp(game, directory, length) == 0 && game[length] == '/';
    free(directory);

    Elf64_Ehdr header;
    ssize_t count = 0;
    if (inside) {
        int fd = open(game, O_RDONLY | O_CLOEXEC | O_NONBLOCK);
        if (fd != -1) {
            struct stat st;
            if (fstat(fd, &st) == 0 && S_ISREG(st.st_mode))
                count = read(fd, &header, sizeof header);
            close(fd);
        }
    }
    free(game);

    // A Linux game's own launch script can also start the x86 binary.
    return (count >= 2 && header.e_ident[0] == '#' && header.e_ident[1] == '!')
        || (count == (ssize_t)sizeof header && memcmp(header.e_ident, ELFMAG, SELFMAG) == 0
            && header.e_ident[EI_CLASS] == ELFCLASS64 && header.e_ident[EI_DATA] == ELFDATA2LSB
            && header.e_machine == EM_X86_64);
}

static bool use_box64(const char *file, char *const argv[])
{
    if (!valve_fex(file) || !argv[0] || !argv[1]
        || (strcmp(argv[1], "waitforexitandrun") != 0 && strcmp(argv[1], "run") != 0)
        || !argv[2] || strcmp(argv[2], "--") != 0)
        return false;

    const char *install = getenv("STEAM_COMPAT_INSTALL_PATH");
    if (!install || !*install)
        return false;

    // Proton supplies its own command after the runtime wrappers. Verify
    // the executable belongs to the game before choosing Box64.
    char *const *command = argv + 3;
    while (*command && runtime_layer(*command)) {
        while (*command && strcmp(*command, "--") != 0)
            ++command;
        if (!*command)
            return false;
        ++command;
    }
    return *command && linux_game_file(*command, install);
}

int execvp(const char *file, char *const argv[])
{
    int (*next)(const char *, char *const[]) = dlsym(RTLD_NEXT, "execvp");
    if (!next) {
        errno = ENOSYS;
        return -1;
    }

    if (use_box64(file, argv))
        // The shell uses the new script as $0 and keeps Valve's verb and
        // runtime arguments for portal-box64 to interpret.
        next(BOX64_TOOL, argv);
    return next(file, argv);
}
