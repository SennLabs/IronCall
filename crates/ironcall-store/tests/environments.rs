//! Environments, local settings and variable helpers (PLAN.md Phase 3).

use std::sync::mpsc;
use std::time::Duration;

use ironcall_core::ItemId;
use ironcall_core::model::{EnvironmentFile, Extra, Variable, VariableSpec, Vars, schema};
use ironcall_store::watch::WorkspaceWatcher;
use ironcall_store::workspace::{
    DirTrash, FOLDER_FILE, LOCAL_FILE, LocalSettings, SaveOutcome, SecretScopeRef, Workspace,
    declared_secrets,
};
use ironcall_store::yaml;

fn new_workspace() -> (tempfile::TempDir, Workspace) {
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::create(&tmp.path().join("ws"), "My API")
        .unwrap()
        .with_trash(Box::new(DirTrash(tmp.path().join("trash"))));
    (tmp, ws)
}

fn secret() -> Variable {
    Variable::Detailed(VariableSpec {
        secret: true,
        ..VariableSpec::default()
    })
}

fn names(ws: &Workspace) -> Vec<String> {
    ws.environments().into_iter().map(|e| e.name).collect()
}

#[test]
fn environment_crud_keeps_file_names_and_sorts_by_name() {
    let (tmp, mut ws) = new_workspace();
    let prod = ws.create_environment("Prod").unwrap();
    let dev = ws.create_environment("Dev").unwrap();
    assert_eq!(dev.path, "environments/dev.yaml");
    assert_eq!(names(&ws), ["Dev", "Prod"]);

    // Rename changes the display name only.
    ws.rename_environment(dev.id, "Development").unwrap();
    assert_eq!(names(&ws), ["Development", "Prod"]);
    assert!(ws.root().join("environments/dev.yaml").is_file());

    let copy = ws.duplicate_environment(prod.id).unwrap();
    assert_ne!(copy.id, prod.id);
    assert_eq!(copy.name, "Prod copy");
    assert_eq!(copy.path, "environments/prod-copy.yaml");

    ws.delete_environment(copy.id).unwrap();
    assert_eq!(names(&ws), ["Development", "Prod"]);
    assert!(tmp.path().join("trash/prod-copy.yaml").is_file());

    // A reopened workspace sees the same IDs.
    let reopened = Workspace::open(ws.root()).unwrap();
    let ids: Vec<ItemId> = reopened.environments().iter().map(|e| e.id).collect();
    assert_eq!(ids, [dev.id, prod.id]);
}

#[test]
fn save_environment_detects_conflicts() {
    let (_tmp, mut ws) = new_workspace();
    let dev = ws.create_environment("Dev").unwrap();
    let loaded = ws.load_environment(dev.id).unwrap();

    let mut theirs = loaded.file.clone();
    theirs
        .vars
        .insert("baseUrl".into(), Variable::Plain("https://theirs".into()));
    std::fs::write(
        ws.environment_path(dev.id).unwrap(),
        yaml::to_string(&theirs).unwrap(),
    )
    .unwrap();

    let mut mine = loaded.file.clone();
    mine.vars
        .insert("baseUrl".into(), Variable::Plain("https://mine".into()));
    match ws.save_environment(&mine, loaded.hash.as_deref(), false).unwrap() {
        SaveOutcome::Conflict { disk: Some(disk) } => assert_eq!(disk.file, theirs),
        other => panic!("expected a conflict, got {other:?}"),
    }
    // Force (keep mine) writes.
    assert!(matches!(
        ws.save_environment(&mine, loaded.hash.as_deref(), true).unwrap(),
        SaveOutcome::Saved { .. }
    ));
    assert_eq!(ws.environment_vars(dev.id).unwrap(), mine.vars);
}

#[test]
fn secret_values_are_never_written() {
    let (_tmp, mut ws) = new_workspace();
    let dev = ws.create_environment("Dev").unwrap();
    let before = std::fs::read(ws.environment_path(dev.id).unwrap()).unwrap();

    let mut file = ws.load_environment(dev.id).unwrap().file;
    file.vars.insert(
        "apiKey".into(),
        Variable::Detailed(VariableSpec {
            value: Some("hunter2".into()),
            secret: true,
            ..VariableSpec::default()
        }),
    );
    let err = ws.save_environment(&file, None, true).unwrap_err();
    assert!(err.to_string().contains("apiKey"), "{err}");
    assert_eq!(
        std::fs::read(ws.environment_path(dev.id).unwrap()).unwrap(),
        before
    );

    // Declaring the secret (no value) is fine, and the same rule guards collections.
    file.vars.insert("apiKey".into(), secret());
    ws.save_environment(&file, None, true).unwrap();
    let mut collection = ws.load_collection().unwrap().file;
    collection.vars.insert(
        "token".into(),
        Variable::Detailed(VariableSpec {
            value: Some("x".into()),
            secret: true,
            ..VariableSpec::default()
        }),
    );
    assert!(ws.save_collection(&collection, None, true).is_err());
}

#[test]
fn broken_environment_files_are_listed_with_an_error_and_left_alone() {
    let (_tmp, mut ws) = new_workspace();
    ws.create_environment("Dev").unwrap();
    let broken = ws.root().join("environments/broken.yaml");
    let text = "schema: 'ironcall/environment@1'\n<<<<<<< HEAD\nname: 'a'\n=======\nname: 'b'\n>>>>>>> x\n";
    std::fs::write(&broken, text).unwrap();
    ws.rescan(&[]).unwrap();

    let envs = ws.environments();
    let bad = envs
        .iter()
        .find(|e| e.path == "environments/broken.yaml")
        .unwrap();
    assert!(bad.error.is_some());
    assert!(ws.load_environment(bad.id).is_err());

    // The session ID is stable across rescans, and nothing rewrites the file.
    let id = bad.id;
    ws.rescan(&[]).unwrap();
    assert!(ws.environments().iter().any(|e| e.id == id));
    assert_eq!(std::fs::read_to_string(&broken).unwrap(), text);
}

#[test]
fn copied_environment_file_gets_a_new_id() {
    let (_tmp, mut ws) = new_workspace();
    let dev = ws.create_environment("Dev").unwrap();
    std::thread::sleep(Duration::from_millis(20));
    std::fs::copy(
        ws.environment_path(dev.id).unwrap(),
        ws.root().join("environments/dev-copy.yaml"),
    )
    .unwrap();
    let event = ws.rescan(&[]).unwrap();
    assert_eq!(event.reassigned.len(), 1);
    assert_eq!(event.reassigned[0].path, "environments/dev-copy.yaml");
    let ids: Vec<ItemId> = ws.environments().iter().map(|e| e.id).collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
    assert!(ids.contains(&dev.id), "the original keeps its ID");
}

#[test]
fn watcher_reports_environment_edits_without_tree_changes() {
    let (_tmp, mut ws) = new_workspace();
    let dev = ws.create_environment("Dev").unwrap();
    let path = ws.environment_path(dev.id).unwrap().to_path_buf();

    let (tx, rx) = mpsc::channel();
    let _watcher = WorkspaceWatcher::start(ws.root(), ws.own_writes(), move |paths| {
        let _ = tx.send(paths);
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(100));

    let mut file = ws.load_environment(dev.id).unwrap().file;
    file.vars
        .insert("baseUrl".into(), Variable::Plain("https://external".into()));
    std::fs::write(&path, yaml::to_string(&file).unwrap()).unwrap();

    let paths = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("watcher reports the change");
    let event = ws.rescan(&paths).unwrap();
    assert!(event.environments_changed);
    assert!(!event.tree_changed);
    assert!(event.changed.is_empty());
    assert!(!event.is_empty());
}

#[test]
fn environments_stay_out_of_the_request_tree() {
    let (_tmp, mut ws) = new_workspace();
    ws.create_environment("Dev").unwrap();
    assert!(ws.tree().children.is_empty());
}

#[test]
fn local_settings_round_trip_and_drop_a_stale_environment() {
    let (_tmp, mut ws) = new_workspace();
    let dev = ws.create_environment("Dev").unwrap();
    assert_eq!(ws.load_local(), LocalSettings::default());

    // An older workspace without `.ironcall/` gets it (and its .gitignore) on save.
    std::fs::remove_dir_all(ws.root().join(".ironcall")).unwrap();
    let settings = LocalSettings {
        active_environment: Some(dev.id),
        secret_backend: Some("encryptedFile".into()),
        ca_certs: vec!["/etc/ssl/corp-ca.pem".into()],
        history_bodies: Some(false),
        extra: Extra::default(),
    };
    ws.save_local(&settings).unwrap();
    assert_eq!(
        std::fs::read_to_string(ws.root().join(".ironcall/.gitignore")).unwrap(),
        "*\n"
    );
    assert!(ws.root().join(".ironcall").join(LOCAL_FILE).is_file());
    assert_eq!(ws.load_local(), settings);

    ws.delete_environment(dev.id).unwrap();
    let after = ws.load_local();
    assert_eq!(after.active_environment, None);
    assert_eq!(after.secret_backend.as_deref(), Some("encryptedFile"));
}

#[test]
fn declared_secrets_cover_collection_environments_and_folders() {
    let (_tmp, mut ws) = new_workspace();
    let root = ws.tree().id;

    let mut collection = ws.load_collection().unwrap();
    collection.file.vars.insert("token".into(), secret());
    collection
        .file
        .vars
        .insert("plain".into(), Variable::Plain("x".into()));
    ws.save_collection(&collection.file, collection.hash.as_deref(), false)
        .unwrap();

    let dev = ws.create_environment("Dev").unwrap();
    let mut env = ws.load_environment(dev.id).unwrap();
    env.file.vars.insert("apiKey".into(), secret());
    ws.save_environment(&env.file, env.hash.as_deref(), false)
        .unwrap();

    let folder = ws.create_folder(root, "Admin", None).unwrap().node;
    let mut f = ws.load_folder(folder.id).unwrap();
    f.file.vars.insert("adminPassword".into(), secret());
    ws.save_folder(&f.file, f.hash.as_deref(), false).unwrap();

    let mut found = declared_secrets(&ws);
    found.sort();
    let mut expected = vec![
        (SecretScopeRef::Collection, "token".to_string()),
        (SecretScopeRef::Environment(dev.id), "apiKey".to_string()),
        (SecretScopeRef::Folder(folder.id), "adminPassword".to_string()),
    ];
    expected.sort();
    assert_eq!(found, expected);
}

#[test]
fn folder_chain_is_outermost_first_with_vars() {
    let (_tmp, mut ws) = new_workspace();
    let root = ws.tree().id;
    let outer = ws.create_folder(root, "Outer", None).unwrap().node;
    let inner = ws.create_folder(outer.id, "Inner", None).unwrap().node;
    let req = ws.create_request(inner.id, "Get", None).unwrap().node;

    let mut f = ws.load_folder(outer.id).unwrap();
    f.file
        .vars
        .insert("level".into(), Variable::Plain("outer".into()));
    ws.save_folder(&f.file, f.hash.as_deref(), false).unwrap();

    // A hand-made folder (no _folder.yaml) in the chain contributes no vars.
    let chain = ws.folder_chain(req.id).unwrap();
    let names: Vec<&str> = chain.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Outer", "Inner"]);
    assert_eq!(chain[0].id, outer.id);
    assert_eq!(chain[0].vars.get("level"), Some(&Variable::Plain("outer".into())));
    assert!(chain[1].vars.is_empty());

    // Top-level requests have an empty chain; the collection vars are separate.
    let top = ws.create_request(root, "Top", None).unwrap().node;
    assert!(ws.folder_chain(top.id).unwrap().is_empty());
    assert!(ws.collection_vars().is_empty());

    // A folder with an invalid _folder.yaml still appears in the chain, without vars.
    std::fs::write(ws.path_of(outer.id).unwrap().join(FOLDER_FILE), "not: [valid").unwrap();
    ws.rescan(&[]).unwrap();
    let req_id = ws
        .tree()
        .children
        .iter()
        .flat_map(|c| c.children.iter())
        .flat_map(|c| c.children.iter())
        .find(|c| c.name == "Get")
        .map(|c| c.id)
        .unwrap();
    let chain = ws.folder_chain(req_id).unwrap();
    assert_eq!(chain.len(), 2);
    assert!(chain[0].vars.is_empty());
}

#[test]
fn hand_written_environment_file_is_read() {
    let (_tmp, mut ws) = new_workspace();
    let dir = ws.root().join("environments");
    std::fs::create_dir_all(&dir).unwrap();
    let mut vars = Vars::default();
    vars.insert("baseUrl".into(), Variable::Plain("https://dev".into()));
    vars.insert("apiKey".into(), secret());
    let file = EnvironmentFile {
        schema: schema::ENVIRONMENT.into(),
        id: ItemId::new(),
        name: "Staging".into(),
        vars: vars.clone(),
        extra: Extra::default(),
    };
    std::fs::write(dir.join("stage.yaml"), yaml::to_string(&file).unwrap()).unwrap();
    let event = ws.rescan(&[]).unwrap();
    assert!(event.environments_changed);
    assert_eq!(names(&ws), ["Staging"]);
    assert_eq!(ws.environment_vars(file.id).unwrap(), vars);
}
