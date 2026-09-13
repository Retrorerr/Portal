//! Durable renderer selection policy shared by Android startup and host tests.
//!
//! The renderer flag is app-private state, not an accidental side effect of
//! Mesa provisioning. A missing flag on fresh/image-only state is initialized
//! to Anland; a completed Portal marker without a flag is kept on the old
//! QPainter compatibility path. Legacy two-line markers are migrated with an
//! explicit QPainter value so older installations retain their behavior.
//! Once a value exists it is never silently rewritten.

use crate::core::provisioning::RuntimeClassification;
use std::{fs, io::ErrorKind, path::Path};

pub const RENDERER_MODE_FILE: &str = "renderer-mode";
pub const ANLAND_MODE: &str = "anland";
pub const QPAINTER_MODE: &str = "qpainter";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererSelection {
    Anland,
    QPainter,
}

impl RendererSelection {
    pub const fn persisted_value(self) -> &'static [u8] {
        match self {
            Self::Anland => b"anland\n",
            Self::QPainter => b"qpainter\n",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParsedRendererMode {
    Anland,
    QPainter,
    Malformed,
}

impl ParsedRendererMode {
    pub const fn selection(self) -> RendererSelection {
        match self {
            Self::Anland => RendererSelection::Anland,
            Self::QPainter | Self::Malformed => RendererSelection::QPainter,
        }
    }
}

/// Parse only documented renderer values. Malformed/unknown values retain the
/// safe historical QPainter path and are never rewritten, so a recovery/debug
/// choice cannot be silently changed by a later launch.
pub fn parse_renderer_mode(raw: &str) -> ParsedRendererMode {
    match raw.trim().to_ascii_lowercase().as_str() {
        ANLAND_MODE => ParsedRendererMode::Anland,
        "qpainter" | "q_painter" | "q-painter" | "smithay" | "smithay-qpainter" => {
            ParsedRendererMode::QPainter
        }
        _ => ParsedRendererMode::Malformed,
    }
}

fn missing_mode_selection(runtime: RuntimeClassification) -> RendererSelection {
    if runtime.is_trusted_recovery() {
        // A completed runtime predates (or lost) renderer-mode state. Older
        // Portal releases used QPainter when this file did not exist. Record
        // that compatibility decision instead of changing an existing user's
        // renderer merely because the new flag was never present.
        RendererSelection::QPainter
    } else {
        RendererSelection::Anland
    }
}

fn read_existing_mode(path: &Path) -> anyhow::Result<Option<RendererSelection>> {
    match fs::read_to_string(path) {
        Ok(raw) => {
            let parsed = parse_renderer_mode(&raw);
            if parsed == ParsedRendererMode::Malformed {
                log::warn!(
                    "renderer-mode is malformed at {}; preserving it and using QPainter recovery",
                    path.display()
                );
            }
            Ok(Some(parsed.selection()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Resolve the renderer without changing disk state. This is used by hot
/// startup paths; installation/finalisation calls [`ensure_renderer_mode`]
/// first so a successful new install always has durable selection state.
pub fn resolve_renderer_mode(
    path: &Path,
    runtime: RuntimeClassification,
) -> anyhow::Result<RendererSelection> {
    Ok(read_existing_mode(path)?.unwrap_or_else(|| missing_mode_selection(runtime)))
}

/// Ensure a renderer choice is durable and then return the exact choice that
/// startup must use. A completed legacy/modern marker gets an explicit
/// QPainter compatibility value when the flag is absent; fresh/image-only
/// state gets Anland. Existing valid or malformed files are preserved.
pub fn ensure_renderer_mode(
    path: &Path,
    runtime: RuntimeClassification,
) -> anyhow::Result<RendererSelection> {
    ensure_renderer_mode_with_writer(path, runtime, |path, contents| {
        crate::core::provisioning::write_atomic(path, contents)
    })
}

fn ensure_renderer_mode_with_writer<F>(
    path: &Path,
    runtime: RuntimeClassification,
    write: F,
) -> anyhow::Result<RendererSelection>
where
    F: FnOnce(&Path, &[u8]) -> anyhow::Result<()>,
{
    if let Some(selection) = read_existing_mode(path)? {
        return Ok(selection);
    }

    let selection = missing_mode_selection(runtime);
    write(path, selection.persisted_value())?;
    let persisted = fs::read_to_string(path)?;
    let parsed = parse_renderer_mode(&persisted);
    anyhow::ensure!(
        parsed != ParsedRendererMode::Malformed && parsed.selection() == selection,
        "renderer-mode changed while it was being initialized"
    );
    Ok(selection)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn mode_path(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join(RENDERER_MODE_FILE)
    }

    #[test]
    fn fresh_missing_mode_initializes_anland_atomically_and_idempotently() {
        let temp = tempdir().unwrap();
        let path = mode_path(temp.path());

        assert_eq!(
            ensure_renderer_mode(&path, RuntimeClassification::Absent).unwrap(),
            RendererSelection::Anland
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "anland\n");
        assert_eq!(
            ensure_renderer_mode(&path, RuntimeClassification::ValidatedImageOnly).unwrap(),
            RendererSelection::Anland
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "anland\n");
    }

    #[test]
    fn image_ready_state_with_mesa_policy_stays_anland_for_first_handoff() {
        let temp = tempdir().unwrap();
        let path = mode_path(temp.path());
        let selection =
            ensure_renderer_mode(&path, RuntimeClassification::ValidatedImageOnly).unwrap();

        assert_eq!(selection, RendererSelection::Anland);
        assert_eq!(
            resolve_renderer_mode(&path, RuntimeClassification::ValidatedImageOnly).unwrap(),
            RendererSelection::Anland
        );
    }

    #[test]
    fn explicit_anland_is_preserved() {
        let temp = tempdir().unwrap();
        let path = mode_path(temp.path());
        fs::write(&path, "AnLaNd\n").unwrap();

        assert_eq!(
            ensure_renderer_mode(&path, RuntimeClassification::BootablePortal).unwrap(),
            RendererSelection::Anland
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "AnLaNd\n");
    }

    #[test]
    fn explicit_qpainter_and_smithay_values_are_preserved() {
        for value in ["qpainter\n", "smithay\n", "smithay-qpainter\n"] {
            let temp = tempdir().unwrap();
            let path = mode_path(temp.path());
            fs::write(&path, value).unwrap();

            assert_eq!(
                ensure_renderer_mode(&path, RuntimeClassification::Absent).unwrap(),
                RendererSelection::QPainter
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), value);
        }
    }

    #[test]
    fn legacy_missing_mode_is_migrated_to_explicit_qpainter() {
        let temp = tempdir().unwrap();
        let path = mode_path(temp.path());

        assert_eq!(
            resolve_renderer_mode(&path, RuntimeClassification::LegacyPortal).unwrap(),
            RendererSelection::QPainter
        );
        assert!(!path.exists());
        assert_eq!(
            ensure_renderer_mode(&path, RuntimeClassification::LegacyPortal).unwrap(),
            RendererSelection::QPainter
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "qpainter\n");
        // Once the legacy choice is explicit, a later modern classification
        // cannot silently migrate it to Anland.
        assert_eq!(
            ensure_renderer_mode(&path, RuntimeClassification::BootablePortal).unwrap(),
            RendererSelection::QPainter
        );
    }

    #[test]
    fn completed_modern_missing_mode_preserves_qpainter_compatibility() {
        let temp = tempdir().unwrap();
        let path = mode_path(temp.path());

        assert_eq!(
            resolve_renderer_mode(&path, RuntimeClassification::BootablePortal).unwrap(),
            RendererSelection::QPainter
        );
        assert_eq!(
            ensure_renderer_mode(&path, RuntimeClassification::BootablePortal).unwrap(),
            RendererSelection::QPainter
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "qpainter\n");
    }

    #[test]
    fn malformed_mode_is_deterministic_safe_qpainter_and_is_not_rewritten() {
        let temp = tempdir().unwrap();
        let path = mode_path(temp.path());
        fs::write(&path, "not-a-renderer\n").unwrap();

        assert_eq!(
            parse_renderer_mode("not-a-renderer"),
            ParsedRendererMode::Malformed
        );
        assert_eq!(
            ensure_renderer_mode(&path, RuntimeClassification::ValidatedImageOnly).unwrap(),
            RendererSelection::QPainter
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "not-a-renderer\n");
    }

    #[test]
    fn initialization_failure_is_retryable_and_post_rename_state_is_reused() {
        let temp = tempdir().unwrap();
        let path = mode_path(temp.path());
        let error = ensure_renderer_mode_with_writer(
            &path,
            RuntimeClassification::Absent,
            |path, contents| {
                crate::core::provisioning::write_atomic(path, contents)?;
                Err(anyhow::anyhow!(
                    "simulated process death after atomic rename"
                ))
            },
        );
        assert!(error.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "anland\n");
        assert_eq!(
            ensure_renderer_mode(&path, RuntimeClassification::Absent).unwrap(),
            RendererSelection::Anland
        );

        let second = tempdir().unwrap();
        let second_path = mode_path(second.path());
        let error = ensure_renderer_mode_with_writer(
            &second_path,
            RuntimeClassification::Absent,
            |_path, _contents| Err(anyhow::anyhow!("simulated process death before rename")),
        );
        assert!(error.is_err());
        assert!(!second_path.exists());
        assert_eq!(
            ensure_renderer_mode(&second_path, RuntimeClassification::Absent).unwrap(),
            RendererSelection::Anland
        );
    }

    #[test]
    fn normal_subsequent_resolution_uses_persisted_selection() {
        let temp = tempdir().unwrap();
        let path = mode_path(temp.path());
        fs::write(&path, QPAINTER_MODE).unwrap();

        assert_eq!(
            resolve_renderer_mode(&path, RuntimeClassification::BootablePortal).unwrap(),
            RendererSelection::QPainter
        );
        assert_eq!(
            resolve_renderer_mode(&path, RuntimeClassification::Absent).unwrap(),
            RendererSelection::QPainter
        );
    }
}
