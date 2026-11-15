//! Workspace registry: layout, the implicit main workspace, and the rules
//! the veil's Workspaces card relies on.

use localdesktop::core::workspaces::{
    activate, active_dir, normalise_name, registry_path, workspace_dir, workspace_rootfs, Registry,
    WorkspaceError, MAIN_ID, MAX_NAME_CHARS,
};
use std::{fs, path::Path};

#[test]
fn main_is_the_app_files_directory_so_existing_installs_need_no_move() {
    let files = Path::new("/data/data/app.polarbear/files");
    assert_eq!(workspace_dir(files, MAIN_ID), files);
    assert_eq!(workspace_rootfs(files, MAIN_ID), files.join("runtime-B"));
    assert_eq!(
        workspace_rootfs(files, "uni"),
        files.join("workspaces/uni/runtime-B")
    );
    // tawcroot keeps its hardlink store beside the rootfs, so each workspace
    // directory also gets its own store.
    assert_ne!(
        workspace_rootfs(files, MAIN_ID).parent(),
        workspace_rootfs(files, "uni").parent()
    );
}

#[test]
fn a_missing_registry_is_main_alone_and_is_not_written() {
    let temp = tempfile::tempdir().unwrap();
    let registry = Registry::read(temp.path()).unwrap();
    assert_eq!(registry, Registry::implicit());
    assert_eq!(registry.active().id, MAIN_ID);
    assert_eq!(registry.active().name, "Main");
}

#[test]
fn names_are_trimmed_bounded_and_unique_ignoring_case() {
    assert_eq!(normalise_name("  Uni   work ").unwrap(), "Uni work");
    assert_eq!(normalise_name("   "), Err(WorkspaceError::EmptyName));
    assert_eq!(
        normalise_name("a\u{7}b"),
        Err(WorkspaceError::NameHasControlCharacters)
    );
    assert_eq!(
        normalise_name(&"x".repeat(MAX_NAME_CHARS + 1)),
        Err(WorkspaceError::NameTooLong)
    );
    assert!(normalise_name(&"é".repeat(MAX_NAME_CHARS)).is_ok());

    let mut registry = Registry::implicit();
    registry.add("Uni", 1).unwrap();
    assert_eq!(registry.add("uni", 2), Err(WorkspaceError::NameTaken));
    assert_eq!(
        registry.rename("uni", "MAIN"),
        Err(WorkspaceError::NameTaken)
    );
    registry.rename("uni", "UNI").unwrap();
    assert_eq!(registry.get("uni").unwrap().name, "UNI");
}

#[test]
fn ids_are_directory_safe_unique_and_never_main() {
    let mut registry = Registry::implicit();
    registry.rename(MAIN_ID, "Home").unwrap();
    assert_eq!(registry.add("Main", 1).unwrap().id, "main-2");
    assert_eq!(registry.add("Games & Steam!", 1).unwrap().id, "games-steam");
    assert_eq!(registry.add("Games/Steam", 1).unwrap().id, "games-steam-2");
    assert_eq!(registry.add("../..", 1).unwrap().id, "workspace");
    assert_eq!(registry.add("日本語", 1).unwrap().id, "workspace-2");
    let long = registry.add(&"a".repeat(MAX_NAME_CHARS), 1).unwrap();
    assert!(long.id.len() <= 28);
    registry.validate().unwrap();
}

#[test]
fn the_running_workspace_and_the_last_one_cannot_be_removed() {
    let mut registry = Registry::implicit();
    assert_eq!(
        registry.remove(MAIN_ID),
        Err(WorkspaceError::ActiveCannotBeRemoved)
    );
    let uni = registry.add("Uni", 1).unwrap();
    registry.set_open_at_launch(Some(&uni.id)).unwrap();
    assert_eq!(
        registry.remove("nope"),
        Err(WorkspaceError::Unknown("nope".into()))
    );
    assert_eq!(registry.remove(&uni.id).unwrap().name, "Uni");
    // Opening at launch falls back to the last one used.
    assert_eq!(registry.open_at_launch, None);

    let games = registry.add("Games", 2).unwrap();
    registry.set_active(&games.id, 3).unwrap();
    registry.remove(MAIN_ID).unwrap();
    assert_eq!(
        registry.remove(&games.id),
        Err(WorkspaceError::ActiveCannotBeRemoved)
    );
    registry.validate().unwrap();
}

#[test]
fn launch_opens_the_chosen_workspace_or_else_the_last_used() {
    let mut registry = Registry::implicit();
    let uni = registry.add("Uni", 1).unwrap();
    registry.set_active(&uni.id, 5).unwrap();
    assert_eq!(registry.launch_target().id, "uni");
    assert_eq!(registry.get("uni").unwrap().last_used_unix, 5);
    registry.set_open_at_launch(Some(MAIN_ID)).unwrap();
    assert_eq!(registry.launch_target().id, MAIN_ID);
    assert_eq!(
        registry.set_open_at_launch(Some("gone")),
        Err(WorkspaceError::Unknown("gone".into()))
    );
}

#[test]
fn the_registry_round_trips_and_activation_follows_it() {
    let temp = tempfile::tempdir().unwrap();
    // Activation is process-wide state, so this is the only test that
    // activates. An install without a registry opens Main and writes none.
    assert_eq!(activate(temp.path(), 1).id, MAIN_ID);
    assert!(!registry_path(temp.path()).exists());
    assert_eq!(active_dir(temp.path()), temp.path());

    let mut registry = Registry::implicit();
    let uni = registry.add("Uni", 10).unwrap();
    registry.set_active(&uni.id, 20).unwrap();
    registry.save(temp.path()).unwrap();
    assert_eq!(Registry::read(temp.path()).unwrap(), registry);

    // A workspace whose directory is missing is skipped for Main, and the
    // switch is recorded.
    assert_eq!(activate(temp.path(), 30).id, MAIN_ID);
    let saved = Registry::read(temp.path()).unwrap();
    assert_eq!(saved.active, MAIN_ID);
    assert_eq!(saved.get(MAIN_ID).unwrap().last_used_unix, 30);

    // With its directory present, the chosen workspace opens.
    let mut saved = saved;
    saved.set_open_at_launch(Some("uni")).unwrap();
    saved.save(temp.path()).unwrap();
    fs::create_dir_all(workspace_dir(temp.path(), "uni")).unwrap();
    assert_eq!(activate(temp.path(), 40).id, "uni");
    assert_eq!(active_dir(temp.path()), temp.path().join("workspaces/uni"));
    assert_eq!(Registry::read(temp.path()).unwrap().active, "uni");
}

#[test]
fn a_malformed_registry_opens_main_and_is_kept_aside() {
    for text in [
        "not json",
        r#"{"version":1,"active":"gone","workspaces":[{"id":"main","name":"Main"}]}"#,
        r#"{"version":1,"active":"main","workspaces":[{"id":"main","name":"Main"},{"id":"../x","name":"X"}]}"#,
        r#"{"version":1,"active":"main","workspaces":[{"id":"main","name":"Main"},{"id":"b","name":"main"}]}"#,
        r#"{"version":99,"active":"main","workspaces":[{"id":"main","name":"Main"}]}"#,
        r#"{"version":1,"active":"main","workspaces":[]}"#,
    ] {
        let temp = tempfile::tempdir().unwrap();
        let path = registry_path(temp.path());
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        assert!(Registry::read(temp.path()).is_err(), "{text}");
        assert_eq!(Registry::load(temp.path()), Registry::implicit(), "{text}");
        assert!(!path.exists());
        assert_eq!(
            fs::read_to_string(path.with_extension("json.invalid")).unwrap(),
            text
        );
    }
}

#[test]
fn saving_refuses_a_registry_that_breaks_the_rules() {
    let temp = tempfile::tempdir().unwrap();
    let mut registry = Registry::implicit();
    registry.active = "gone".into();
    assert!(registry.save(temp.path()).is_err());
    assert!(!registry_path(temp.path()).exists());
}
