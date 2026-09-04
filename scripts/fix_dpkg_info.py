#!/usr/bin/env python3
import os
import sys

def main():
    info_dir = "/var/lib/dpkg/info"
    linked = 0
    errors = 0
    for f in os.listdir(info_dir):
        if ":arm64." in f:
            target = f.replace(":arm64.", ".")
            src_path = os.path.join(info_dir, f)
            tgt_path = os.path.join(info_dir, target)
            if not os.path.lexists(tgt_path):
                try:
                    os.symlink(f, tgt_path)
                    linked += 1
                except Exception as e:
                    print(f"Error linking {f} -> {target}: {e}", file=sys.stderr)
                    errors += 1
    print(f"Created {linked} symlinks in {info_dir} (errors: {errors})")

if __name__ == "__main__":
    main()
