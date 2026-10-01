# Security policy

## Android / PRoot release boundary

PRoot is a compatibility layer, not a security boundary. Guest applications run
with Portal's Android UID and share its files, sockets and process permissions.
Guest root is simulated; it does not grant Android root. Android's app sandbox
still separates Portal from other apps, subject to permissions the user grants.

Browser sandboxes run with their seccomp-bpf filters and file brokers as on
desktop Linux: PRoot passes their seccomp traps through and emulates the user
and PID namespaces they create. The namespaces are emulated, not enforced by
the kernel, so a compromised renderer that escapes its seccomp filter is not
contained by them the way it would be on desktop Linux. The `bwrap`
compatibility wrapper executes its target without namespace isolation. Do not
treat guest apps or purported Flatpak containers as isolated from one another.

Shared storage is bound at `/android` and `/root/Android` only when the user
grants all-files access. That grant exposes shared storage to every guest app.
Portal does not bind other Android apps' private directories. `/dev`, `/proc`
and `/sys` remain constrained by Android UID permissions and SELinux.

The clipboard and setup brokers listen on loopback and authenticate with
per-session tokens; loopback alone is not trusted because other Android apps
can connect. Clipboard messages are bounded text, not arbitrary commands.
Tokens are intentionally available to the guest bridge, so another compromised
guest process can impersonate it. Host/guest Wayland and audio sockets live
under Portal's app-private directory, not shared storage. The former permanent
session shell-command FIFO/autostart runner is removed by setup.

The local release candidate uses the existing installation's development
signing certificate for a data-preserving update. Public distribution requires
a protected production signing key and must not reuse that certificate.

## Supported code

Security fixes target the latest release and current `main`. Older development builds may not receive patches.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Use **Report a vulnerability** in the repository's Security tab so the report, discussion, and any fix can remain private until disclosure is safe.

Include:

- the affected Portal version or commit;
- the Android and device versions involved;
- a minimal reproduction or proof of concept;
- the impact and required attacker position;
- whether the issue crosses the Android host, PRoot guest, loopback services, clipboard, shared storage, or release pipeline.

Do not include unrelated personal data, production credentials, or signing keys. You should receive an initial acknowledgement through GitHub's private report within seven days.
