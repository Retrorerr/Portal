#!/usr/bin/env python3
# Managed by Portal: maps installed x86 Linux games to the Portal Box64 tool.
#
# Steam's ARM64 client runs native Linux titles through Steam Linux Runtime
# with Valve's FEX tool beneath it, which cannot start under PRoot. Before
# the client starts, this adds a per-game compatibility mapping (the same
# entry "Force the use of a specific compatibility tool" writes) for every
# installed game whose own binary is x86-64 Linux. A mapping the user chose
# is never changed.
import os
import re
import sys

TOOL = "portal-box64"
SKIP_DIRS = ("Proton", "SteamLinuxRuntime", "FEX-Emu", "Steamworks Shared", "Steam Controller Configs")


def library_folders(root):
    yield root
    try:
        with open(os.path.join(root, "steamapps", "libraryfolders.vdf"), encoding="utf-8") as vdf:
            for match in re.finditer(r'^\s*"path"\s*"(.*)"\s*$', vdf.read(), re.M):
                path = match.group(1).replace("\\\\", "\\")
                if os.path.realpath(path) != os.path.realpath(root):
                    yield path
    except OSError:
        pass


def acf_value(text, key):
    match = re.search(r'^\s*"%s"\s*"(.*)"\s*$' % key, text, re.M)
    return match.group(1) if match else None


def elf_machine(path):
    try:
        with open(path, "rb") as handle:
            head = handle.read(20)
    except OSError:
        return None
    if len(head) < 20 or head[:4] != b"\x7fELF":
        return None
    return int.from_bytes(head[18:20], "little")


def is_x86_linux_game(directory):
    """An x86-64 ELF program at the top level or one directory down, and no
    Windows executable beside it."""
    candidates = []
    try:
        top = list(os.scandir(directory))
    except OSError:
        return False
    for entry in top:
        if entry.is_file() and entry.name.lower().endswith(".exe"):
            return False
    for entry in top:
        if entry.is_dir(follow_symlinks=False):
            try:
                candidates.extend(e for e in os.scandir(entry.path) if e.is_file())
            except OSError:
                pass
        elif entry.is_file():
            candidates.append(entry)
    for entry in candidates:
        name = entry.name
        if ".so" in name or name.endswith((".debug", ".dbg")):
            continue
        if elf_machine(entry.path) == 0x3E:
            return True
    return False


def installed_x86_games(root):
    for library in library_folders(root):
        steamapps = os.path.join(library, "steamapps")
        try:
            names = os.listdir(steamapps)
        except OSError:
            continue
        for name in names:
            if not (name.startswith("appmanifest_") and name.endswith(".acf")):
                continue
            try:
                with open(os.path.join(steamapps, name), encoding="utf-8", errors="replace") as acf:
                    text = acf.read()
            except OSError:
                continue
            appid, installdir = acf_value(text, "appid"), acf_value(text, "installdir")
            if not appid or not installdir or installdir.startswith(SKIP_DIRS):
                continue
            if is_x86_linux_game(os.path.join(steamapps, "common", installdir)):
                yield appid


def block_bounds(text, start):
    """Index just after the "{" opening the block that follows `start`, and
    the index of its matching "}"."""
    open_index = text.index("{", start)
    depth = 0
    for index in range(open_index, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return open_index + 1, index
    raise ValueError("unbalanced config.vdf")


def add_mappings(config_path, appids):
    with open(config_path, encoding="utf-8") as handle:
        text = handle.read()
    match = re.search(r'"CompatToolMapping"', text)
    if not match:
        steam = re.search(r'"Steam"\s*\{', text)
        if not steam:
            return 0
        insert_at = steam.end()
        text = text[:insert_at] + '\n\t\t\t\t"CompatToolMapping"\n\t\t\t\t{\n\t\t\t\t}' + text[insert_at:]
        match = re.search(r'"CompatToolMapping"', text)
    body_start, body_end = block_bounds(text, match.end())
    body = text[body_start:body_end]
    mapped = set(re.findall(r'^\s*"(\d+)"\s*$', body, re.M))
    new = [appid for appid in appids if appid not in mapped]
    if not new:
        return 0
    indent = "\t\t\t\t\t"
    entries = "".join(
        f'{indent}"{appid}"\n{indent}{{\n{indent}\t"name"\t\t"{TOOL}"\n'
        f'{indent}\t"config"\t\t""\n{indent}\t"priority"\t\t"250"\n{indent}}}\n'
        for appid in new
    )
    head = text[:body_end].rstrip("\t")
    text = head + entries + "\t\t\t\t" + text[body_end:]
    temporary = config_path + ".portal-tmp"
    with open(temporary, "w", encoding="utf-8") as handle:
        handle.write(text)
    os.replace(temporary, config_path)
    return len(new)


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/.local/share/Steam")
    config_path = os.path.join(root, "config", "config.vdf")
    if not os.path.isfile(config_path):
        return
    games = sorted(set(installed_x86_games(root)))
    if games:
        added = add_mappings(config_path, games)
        if added:
            print(f"portal-steam-compat: mapped {added} x86 Linux game(s) to {TOOL}", file=sys.stderr)


if __name__ == "__main__":
    try:
        main()
    except Exception as error:  # never block Steam from starting
        print(f"portal-steam-compat: {error}", file=sys.stderr)
