# Archived graphics artifacts

This directory is audit-only. Nothing below it is packaged, provisioned, or
selected by the active Portal setup path.

The archived KWin 6.3.6 libraries, the earlier unpatched KWin 6.7.4 library,
and the old Anland load-time stub remain here only for provenance and binary
comparison. The active graphics tuple is the exact pair under
`assets/kwin-forky-anland-arm64/`, checked by `scripts/verify_graphics_stack.py`.

Do not copy an artifact from this directory into the runtime without adding a
new source, patch, hash, ABI, and physical-device validation record.
