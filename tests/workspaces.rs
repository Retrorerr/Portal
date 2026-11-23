//! Workspace registry: layout, the implicit main workspace, and the rules
//! the veil's Workspaces card relies on.

use localdesktop::core::workspaces::{
    abandon_setup, activate, active_dir, delete, delete_files, delete_running, edit,
    normalise_name, registry_path, workspace_dir, workspace_rootfs, AfterDelete, Registry,
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

    // A new workspace opens once, ahead of the one chosen for launch; the
    // launch after that goes back to the chosen one.
    let (_, fresh) = edit(temp.path(), |registry| {
        registry.create(temp.path(), "Fresh", 45)
    })
    .unwrap();
    assert_eq!(activate(temp.path(), 50).id, fresh.id);
    let saved = Registry::read(temp.path()).unwrap();
    assert_eq!(saved.active, fresh.id);
    assert_eq!(saved.open_next, None);
    assert_eq!(activate(temp.path(), 60).id, "uni");

    // With Main deleted, a workspace whose folder has gone falls back to the
    // last one used that still has its own.
    let (_, ()) = edit(temp.path(), |registry| {
        registry.set_open_at_launch(None)?;
        registry.remove(MAIN_ID)?;
        Ok(())
    })
    .unwrap();
    fs::remove_dir_all(workspace_dir(temp.path(), "uni")).unwrap();
    assert_eq!(activate(temp.path(), 70).id, fresh.id);
    assert_eq!(Registry::read(temp.path()).unwrap().active, fresh.id);
    // With no folder left at all, the target's is made again for its
    // installer rather than opening nothing.
    fs::remove_dir_all(workspace_dir(temp.path(), &fresh.id)).unwrap();
    assert_eq!(activate(temp.path(), 80).id, fresh.id);
    assert!(workspace_dir(temp.path(), &fresh.id).is_dir());
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

#[test]
fn a_switch_needs_another_workspace_whose_system_is_there() {
    let temp = tempfile::tempdir().unwrap();
    let mut registry = Registry::implicit();
    let uni = registry.add("Uni", 1).unwrap();
    assert_eq!(
        registry.switch_target(temp.path(), MAIN_ID, &ready),
        Err(WorkspaceError::AlreadyActive)
    );
    assert_eq!(
        registry.switch_target(temp.path(), "nope", &ready),
        Err(WorkspaceError::Unknown("nope".into()))
    );
    assert_eq!(
        registry.switch_target(temp.path(), &uni.id, &ready),
        Err(WorkspaceError::FilesMissing)
    );
    // A workspace whose install hasn't committed opens through its installer.
    fs::create_dir_all(workspace_rootfs(temp.path(), &uni.id)).unwrap();
    assert_eq!(
        registry.switch_target(temp.path(), &uni.id, &ready),
        Err(WorkspaceError::NeedsSetup)
    );
    mark_ready(temp.path(), &uni.id);
    assert_eq!(
        registry
            .switch_target(temp.path(), &uni.id, &ready)
            .unwrap()
            .name,
        "Uni"
    );
}

/// Stands in for the runtime's completion marker.
fn ready(rootfs: &Path) -> bool {
    rootfs.join("ready").is_file()
}

fn mark_ready(files: &Path, id: &str) {
    let rootfs = workspace_rootfs(files, id);
    fs::create_dir_all(&rootfs).unwrap();
    fs::write(rootfs.join("ready"), "").unwrap();
}

#[test]
fn a_new_workspace_is_an_empty_folder_the_next_launch_opens() {
    let temp = tempfile::tempdir().unwrap();
    mark_ready(temp.path(), MAIN_ID);
    let mut registry = Registry::implicit();
    registry.set_open_at_launch(Some(MAIN_ID)).unwrap();
    // A folder left by a workspace given up earlier is never reused.
    fs::create_dir_all(workspace_dir(temp.path(), "uni").join("runtime-B")).unwrap();
    let uni = registry.create(temp.path(), " Uni ", 7).unwrap();
    assert_eq!((uni.id.as_str(), uni.name.as_str()), ("uni-2", "Uni"));
    assert_eq!(uni.created_unix, 7);
    let dir = workspace_dir(temp.path(), &uni.id);
    assert!(dir.is_dir());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    // The restart opens it once, ahead of the workspace chosen for launch.
    assert_eq!(registry.open_next.as_deref(), Some("uni-2"));
    assert_eq!(registry.open_at_launch.as_deref(), Some(MAIN_ID));
    registry.validate().unwrap();
    assert_eq!(
        registry.create(temp.path(), "UNI", 8),
        Err(WorkspaceError::NameTaken)
    );
    assert_eq!(
        registry.create(temp.path(), "  ", 8),
        Err(WorkspaceError::EmptyName)
    );
    assert_eq!(registry.workspaces.len(), 2);

    let listing: serde_json::Value =
        serde_json::from_str(&registry.listing_json(temp.path(), &ready)).unwrap();
    assert_eq!(listing["workspaces"][0]["needsSetup"], false);
    assert_eq!(listing["workspaces"][1]["needsSetup"], true);
    // Main is running and set up, so its installer has nowhere to go back to.
    assert!(listing["setupReturn"].is_null());
}

#[test]
fn an_unfinished_workspace_reopens_its_installer() {
    let temp = tempfile::tempdir().unwrap();
    mark_ready(temp.path(), MAIN_ID);
    let mut registry = Registry::implicit();
    let uni = registry.add("Uni", 1).unwrap();
    // Its folder went missing: it starts again empty.
    registry
        .request_setup(temp.path(), &uni.id, 2, &ready)
        .unwrap();
    assert!(workspace_dir(temp.path(), &uni.id).is_dir());
    assert_eq!(registry.open_next.as_deref(), Some("uni"));
    mark_ready(temp.path(), &uni.id);
    assert_eq!(
        registry.request_setup(temp.path(), &uni.id, 2, &ready),
        Err(WorkspaceError::AlreadySetUp)
    );
    assert_eq!(
        registry.request_setup(temp.path(), MAIN_ID, 2, &ready),
        Err(WorkspaceError::AlreadySetUp)
    );
}

#[test]
fn giving_up_goes_back_to_the_workspace_it_was_started_from() {
    let temp = tempfile::tempdir().unwrap();
    mark_ready(temp.path(), MAIN_ID);
    let mut registry = Registry::implicit();
    let games = registry.add("Games", 1).unwrap();
    mark_ready(temp.path(), &games.id);
    registry.set_active(&games.id, 10).unwrap();
    registry.set_active(MAIN_ID, 20).unwrap();
    // Main is left at 30 for the new workspace, after Games was last used.
    let uni = registry.create(temp.path(), "Uni", 30).unwrap();
    assert_eq!(registry.get(MAIN_ID).unwrap().last_used_unix, 30);
    registry.set_active(&uni.id, 31).unwrap();
    assert_eq!(
        registry.setup_return(temp.path(), &ready).unwrap().id,
        MAIN_ID
    );
    // Reopening the unfinished one from Games marks Games as left.
    registry.set_active(&games.id, 40).unwrap();
    registry
        .request_setup(temp.path(), &uni.id, 50, &ready)
        .unwrap();
    registry.set_active(&uni.id, 51).unwrap();
    assert_eq!(
        registry.setup_return(temp.path(), &ready).unwrap().id,
        games.id
    );
}

#[test]
fn giving_up_a_setup_goes_back_to_the_last_workspace_that_works() {
    let temp = tempfile::tempdir().unwrap();
    let mut registry = Registry::implicit();
    let games = registry.add("Games", 1).unwrap();
    mark_ready(temp.path(), &games.id);
    let broken = registry.add("Broken", 1).unwrap();
    registry.set_active(&games.id, 50).unwrap();
    registry.set_active(&broken.id, 60).unwrap();
    let uni = registry.create(temp.path(), "Uni", 70).unwrap();
    registry.set_active(&uni.id, 80).unwrap();
    registry.set_open_at_launch(Some(&uni.id)).unwrap();

    // Broken was used later but isn't set up; Games is the last that works.
    assert_eq!(
        registry.setup_return(temp.path(), &ready).unwrap().id,
        games.id
    );
    let (removed, back) = registry.abandon_setup(temp.path(), &ready).unwrap();
    assert_eq!((removed.id.as_str(), back.id.as_str()), ("uni", "games"));
    assert_eq!(registry.active, "games");
    assert_eq!(registry.open_next.as_deref(), Some("games"));
    assert_eq!(registry.open_at_launch, None);
    assert!(registry.get("uni").is_none());
    registry.validate().unwrap();
    // A set-up workspace has no installer to leave.
    assert_eq!(
        registry.abandon_setup(temp.path(), &ready),
        Err(WorkspaceError::AlreadySetUp)
    );
    // Main whose install was deleted is given up like any other.
    registry.set_active(MAIN_ID, 90).unwrap();
    let (removed, back) = registry.abandon_setup(temp.path(), &ready).unwrap();
    assert_eq!((removed.id.as_str(), back.id.as_str()), (MAIN_ID, "games"));
    assert!(registry.get(MAIN_ID).is_none());
    registry.validate().unwrap();
}

#[test]
fn giving_up_saves_the_registry_then_deletes_only_that_workspace() {
    let temp = tempfile::tempdir().unwrap();
    mark_ready(temp.path(), MAIN_ID);
    let (_, uni) = edit(temp.path(), |registry| {
        let uni = registry.create(temp.path(), "Uni", 1)?;
        registry.set_active(&uni.id, 2)?;
        Ok(uni)
    })
    .unwrap();
    let rootfs = workspace_rootfs(temp.path(), &uni.id);
    fs::create_dir_all(rootfs.join("usr/share/locked/deeper")).unwrap();
    fs::write(rootfs.join("usr/share/locked/deeper/file"), "x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            rootfs.join("usr/share/locked"),
            fs::Permissions::from_mode(0o500),
        )
        .unwrap();
    }
    fs::write(temp.path().join("renderer-mode"), "anland").unwrap();

    let back = abandon_setup(temp.path(), &ready).unwrap();
    assert_eq!(back.id, MAIN_ID);
    assert!(!workspace_dir(temp.path(), &uni.id).exists());
    // Main's files, which are Portal's own, are untouched.
    assert!(temp.path().join("renderer-mode").is_file());
    let saved = Registry::read(temp.path()).unwrap();
    assert_eq!(saved.active, MAIN_ID);
    assert_eq!(saved.open_next.as_deref(), Some(MAIN_ID));
    assert!(saved.get(&uni.id).is_none());

    // A finished workspace that isn't running can be deleted; the running
    // one can't be removed this way (delete_running).
    let (_, games) = edit(temp.path(), |registry| {
        registry.create(temp.path(), "Games", 3)
    })
    .unwrap();
    mark_ready(temp.path(), &games.id);
    assert_eq!(delete(temp.path(), &games.id).unwrap().id, games.id);
    assert!(!workspace_dir(temp.path(), &games.id).exists());
    assert!(Registry::read(temp.path())
        .unwrap()
        .get(&games.id)
        .is_none());
    let (_, uni) = edit(temp.path(), |registry| {
        let uni = registry.create(temp.path(), "Uni", 4)?;
        registry.set_active(&uni.id, 5)?;
        Ok(uni)
    })
    .unwrap();
    assert_eq!(
        delete(temp.path(), &uni.id)
            .unwrap_err()
            .downcast_ref::<WorkspaceError>(),
        Some(&WorkspaceError::ActiveCannotBeRemoved)
    );
    assert!(workspace_dir(temp.path(), &uni.id).is_dir());

    assert!(delete_files(temp.path(), "..").is_err());
    assert!(delete_files(temp.path(), "").is_err());
    assert!(temp.path().join("renderer-mode").is_file());
}

#[test]
fn the_card_lists_workspaces_in_the_order_they_were_made() {
    let mut registry = Registry::implicit();
    let old = registry.add("Old", 1).unwrap();
    let recent = registry.add("Recent", 2).unwrap();
    registry.set_active(&old.id, 10).unwrap();
    registry.set_active(&recent.id, 20).unwrap();
    registry.set_active(MAIN_ID, 30).unwrap();
    registry.set_open_at_launch(Some(&old.id)).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let listing: serde_json::Value =
        serde_json::from_str(&registry.listing_json(temp.path(), &ready)).unwrap();
    let names: Vec<&str> = listing["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|workspace| workspace["name"].as_str().unwrap())
        .collect();
    // The running one doesn't jump to the front: cards keep their places.
    assert_eq!(names, ["Main", "Old", "Recent"]);
    assert_eq!(listing["active"], MAIN_ID);
    assert_eq!(listing["openAtLaunch"], "old");
    assert_eq!(listing["workspaces"][2]["lastUsed"], 20);
}

#[test]
fn edits_save_only_what_succeeded() {
    let temp = tempfile::tempdir().unwrap();
    let (registry, ()) = edit(temp.path(), |registry| registry.rename(MAIN_ID, "Home")).unwrap();
    assert_eq!(Registry::read(temp.path()).unwrap(), registry);
    assert!(edit(temp.path(), |registry| registry.rename(MAIN_ID, " ")).is_err());
    assert_eq!(Registry::read(temp.path()).unwrap().active().name, "Home");

    // An unreadable registry is never replaced by an edit.
    fs::write(registry_path(temp.path()), "not json").unwrap();
    assert!(edit(temp.path(), |registry| registry.rename(MAIN_ID, "Again")).is_err());
    assert_eq!(
        fs::read_to_string(registry_path(temp.path())).unwrap(),
        "not json"
    );
}

/// Lays out a Main install the way the app files directory holds one,
/// beside Portal's own files.
fn main_install(files: &Path) {
    mark_ready(files, MAIN_ID);
    let rootfs = workspace_rootfs(files, MAIN_ID);
    fs::create_dir_all(rootfs.join("usr/share/locked/deeper")).unwrap();
    fs::write(rootfs.join("usr/share/locked/deeper/file"), "x").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            rootfs.join("usr/share/locked"),
            fs::Permissions::from_mode(0o500),
        )
        .unwrap();
    }
    for dir in [
        "runtime-B.previous",
        "runtime-B.invalid.2",
        "tawcroot/link",
        ".tawcroot-tmpfs/tmp",
    ] {
        fs::create_dir_all(files.join(dir)).unwrap();
    }
    for file in [
        "portal-runtime.tar.xz.part",
        "portal-runtime.tar.xz.part.offset",
        "portal-install-plan-v1.json",
        "optional-apps-deferred",
    ] {
        fs::write(files.join(file), "x").unwrap();
    }
}

/// Portal's own files, which no workspace's delete may touch.
const SHARED: &[&str] = &[
    "renderer-mode",
    "tawcroot-instances/1/marker",
    "updates/state.json",
    "mesa-kgsl-layer/lib",
    "runtime-Backup",
    "workspaces/previews/main.jpg",
];

fn shared_files(files: &Path) {
    for file in SHARED {
        let path = files.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "keep").unwrap();
    }
}

fn assert_shared_kept(files: &Path) {
    for file in SHARED {
        assert!(files.join(file).is_file(), "{file} was deleted");
    }
}

fn assert_main_install_gone(files: &Path) {
    for name in [
        "runtime-B",
        "runtime-B.previous",
        "runtime-B.invalid.2",
        "tawcroot",
        ".tawcroot-tmpfs",
        "portal-runtime.tar.xz.part",
        "portal-runtime.tar.xz.part.offset",
        "portal-install-plan-v1.json",
        "optional-apps-deferred",
        "workspaces/.deleting-main",
    ] {
        assert!(
            fs::symlink_metadata(files.join(name)).is_err(),
            "{name} survived"
        );
    }
}

#[test]
fn main_can_be_deleted_leaving_portals_own_files() {
    let temp = tempfile::tempdir().unwrap();
    main_install(temp.path());
    shared_files(temp.path());
    let (_, games) = edit(temp.path(), |registry| {
        let games = registry.add("Games", 1)?;
        registry.set_active(&games.id, 2)?;
        registry.set_open_at_launch(Some(MAIN_ID))?;
        Ok(games)
    })
    .unwrap();
    mark_ready(temp.path(), &games.id);

    assert_eq!(delete(temp.path(), MAIN_ID).unwrap().id, MAIN_ID);
    assert_main_install_gone(temp.path());
    assert_shared_kept(temp.path());
    assert!(workspace_rootfs(temp.path(), &games.id)
        .join("ready")
        .is_file());
    let saved = Registry::read(temp.path()).unwrap();
    assert!(saved.get(MAIN_ID).is_none());
    // Launch no longer points at it.
    assert_eq!(saved.open_at_launch, None);
    assert_eq!(saved.launch_target().id, games.id);
    // A new workspace never takes Main's id, which is the app files directory.
    let (_, again) = edit(temp.path(), |registry| registry.add("Main", 3)).unwrap();
    assert_ne!(again.id, MAIN_ID);
}

#[test]
fn an_interrupted_main_delete_leaves_no_install_and_is_finished_next_time() {
    let temp = tempfile::tempdir().unwrap();
    main_install(temp.path());
    shared_files(temp.path());
    // A delete cut short left its trash behind.
    let trash = temp.path().join("workspaces/.deleting-main/runtime-B/etc");
    fs::create_dir_all(&trash).unwrap();
    assert!(!workspace_rootfs(temp.path(), MAIN_ID).join("etc").exists());
    delete_files(temp.path(), MAIN_ID).unwrap();
    assert_main_install_gone(temp.path());
    assert_shared_kept(temp.path());
}

#[test]
fn deleting_the_open_workspace_switches_to_the_last_one_used() {
    let temp = tempfile::tempdir().unwrap();
    main_install(temp.path());
    shared_files(temp.path());
    let (_, (games, uni)) = edit(temp.path(), |registry| {
        let games = registry.add("Games", 1)?;
        let uni = registry.add("Uni", 1)?;
        registry.set_active(&uni.id, 3)?;
        registry.set_active(MAIN_ID, 5)?;
        registry.set_active(&games.id, 10)?;
        registry.set_open_at_launch(Some(&games.id))?;
        Ok((games, uni))
    })
    .unwrap();
    mark_ready(temp.path(), &games.id);
    mark_ready(temp.path(), &uni.id);
    let listing: serde_json::Value = serde_json::from_str(
        &Registry::read(temp.path())
            .unwrap()
            .listing_json(temp.path(), &ready),
    )
    .unwrap();
    assert_eq!(listing["afterDelete"]["id"], MAIN_ID);

    // Another workspace opened meanwhile, or the plan changed: nothing goes.
    for (id, expected) in [
        (MAIN_ID, Some(uni.id.as_str())),
        (games.id.as_str(), Some("uni")),
    ] {
        assert_eq!(
            delete_running(temp.path(), id, expected, 20, &ready)
                .unwrap_err()
                .downcast_ref::<WorkspaceError>(),
            Some(&WorkspaceError::NotOpen)
        );
    }
    assert!(workspace_rootfs(temp.path(), &games.id).is_dir());

    let after = delete_running(temp.path(), &games.id, Some(MAIN_ID), 20, &ready).unwrap();
    let AfterDelete::Switched(main) = after else {
        panic!("expected a switch, got {after:?}");
    };
    assert_eq!(main.id, MAIN_ID);
    let saved = Registry::read(temp.path()).unwrap();
    assert_eq!(saved.active, MAIN_ID);
    assert!(saved.get(&games.id).is_none());
    assert_eq!(saved.open_next, None);
    assert_eq!(saved.open_at_launch, None);
    // Its files are the caller's to delete once the switch is under way.
    delete_files(temp.path(), &games.id).unwrap();
    assert!(!workspace_dir(temp.path(), &games.id).exists());
    assert!(workspace_rootfs(temp.path(), &uni.id).is_dir());
    assert_shared_kept(temp.path());
}

#[test]
fn deleting_the_open_workspace_with_only_unfinished_ones_left_restarts_into_one() {
    let temp = tempfile::tempdir().unwrap();
    main_install(temp.path());
    shared_files(temp.path());
    let (_, uni) = edit(temp.path(), |registry| {
        let uni = registry.create(temp.path(), "Uni", 1)?;
        registry.set_active(&uni.id, 2)?;
        registry.set_active(MAIN_ID, 3)?;
        Ok(uni)
    })
    .unwrap();

    let after = delete_running(temp.path(), MAIN_ID, Some(&uni.id), 4, &ready).unwrap();
    assert_eq!(
        after,
        AfterDelete::Restart(
            Registry::read(temp.path())
                .unwrap()
                .get(&uni.id)
                .unwrap()
                .clone()
        )
    );
    assert_main_install_gone(temp.path());
    assert_shared_kept(temp.path());
    let saved = Registry::read(temp.path()).unwrap();
    assert!(saved.get(MAIN_ID).is_none());
    assert_eq!(saved.active, uni.id);
    assert_eq!(saved.open_next.as_deref(), Some(uni.id.as_str()));
}

#[test]
fn deleting_the_only_workspace_starts_over_with_nowhere_to_go_back() {
    let temp = tempfile::tempdir().unwrap();
    main_install(temp.path());
    shared_files(temp.path());
    let (_, ()) = edit(temp.path(), |registry| {
        registry.set_open_at_launch(Some(MAIN_ID))
    })
    .unwrap();
    let after = delete_running(temp.path(), MAIN_ID, None, 5, &ready).unwrap();
    let AfterDelete::Restart(main) = after else {
        panic!("expected a restart, got {after:?}");
    };
    assert_eq!(main.id, MAIN_ID);
    assert_main_install_gone(temp.path());
    assert_shared_kept(temp.path());
    let saved = Registry::read(temp.path()).unwrap();
    // Still listed for its fresh install, which the next launch opens.
    assert_eq!(saved.workspaces.len(), 1);
    assert_eq!(saved.open_next.as_deref(), Some(MAIN_ID));
    assert_eq!(saved.open_at_launch, None);
    assert_eq!(saved.setup_return(temp.path(), &ready), None);
    assert_eq!(saved.after_deleting_active(temp.path(), &ready), None);
    assert!(saved
        .listing_json(temp.path(), &ready)
        .contains("\"needsSetup\":true"));
    // A fresh install of Main alone never offers to go back to itself.
    assert_eq!(Registry::implicit().setup_return(temp.path(), &ready), None);

    // A workspace emptied this way keeps its folder for the installer.
    let (_, games) = edit(temp.path(), |registry| {
        let games = registry.create(temp.path(), "Games", 6)?;
        registry.set_active(&games.id, 7)?;
        registry.remove(MAIN_ID)?;
        Ok(games)
    })
    .unwrap();
    mark_ready(temp.path(), &games.id);
    delete_running(temp.path(), &games.id, None, 8, &ready).unwrap();
    let dir = workspace_dir(temp.path(), &games.id);
    assert!(dir.is_dir());
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
}
