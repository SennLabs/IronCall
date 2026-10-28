//! Workspace behaviour, including the PLAN.md Phase 2 exit criteria.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ironcall_core::ItemId;
use ironcall_core::model::RequestFile;
use ironcall_store::watch::WorkspaceWatcher;
use ironcall_store::workspace::{
    COLLECTION_FILE, DirTrash, FOLDER_FILE, Node, NodeKind, SaveOutcome, Workspace, WorkspaceError,
};
use ironcall_store::yaml;

const CONFLICTED: &str =
    "schema: 'ironcall/request@1'\n<<<<<<< HEAD\nname: 'Mine'\n=======\nname: 'Theirs'\n>>>>>>> branch\n";

fn new_workspace() -> (tempfile::TempDir, Workspace) {
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create(&tmp.path().join("ws"), "My API")
        .unwrap()
        .with_trash(Box::new(DirTrash(tmp.path().join("trash"))));
    (tmp, ws)
}

fn reopen(ws: &Workspace, tmp: &Path) -> Workspace {
    Workspace::open(ws.root())
        .unwrap()
        .with_trash(Box::new(DirTrash(tmp.join("trash"))))
}

fn names(node: &Node) -> Vec<&str> {
    node.children.iter().map(|c| c.name.as_str()).collect()
}

fn write_request(path: &Path, name: &str) -> RequestFile {
    let mut req = RequestFile::new(name);
    req.url = "https://example.com".into();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, yaml::to_string(&req).unwrap()).unwrap();
    req
}

fn collection_order(ws: &Workspace) -> Vec<ItemId> {
    ws.load_collection().unwrap().file.order
}

#[test]
fn create_writes_workspace_files_and_refuses_twice() {
    let (_tmp, ws) = new_workspace();
    let root = ws.root();
    assert!(root.join(COLLECTION_FILE).is_file());
    assert_eq!(
        std::fs::read_to_string(root.join(".gitattributes")).unwrap(),
        "*.yaml text eol=lf\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".ironcall/.gitignore")).unwrap(),
        "*\n"
    );
    assert_eq!(ws.name(), "My API");
    assert!(matches!(
        Workspace::create(root, "Again"),
        Err(WorkspaceError::AlreadyExists(_))
    ));
    assert!(matches!(
        Workspace::open(root.parent().unwrap()),
        Err(WorkspaceError::NotAWorkspace(_))
    ));
}

#[test]
fn create_items_with_slugs_and_stable_ids() {
    let (tmp, mut ws) = new_workspace();
    let root_id = ws.tree().id;
    let users = ws.create_folder(root_id, "Users", None).unwrap().node;
    let get = ws.create_request(users.id, "Get user", None).unwrap().node;
    let get2 = ws.create_request(users.id, "GET USER", None).unwrap().node;
    let cafe = ws.create_request(users.id, "Café ☕", None).unwrap().node;

    assert_eq!(users.path, "users");
    assert_eq!(get.path, "users/get-user.yaml");
    assert_eq!(get2.path, "users/get-user-2.yaml");
    assert_eq!(cafe.path, "users/cafe-coffee.yaml");
    assert_eq!(get.method.as_deref(), Some("GET"));

    let reopened = reopen(&ws, tmp.path());
    let folder = reopened.tree().find(users.id).expect("folder ID is stable");
    assert_eq!(folder.children.len(), 3);
    assert!(reopened.tree().find(get.id).is_some());
}

#[test]
fn rename_changes_display_name_only_until_asked() {
    let (_tmp, mut ws) = new_workspace();
    let root_id = ws.tree().id;
    let req = ws.create_request(root_id, "Old", None).unwrap().node;
    ws.rename(req.id, "New name").unwrap();
    let node = ws.tree().find(req.id).unwrap();
    assert_eq!(node.name, "New name");
    assert_eq!(node.path, "old.yaml", "file name is fixed at creation");

    ws.rename_file_to_match(req.id).unwrap();
    assert_eq!(ws.tree().find(req.id).unwrap().path, "new-name.yaml");
    assert!(!ws.root().join("old.yaml").exists());
}

#[test]
fn ordering_rules_listed_unlisted_and_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    Workspace::create(&root, "W").unwrap();
    let a = write_request(&root.join("a.yaml"), "Alpha");
    let b = write_request(&root.join("b.yaml"), "Beta");
    let _c = write_request(&root.join("c.yaml"), "charlie");
    std::fs::create_dir(root.join("zfolder")).unwrap();

    // `order` lists Beta and an ID that no longer exists (e.g. deleted in a merge).
    let mut collection: ironcall_core::model::CollectionFile =
        yaml::from_str(&std::fs::read_to_string(root.join(COLLECTION_FILE)).unwrap()).unwrap();
    collection.order = vec![b.id, ItemId::new(), a.id];
    std::fs::write(root.join(COLLECTION_FILE), yaml::to_string(&collection).unwrap()).unwrap();

    let ws = Workspace::open(&root).unwrap();
    // Listed first in order (missing skipped), then unlisted: folders first, then by name.
    assert_eq!(names(ws.tree()), ["Beta", "Alpha", "zfolder", "charlie"]);
}

#[test]
fn files_added_by_a_merge_appear_unlisted() {
    let (_tmp, mut ws) = new_workspace();
    let root_id = ws.tree().id;
    let first = ws.create_request(root_id, "First", Some(0)).unwrap().node;
    let second = ws.create_request(root_id, "Second", Some(0)).unwrap().node;
    assert_eq!(collection_order(&ws), [second.id, first.id]);

    write_request(&ws.root().join("merged.yaml"), "Merged in");
    ws.rescan(&[]).unwrap();
    assert_eq!(names(ws.tree()), ["Second", "First", "Merged in"]);
}

#[test]
fn duplicate_ids_are_reassigned_on_the_newer_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    Workspace::create(&root, "W").unwrap();
    let original = write_request(&root.join("original.yaml"), "Original");
    std::thread::sleep(Duration::from_millis(30));
    // Named the way Windows Explorer names a copy. Windows keeps the modified time on a
    // copy (the creation time is new); the name length breaks a tie if times are equal.
    std::fs::copy(root.join("original.yaml"), root.join("original - Copy.yaml")).unwrap();

    let mut ws = Workspace::open(&root).unwrap();
    let reassigned = ws.take_reassigned();
    assert_eq!(reassigned.len(), 1);
    assert_eq!(reassigned[0].path, "original - Copy.yaml");
    assert_eq!(reassigned[0].old, original.id);

    let kept: RequestFile =
        yaml::from_str(&std::fs::read_to_string(root.join("original.yaml")).unwrap()).unwrap();
    let copy: RequestFile =
        yaml::from_str(&std::fs::read_to_string(root.join("original - Copy.yaml")).unwrap()).unwrap();
    assert_eq!(kept.id, original.id);
    assert_eq!(copy.id, reassigned[0].new);
    assert_eq!(ws.tree().request_count(), 2);
}

#[test]
fn merge_conflict_file_is_an_error_node_and_never_touched() {
    let (tmp, mut ws) = new_workspace();
    let root_id = ws.tree().id;
    let folder = ws.create_folder(root_id, "Users", None).unwrap().node;
    let conflicted = ws.root().join("users/broken.yaml");
    std::fs::write(&conflicted, CONFLICTED).unwrap();
    ws.rescan(&[]).unwrap();

    let error = ws.tree().find(folder.id).unwrap().children[0].clone();
    assert_eq!(error.kind, NodeKind::Error);
    assert!(
        error.error.as_deref().unwrap().contains("merge conflict"),
        "{error:?}"
    );
    assert!(ws.load_request(error.id).is_err());
    assert!(ws.move_item(error.id, root_id, 0).is_err());

    // Every other operation leaves it byte-for-byte alone.
    let other = ws.create_request(folder.id, "Other", Some(0)).unwrap().node;
    ws.rename(other.id, "Other renamed").unwrap();
    ws.move_item(other.id, root_id, 0).unwrap();
    ws.duplicate(folder.id).unwrap();
    ws.rename(folder.id, "People").unwrap();
    ws.delete(other.id).unwrap();
    let ws = reopen(&ws, tmp.path());
    assert_eq!(std::fs::read_to_string(&conflicted).unwrap(), CONFLICTED);
    assert!(ws.tree().children.iter().any(|f| f.name == "People"));
}

#[test]
fn move_and_reorder_across_folders() {
    let (_tmp, mut ws) = new_workspace();
    let root_id = ws.tree().id;
    let a = ws.create_folder(root_id, "A", None).unwrap().node;
    let b = ws.create_folder(root_id, "B", None).unwrap().node;
    let r1 = ws.create_request(a.id, "One", None).unwrap().node;
    let r2 = ws.create_request(a.id, "Two", None).unwrap().node;
    std::fs::write(ws.root().join("b/one.yaml"), "x").unwrap(); // forces a slug collision

    // Reorder within A.
    ws.move_item(r2.id, a.id, 0).unwrap();
    assert_eq!(names(ws.tree().find(a.id).unwrap()), ["Two", "One"]);

    // Move to B: file moves, collision resolved, both orders updated.
    ws.move_item(r1.id, b.id, 0).unwrap();
    let moved = ws.tree().find(r1.id).unwrap();
    assert_eq!(moved.path, "b/one-2.yaml");
    assert_eq!(names(ws.tree().find(a.id).unwrap()), ["Two"]);
    let folder_b = ws.load_folder(b.id).unwrap().file;
    assert_eq!(folder_b.order.first(), Some(&r1.id));

    // Folders move with their contents; never into themselves.
    ws.move_item(a.id, b.id, 1).unwrap();
    assert_eq!(ws.tree().find(r2.id).unwrap().path, "b/a/two.yaml");
    assert!(ws.move_item(b.id, a.id, 0).is_err());
}

#[test]
fn hand_made_folders_keep_session_ids_and_persist_when_needed() {
    let (tmp, mut ws) = new_workspace();
    write_request(&ws.root().join("manual/req.yaml"), "Req");
    ws.rescan(&[]).unwrap();
    let folder = ws.tree().children[0].clone();
    assert_eq!(folder.name, "manual");
    assert!(
        !ws.root().join("manual").join(FOLDER_FILE).exists(),
        "not written on scan"
    );

    ws.rescan(&[]).unwrap();
    assert_eq!(
        ws.tree().children[0].id,
        folder.id,
        "session ID is stable across rescans"
    );

    ws.rename(folder.id, "Manual folder").unwrap();
    assert!(ws.root().join("manual").join(FOLDER_FILE).exists());
    let reopened = reopen(&ws, tmp.path());
    assert_eq!(
        reopened.tree().children[0].id,
        folder.id,
        "ID persisted on first write"
    );
}

#[test]
fn duplicate_places_copy_after_original_with_new_ids() {
    let (_tmp, mut ws) = new_workspace();
    let root_id = ws.tree().id;
    let folder = ws.create_folder(root_id, "Users", None).unwrap().node;
    let req = ws.create_request(folder.id, "Get", None).unwrap().node;
    let other = ws.create_request(root_id, "Zed", None).unwrap().node;

    let copy = ws.duplicate(req.id).unwrap().node;
    assert_ne!(copy.id, req.id);
    assert_eq!(names(ws.tree().find(folder.id).unwrap()), ["Get", "Get copy"]);

    let folder_copy = ws.duplicate(folder.id).unwrap().node;
    assert_eq!(names(ws.tree()), ["Users", "Users copy", "Zed"]);
    let copied = ws.tree().find(folder_copy.id).unwrap();
    assert_eq!(copied.children.len(), 2);
    for child in &copied.children {
        assert!(child.id != req.id && child.id != copy.id, "new IDs in the copy");
    }
    assert!(ws.tree().find(other.id).is_some());
}

#[test]
fn delete_moves_to_trash_and_tidies_order() {
    let (tmp, mut ws) = new_workspace();
    let root_id = ws.tree().id;
    let a = ws.create_request(root_id, "A", Some(0)).unwrap().node;
    let b = ws.create_request(root_id, "B", Some(1)).unwrap().node;
    ws.delete(a.id).unwrap();
    assert_eq!(collection_order(&ws), [b.id]);
    assert!(tmp.path().join("trash/a.yaml").exists());
    assert!(ws.tree().find(a.id).is_none());
}

#[test]
fn newer_schema_is_read_only() {
    let (_tmp, mut ws) = new_workspace();
    let req = write_request(&ws.root().join("future.yaml"), "Future");
    let text = std::fs::read_to_string(ws.root().join("future.yaml"))
        .unwrap()
        .replace("ironcall/request@1", "ironcall/request@2");
    std::fs::write(ws.root().join("future.yaml"), &text).unwrap();
    ws.rescan(&[]).unwrap();
    assert!(ws.tree().find(req.id).unwrap().read_only);
    let loaded = ws.load_request(req.id).unwrap();
    assert!(loaded.read_only);
    assert!(matches!(
        ws.save_request(&loaded.file, loaded.hash.as_deref(), false),
        Err(WorkspaceError::ReadOnly(_))
    ));
    assert_eq!(
        std::fs::read_to_string(ws.root().join("future.yaml")).unwrap(),
        text
    );
}

#[test]
fn environments_dot_dirs_and_other_files_are_not_requests() {
    let (_tmp, mut ws) = new_workspace();
    let root = ws.root().to_path_buf();
    std::fs::create_dir_all(root.join("environments")).unwrap();
    std::fs::write(
        root.join("environments/dev.yaml"),
        "schema: 'ironcall/environment@1'\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/x.yaml"), "x").unwrap();
    std::fs::write(root.join("README.md"), "# hi").unwrap();
    ws.rescan(&[]).unwrap();
    assert!(ws.tree().children.is_empty(), "{:?}", ws.tree());
}

#[test]
fn resaving_unchanged_canonical_requests_is_byte_identical() {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/yaml/canonical");
    let mut checked = 0;
    for entry in std::fs::read_dir(&fixtures).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.starts_with("request-") {
            continue;
        }
        // One workspace per fixture: fixtures may share IDs.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("ws");
        Workspace::create(&root, "W").unwrap();
        let original = std::fs::read(&path).unwrap();
        std::fs::write(root.join(&name), &original).unwrap();

        let mut ws = Workspace::open(&root).unwrap();
        let id = ws.tree().children[0].id;
        let loaded = ws.load_request(id).unwrap();
        let outcome = ws
            .save_request(&loaded.file, loaded.hash.as_deref(), false)
            .unwrap();
        assert!(matches!(outcome, SaveOutcome::Saved { .. }), "{name}");
        assert_eq!(std::fs::read(root.join(&name)).unwrap(), original, "{name}");
        checked += 1;
    }
    assert!(checked >= 10, "only {checked} fixtures");
}

#[test]
fn edit_then_external_change_then_conflict_loses_nothing() {
    let (_tmp, mut ws) = new_workspace();
    let root_id = ws.tree().id;
    let node = ws.create_request(root_id, "Shared", None).unwrap().node;
    let path = ws.root().join(&node.path);

    let (tx, rx) = mpsc::channel();
    let _watcher = WorkspaceWatcher::start(ws.root(), ws.own_writes(), move |paths| {
        let _ = tx.send(paths);
    })
    .unwrap();

    // The user edits in IronCall (unsaved) while a `git pull` changes the file.
    let loaded = ws.load_request(node.id).unwrap();
    let mut mine = loaded.file.clone();
    mine.url = "https://mine.example".into();
    let mut theirs = loaded.file.clone();
    theirs.url = "https://theirs.example".into();
    let theirs_text = yaml::to_string(&theirs).unwrap();
    std::fs::write(&path, &theirs_text).unwrap();

    let paths = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("watcher reports the change");
    let event = ws.rescan(&paths).unwrap();
    assert_eq!(event.changed, [node.id]);

    // Saving with the old base hash is refused and hands back their version.
    match ws.save_request(&mine, loaded.hash.as_deref(), false).unwrap() {
        SaveOutcome::Conflict { disk: Some(disk) } => assert_eq!(disk.file.url, "https://theirs.example"),
        other => panic!("expected a conflict, got {other:?}"),
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        theirs_text,
        "nothing overwritten"
    );

    // "Keep mine" saves with force; both versions were available to the user.
    ws.save_request(&mine, None, true).unwrap();
    assert!(std::fs::read_to_string(&path).unwrap().contains("mine.example"));
}

#[test]
fn watcher_ignores_own_writes() {
    let (_tmp, mut ws) = new_workspace();
    let root_id = ws.tree().id;
    let node = ws.create_request(root_id, "Mine", None).unwrap().node;

    let (tx, rx) = mpsc::channel();
    let _watcher = WorkspaceWatcher::start(ws.root(), ws.own_writes(), move |paths| {
        let _ = tx.send(paths);
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let mut loaded = ws.load_request(node.id).unwrap();
    loaded.file.url = "https://changed.example".into();
    ws.save_request(&loaded.file, loaded.hash.as_deref(), false)
        .unwrap();
    ws.create_request(root_id, "Another", Some(0)).unwrap();
    ws.rename(node.id, "Renamed").unwrap();

    assert!(
        rx.recv_timeout(Duration::from_millis(1200)).is_err(),
        "own writes must not be reported"
    );

    std::fs::write(ws.root().join("external.yaml"), "x").unwrap();
    let paths = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("external write is reported");
    assert!(paths.iter().any(|p| p.ends_with("external.yaml")), "{paths:?}");
}

#[test]
fn tree_build_for_2000_requests_in_50_folders() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    Workspace::create(&root, "Big").unwrap();
    for f in 0..50 {
        for r in 0..40 {
            write_request(
                &root.join(format!("folder-{f:02}/req-{r:02}.yaml")),
                &format!("Request {f}-{r}"),
            );
        }
    }

    let start = Instant::now();
    let mut ws = Workspace::open(&root).unwrap();
    let cold = start.elapsed();
    assert_eq!(ws.tree().request_count(), 2000);

    let start = Instant::now();
    ws.rescan(&[]).unwrap();
    let warm = start.elapsed();
    eprintln!("2,000 requests: open {cold:?}, rescan {warm:?}");

    // Release builds meet the 100ms budget; debug builds are much slower, so only
    // guard against pathological regressions there.
    let budget = if cfg!(debug_assertions) {
        Duration::from_secs(5)
    } else {
        Duration::from_millis(100)
    };
    assert!(cold < budget, "open took {cold:?}");
    assert!(warm < budget, "rescan took {warm:?}");
}

#[cfg(unix)]
#[test]
fn symlinks_are_skipped_and_never_rewrite_ids() {
    let (_tmp, mut ws) = new_workspace();
    let root_id = ws.tree().id;
    let folder = ws.create_folder(root_id, "Shared", None).unwrap().node.id;
    let req = ws.create_request(folder, "One", None).unwrap().node.id;
    let shared = ws.path_of(folder).unwrap().to_path_buf();
    std::os::unix::fs::symlink(&shared, ws.root().join("alias")).unwrap();
    std::os::unix::fs::symlink(ws.root(), ws.root().join("loop")).unwrap();
    std::os::unix::fs::symlink(ws.root(), ws.root().join("loop2")).unwrap();
    let before = std::fs::read(ws.path_of(req).unwrap()).unwrap();

    let event = ws.rescan(&[]).unwrap();
    assert!(event.reassigned.is_empty(), "{:?}", event.reassigned);
    assert_eq!(ws.tree().request_count(), 1);
    assert_eq!(std::fs::read(ws.path_of(req).unwrap()).unwrap(), before);
    // Reopening (which used to loop forever with two `-> .` links) works too.
    let reopened = Workspace::open(ws.root()).unwrap();
    assert_eq!(reopened.tree().request_count(), 1);
}

#[cfg(unix)]
#[test]
fn an_unreadable_folder_is_an_error_node_and_the_rest_still_works() {
    use std::os::unix::fs::PermissionsExt;
    if running_as_root() {
        return; // root can read anything; the scenario can't be set up
    }
    let (_tmp, mut ws) = new_workspace();
    let root_id = ws.tree().id;
    let req = ws.create_request(root_id, "Health", None).unwrap().node.id;
    let locked = ws.root().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

    let result = ws.rescan(&[]);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    result.unwrap();
    assert!(ws.load_request(req).is_ok(), "other requests still load");
    let node = ws.tree().children.iter().find(|n| n.name == "locked").unwrap();
    assert!(node.error.as_deref().unwrap().contains("can't read this folder"));
}

/// True when the tests run as root (permission tests are meaningless then).
#[cfg(unix)]
fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .map(|s| {
            s.lines()
                .any(|l| l.starts_with("Uid:") && l.split_whitespace().nth(1) == Some("0"))
        })
        .unwrap_or(false)
}
