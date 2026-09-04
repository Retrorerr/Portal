# Contributing to Portal

Portal crosses Android, native graphics, Wayland, and a Debian userspace. A useful change identifies which boundary it affects and proves that boundary directly.

## Before changing code

1. Search the issue tracker and existing pull requests.
2. Keep the change focused. Avoid mixing runtime behaviour, formatting, generated binaries, and documentation in one commit.
3. Preserve the compatibility identifiers listed in the README unless a migration plan is part of the change.
4. Do not replace the native-Wayland Plasma session with X11 or a remote-desktop path as an implicit fallback.

## Local checks

For Rust changes:

```bash
cargo fmt --all -- --check
cargo test --tests
```

Run relevant syntax checks for modified shell, PowerShell, Python, Java, or XML assets. Android changes should be built for `arm64-v8a`; release candidates also require package, signing, alignment, install, launch, log, and physical-device checks from `docs/arm64-validation.md`.

## Pull requests

Explain the problem, the chosen boundary, and the evidence. Screenshots are useful for visible behaviour, but do not substitute emulator evidence for a real ARM64 runtime result.

Pull requests to `main` use exactly one release label:

- `release:breaking`
- `release:feature`
- `release:bugfix`
- `release:patch`
- `release:improvement`
- `release:optimisation`
- `release:docs`

Generated APKs, logs, diagnostics archives, and device captures should not be committed unless they are intentional fixtures or release artifacts.

## Commit style

Use a short imperative subject with a meaningful scope, for example:

```text
fix(ime): retain focus across activity resume
docs: clarify signing-key upgrade boundary
```

## Reporting bugs

Use the bug template. Include the device model, Android version, Portal version or commit, install history, exact reproduction steps, and the narrowest logs that demonstrate the failure. Review diagnostic archives before sharing them.
