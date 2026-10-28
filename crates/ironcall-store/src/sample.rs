//! The first-run sample workspace (PLAN.md Phase 8): requests against IronCall's
//! built-in echo server (`{{$localEcho}}`), so every example works offline. It shows
//! environments, JSON bodies, JSONPath assertions, chaining a token, and the runner.

use std::path::Path;

use ironcall_core::model::{
    AssertOp, AssertTarget, Assertion, Auth, Body, ExtractSource, Extraction, KeyValue, RequestFile, Variable,
};
use serde_json::{Value, json};

use crate::workspace::{Result, SaveOutcome, Workspace, WorkspaceError};

const WELCOME: &str = "\
# Welcome to IronCall

This sample workspace calls IronCall's **built-in echo server**, so it works offline. \
The *Local echo* environment sets `baseUrl` to `{{$localEcho}}`.

- **Send** a request with Ctrl+Enter, and see assertions in the response's **Tests** tab.
- **Run** the whole workspace from the toolbar: requests run in order, and *Get a token* \
passes its token to *Call with the token*.
- **Find** any request with Ctrl+P. **Paste a cURL command** into the URL bar to import it.

Everything here is plain YAML in this folder, ready to commit to git.
";

fn assertion(target: AssertTarget, path: Option<&str>, op: AssertOp, value: Option<Value>) -> Assertion {
    Assertion {
        target,
        path: path.map(str::to_string),
        name: None,
        op,
        value,
        enabled: true,
        extra: Default::default(),
    }
}

fn status(code: u16) -> Assertion {
    assertion(AssertTarget::Status, None, AssertOp::Eq, Some(json!(code)))
}

fn request(name: &str, method: &str, url: &str, description: &str) -> RequestFile {
    let mut r = RequestFile::new(name);
    r.method = method.into();
    r.url = url.into();
    r.description = Some(description.into());
    r
}

/// The sample's folders and requests, in order.
fn contents() -> Vec<(&'static str, Vec<RequestFile>)> {
    let mut get = request(
        "Echo a GET",
        "GET",
        "{{baseUrl}}/anything/hello?lang=en&page=1",
        "The echo server returns what it received. The assertions check the status and a query value.",
    );
    get.assert = vec![
        status(200),
        assertion(
            AssertTarget::Body,
            Some("$.query.lang"),
            AssertOp::Eq,
            Some(json!("en")),
        ),
    ];

    let mut post = request(
        "Send JSON",
        "POST",
        "{{baseUrl}}/anything/users",
        "A JSON body with a dynamic helper. `{{$randomEmail}}` is different on every send.",
    );
    post.headers = vec![KeyValue::new("Content-Type", "application/json")];
    post.body = Body::Json {
        content: "{\n  \"name\": \"Ada\",\n  \"email\": \"{{$randomEmail}}\"\n}\n".into(),
        extra: Default::default(),
    };
    post.assert = vec![
        status(200),
        assertion(
            AssertTarget::Body,
            Some("$.json.name"),
            AssertOp::Eq,
            Some(json!("Ada")),
        ),
        assertion(
            AssertTarget::Body,
            Some("$.json.email"),
            AssertOp::Matches,
            Some(json!("@example\\.com$")),
        ),
    ];

    let mut doc = request(
        "Query with JSONPath",
        "GET",
        "{{baseUrl}}/json",
        "JSONPath assertions on a sample document: a filter, a type check and a list.",
    );
    doc.assert = vec![
        assertion(
            AssertTarget::Body,
            Some("$.store.books[?@.price > 50].title"),
            AssertOp::Eq,
            Some(json!("HTTP: The Definitive Guide")),
        ),
        assertion(
            AssertTarget::Body,
            Some("$.store.open"),
            AssertOp::Type,
            Some(json!("boolean")),
        ),
        assertion(
            AssertTarget::Body,
            Some("$.store.books[*].id"),
            AssertOp::Contains,
            Some(json!(2)),
        ),
    ];

    let mut token = request(
        "Get a token",
        "POST",
        "{{baseUrl}}/token",
        "Extracts `access_token` into the runtime variable `authToken`. Token-like values are always sensitive: masked in the UI, history and logs.",
    );
    token.assert = vec![status(200)];
    token.extract = vec![Extraction {
        var: "authToken".into(),
        from: ExtractSource::Body,
        path: Some("$.access_token".into()),
        name: None,
        pattern: None,
        sensitive: true,
        extra: Default::default(),
    }];

    let mut bearer = request(
        "Call with the token",
        "GET",
        "{{baseUrl}}/bearer",
        "Uses `{{authToken}}` from *Get a token*. Send that one first, or run the folder.",
    );
    bearer.auth = Auth::Bearer {
        token: "{{authToken}}".into(),
        extra: Default::default(),
    };
    bearer.assert = vec![
        status(200),
        assertion(
            AssertTarget::Body,
            Some("$.authenticated"),
            AssertOp::Eq,
            Some(json!(true)),
        ),
    ];

    let mut not_found = request(
        "Expect a 404",
        "GET",
        "{{baseUrl}}/status/404",
        "Assertions can expect errors too.",
    );
    not_found.assert = vec![status(404)];

    let mut slow = request(
        "Slow response",
        "GET",
        "{{baseUrl}}/delay/300",
        "Waits 300 ms. See the Timing tab, and the duration assertion.",
    );
    slow.assert = vec![assertion(
        AssertTarget::Duration,
        None,
        AssertOp::Lt,
        Some(json!(2000)),
    )];

    vec![
        ("Basics", vec![get, post, doc]),
        ("Chaining", vec![token, bearer]),
        ("Status and timing", vec![not_found, slow]),
    ]
}

/// Creates the sample workspace in `root` (an empty or new folder) and returns it open.
pub fn create(root: &Path) -> Result<Workspace> {
    let mut ws = Workspace::create(root, "IronCall sample")?;

    let loaded = ws.load_collection()?;
    let mut collection = loaded.file;
    collection.description = Some(WELCOME.into());
    if let SaveOutcome::Conflict { .. } = ws.save_collection(&collection, loaded.hash.as_deref(), false)? {
        return Err(WorkspaceError::Invalid(
            "the sample changed while it was being created".into(),
        ));
    }

    let env = ws.create_environment("Local echo")?;
    let mut file = ws.load_environment(env.id)?;
    file.file
        .vars
        .insert("baseUrl".into(), Variable::Plain("{{$localEcho}}".into()));
    ws.save_environment(&file.file, file.hash.as_deref(), false)?;
    let mut local = ws.load_local();
    local.active_environment = Some(env.id);
    ws.save_local(&local)?;

    let root_id = ws.tree().id;
    for (index, (folder, requests)) in contents().into_iter().enumerate() {
        let folder_id = ws.create_folder(root_id, folder, Some(index))?.node.id;
        for (i, mut r) in requests.into_iter().enumerate() {
            let created = ws.create_request(folder_id, &r.name, Some(i))?;
            let loaded = ws.load_request(created.node.id)?;
            r.id = created.node.id;
            ws.save_request(&r, loaded.hash.as_deref(), false)?;
        }
    }
    Ok(ws)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::NodeKind;

    #[test]
    fn sample_has_folders_requests_and_an_active_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = create(tmp.path()).unwrap();
        let folders: Vec<_> = ws
            .tree()
            .children
            .iter()
            .map(|n| (n.kind, n.name.clone()))
            .collect();
        assert_eq!(
            folders,
            [
                (NodeKind::Folder, "Basics".to_string()),
                (NodeKind::Folder, "Chaining".to_string()),
                (NodeKind::Folder, "Status and timing".to_string()),
            ]
        );
        assert_eq!(ws.tree().request_count(), 7);
        let chaining = &ws.tree().children[1];
        let names: Vec<_> = chaining.children.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, ["Get a token", "Call with the token"], "in run order");

        let env = ws.load_local().active_environment.expect("active environment");
        let vars = ws.environment_vars(env).unwrap();
        assert!(matches!(vars.get("baseUrl"), Some(Variable::Plain(v)) if v == "{{$localEcho}}"));
        assert!(
            ws.load_collection()
                .unwrap()
                .file
                .description
                .unwrap()
                .contains("echo server")
        );

        // Everything round-trips: reopening reads the same files without errors.
        let reopened = Workspace::open(tmp.path()).unwrap();
        assert_eq!(reopened.tree().request_count(), 7);
        let token = &reopened.tree().children[1].children[0];
        let file = reopened.load_request(token.id).unwrap().file;
        assert_eq!(file.extract[0].var, "authToken");
    }

    #[test]
    fn refuses_a_folder_that_is_already_a_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        create(tmp.path()).unwrap();
        assert!(create(tmp.path()).is_err());
    }
}
