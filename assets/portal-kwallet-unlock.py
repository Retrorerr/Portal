#!/usr/bin/env python3
"""Open the default KWallet at login, as kwallet-pam does on a native desktop.

Chromium, Electron (Claude, ChatGPT, VS Code) and KDE apps keep secrets in
KWallet. Without a wallet the first such app stops on KWallet's setup
wizard, and with a password-protected one every session asks for it. Portal
has no login password for kwallet-pam to reuse, so this helper does what
pam_kwallet5 does with it: it hands kwalletd the PBKDF2-SHA512 key of the
(empty) login password through pamOpen, which creates "kdewallet" on first
use and opens it silently afterwards. The wallet stays a normal encrypted
wallet; KWalletManager opens it with an empty password, and a user who sets
a real password there simply gets KWallet's usual prompt from then on.
"""

import hashlib
import os
import subprocess
import sys
from pathlib import Path

WALLET = "kdewallet"
# kwallet's PBKDF2_SHA512_{SALTSIZE,KEYSIZE,ITERATIONS}, shared with kwallet-pam.
SALT_SIZE = 56
KEY_SIZE = 56
ITERATIONS = 50000
WALLET_DIR = Path(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share")) / "kwalletd"


def wallet_salt():
    wallet = WALLET_DIR / f"{WALLET}.kwl"
    salt = WALLET_DIR / f"{WALLET}.salt"
    if salt.is_file() and salt.stat().st_size == SALT_SIZE:
        return salt.read_bytes()
    if wallet.exists():
        # A wallet without a PBKDF2 salt predates it or was made elsewhere;
        # leave it to KWallet's own prompt.
        return None
    WALLET_DIR.mkdir(parents=True, exist_ok=True)
    os.chmod(WALLET_DIR, 0o700)
    data = os.urandom(SALT_SIZE)
    fd = os.open(salt, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "wb") as out:
        out.write(data)
    return data


def first_use_defaults():
    # Skip the first-run wizard only while no wallet exists; never override
    # choices made later in System Settings.
    if (WALLET_DIR / f"{WALLET}.kwl").exists():
        return
    config = Path(os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")) / "kwalletrc"
    text = config.read_text() if config.exists() else ""
    if "First Use" in text:
        return
    config.parent.mkdir(parents=True, exist_ok=True)
    with open(config, "a") as out:
        out.write(
            ("\n" if text and not text.endswith("\n") else "")
            + "[Wallet]\nDefault Wallet=kdewallet\nEnabled=true\nFirst Use=false\n"
        )


def main():
    first_use_defaults()
    salt = wallet_salt()
    if salt is None:
        return 0
    key = hashlib.pbkdf2_hmac("sha512", b"", salt, ITERATIONS, KEY_SIZE)
    # busctl is part of the base desktop. Older runtime images acquire
    # python3-gi asynchronously for IBus, after this autostart has run.
    # Wallet creation must not depend on that background package repair.
    bus = ["busctl", "--user", "--timeout=60", "call", "org.kde.kwalletd6",
           "/modules/kwalletd6", "org.kde.KWallet"]
    try:
        result = subprocess.run([*bus, "pamOpen", "sayi", WALLET, str(len(key)),
                                 *map(str, key), "0"], capture_output=True, text=True, timeout=65)
        if result.returncode:
            print(f"portal-kwallet-unlock: {result.stderr.strip()}", file=sys.stderr)
        else:
            # pamOpen has no D-Bus return value. A successful method call
            # alone does not prove that the wallet opened.
            opened = subprocess.run([*bus, "isOpen", "s", WALLET], capture_output=True,
                                    text=True, timeout=65)
            if opened.returncode or opened.stdout.strip() != "b true":
                print("portal-kwallet-unlock: wallet remains locked; keeping its password and settings", file=sys.stderr)
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"portal-kwallet-unlock: {type(error).__name__}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
