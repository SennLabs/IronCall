//! OpenAPI/Swagger import: behaviour checks plus a snapshot of every fixture's plan.

use std::path::PathBuf;

use ironcall_core::ItemId;
use ironcall_core::model::{ApiKeyLocation, Auth, Body, MultipartKind, RequestFile};
use ironcall_import::openapi::{ImportError, Plan, PlanItem, SpecVersion, plan_with_ids};

fn fixtures() -> PathBuf {
    std::env::var_os("IRONCALL_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures"))
        .join("openapi")
}

/// Plans a fixture with deterministic IDs, so snapshots are stable.
fn plan(name: &str) -> Plan {
    let bytes = std::fs::read(fixtures().join(name)).unwrap();
    let mut n = 0u32;
    plan_with_ids(&bytes, &mut || {
        n += 1;
        format!("01J{n:023}").parse::<ItemId>().unwrap()
    })
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

fn requests(items: &[PlanItem]) -> Vec<&RequestFile> {
    let mut out = Vec::new();
    for item in items {
        match item {
            PlanItem::Folder { items, .. } => out.extend(requests(items)),
            PlanItem::Request(r) => out.push(r),
        }
    }
    out
}

fn by_name<'a>(plan: &'a Plan, name: &str) -> &'a RequestFile {
    requests(&plan.tree)
        .into_iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("no request named {name}"))
}

fn env<'a>(plan: &'a Plan, name: &str) -> &'a ironcall_import::openapi::EnvVar {
    plan.environment
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no env var {name}"))
}

fn has_warning(plan: &Plan, needle: &str) -> bool {
    plan.warnings.iter().any(|w| w.message.contains(needle))
}

#[test]
fn petstore_in_every_version() {
    for (file, version) in [
        ("petstore-v2.json", SpecVersion::Swagger2),
        ("petstore-v3.0.yaml", SpecVersion::OpenApi30),
        ("petstore-v3.1.yaml", SpecVersion::OpenApi31),
    ] {
        let plan = plan(file);
        assert_eq!(plan.version, version, "{file}");
        assert_eq!(plan.title, "Swagger Petstore");
        assert_eq!(plan.request_count(), 3, "{file}");
        // Tags become folders.
        assert!(
            matches!(&plan.tree[0], PlanItem::Folder { name, .. } if name == "pets"),
            "{file}"
        );
        let show = by_name(&plan, "Info for a specific pet");
        assert_eq!(show.url, "{{baseUrl}}/pets/:petId");
        assert_eq!(show.path_params[0].name, "petId");
        let list = by_name(&plan, "List all pets");
        assert_eq!(list.params[0].name, "limit");
        assert!(!list.params[0].enabled, "optional query params start disabled");
        assert!(env(&plan, "baseUrl").value.ends_with("/v1"));
    }
}

#[test]
fn swagger2_server_from_host_and_base_path() {
    let plan = plan("petstore-v2.json");
    assert_eq!(plan.servers, ["http://petstore.swagger.io/v1"]);
}

#[test]
fn openapi31_examples_security_override_and_webhooks() {
    let plan = plan("petstore-v3.1.yaml");
    let create = by_name(&plan, "Create a pet");
    let Body::Json { content, .. } = &create.body else {
        panic!("json body")
    };
    assert!(content.contains("\"name\": \"doggie\""), "{content}");
    assert_eq!(by_name(&plan, "List all pets").params[0].value, "20");
    // Global API key on the collection; `security: []` means no auth.
    assert!(
        matches!(&plan.root_auth, Some(Auth::ApiKey { key, location: ApiKeyLocation::Header, value, .. })
        if key == "X-Petstore-Key" && value == "{{apiKey}}")
    );
    assert!(matches!(
        by_name(&plan, "Info for a specific pet").auth,
        Auth::None { .. }
    ));
    assert!(matches!(create.auth, Auth::Inherit));
    assert!(env(&plan, "apiKey").secret);
    assert!(has_warning(&plan, "webhooks"));
}

#[test]
fn webhook_only_spec_has_no_requests() {
    let plan = plan("webhook-example-v3.1.yaml");
    assert_eq!(plan.request_count(), 0);
    assert!(has_warning(&plan, "webhooks"));
    assert!(has_warning(&plan, "no `paths`"));
}

#[test]
fn refs_chains_cycles_external_and_broken() {
    let plan = plan("edge-refs.yaml");
    let chained = by_name(&plan, "Path item via ref, parameter via a ref chain");
    assert_eq!(chained.path_params[0].value, "abc-123");
    let cycle = by_name(&plan, "Parameter ref cycle");
    assert_eq!(cycle.params.len(), 1, "the cyclic parameter is skipped");
    assert_eq!(cycle.params[0].value, "25");
    assert!(has_warning(&plan, "cycle"));
    assert!(has_warning(&plan, "external `$ref`"));
    assert!(has_warning(&plan, "doesn't point to anything"));
    let recursive = by_name(&plan, "Recursive schema");
    let Body::Json { content, .. } = &recursive.body else {
        panic!()
    };
    assert!(content.contains("leaf"));
}

#[test]
fn no_tags_relative_server_headers_and_cookies() {
    let plan = plan("edge-no-tags.json");
    assert!(
        plan.tree.iter().all(|i| matches!(i, PlanItem::Request(_))),
        "untagged ops stay at the root"
    );
    assert_eq!(by_name(&plan, "GET /health").method, "GET");
    assert_eq!(by_name(&plan, "listItems").url, "{{baseUrl}}/items");
    let create = by_name(&plan, "Create item");
    assert!(create.description.as_deref().unwrap().contains("Deprecated"));
    assert!(create.description.as_deref().unwrap().contains("no example body"));
    let header = |n: &str| create.headers.iter().find(|h| h.name == n);
    assert!(header("X-Request-Id").unwrap().enabled);
    assert_eq!(header("X-Request-Id").unwrap().value, "req-1");
    assert!(!header("X-Trace").unwrap().enabled);
    assert!(
        header("Accept").is_none(),
        "Accept/Content-Type/Authorization params are ignored"
    );
    assert_eq!(header("Cookie").unwrap().value, "session=s1; theme=");
    assert!(has_warning(&plan, "relative"));
}

#[test]
fn server_variables_are_expanded_and_listed() {
    let plan = plan("edge-servers.yaml");
    assert_eq!(
        env(&plan, "baseUrl").value,
        "https://eu-west.api.example.com:8443/v3"
    );
    assert_eq!(env(&plan, "region").value, "eu-west");
    assert_eq!(plan.servers.len(), 2);
}

#[test]
fn every_security_scheme_kind() {
    let plan = plan("edge-security.yaml");
    assert!(matches!(&plan.root_auth, Some(Auth::Bearer { token, .. }) if token == "{{token}}"));
    assert!(matches!(
        by_name(&plan, "Uses the global bearer").auth,
        Auth::Inherit
    ));
    assert!(matches!(by_name(&plan, "No auth").auth, Auth::None { .. }));
    assert!(matches!(
        by_name(&plan, "Basic override").auth,
        Auth::Basic { .. }
    ));
    assert!(matches!(&by_name(&plan, "API key in header").auth,
        Auth::ApiKey { key, location: ApiKeyLocation::Header, value, .. } if key == "X-API-Key" && value == "{{apiKey}}"));
    assert!(matches!(&by_name(&plan, "API key in query").auth,
        Auth::ApiKey { key, location: ApiKeyLocation::Query, value, .. } if key == "api_key" && value == "{{queryKey}}"));
    assert!(matches!(by_name(&plan, "OAuth2").auth, Auth::Bearer { .. }));
    assert!(matches!(by_name(&plan, "API key in cookie").auth, Auth::Inherit));
    assert!(matches!(
        by_name(&plan, "Digest (unsupported)").auth,
        Auth::Inherit
    ));
    assert!(has_warning(&plan, "cookie"));
    assert!(has_warning(&plan, "OAuth"));
    assert!(has_warning(&plan, "only the first one"));
    for secret in ["token", "password", "apiKey", "queryKey"] {
        assert!(env(&plan, secret).secret, "{secret}");
    }
    assert!(!env(&plan, "username").secret);
    assert!(
        plan.auth_summary
            .iter()
            .any(|s| s.contains("bearerAuth → Bearer"))
    );
}

#[test]
fn bodies_forms_multipart_examples_and_media_types() {
    let plan = plan("edge-bodies.yaml");
    let Body::FormUrlencoded { fields, .. } = &by_name(&plan, "Form login").body else {
        panic!()
    };
    let field = |n: &str| fields.iter().find(|f| f.name == n).unwrap().value.clone();
    assert_eq!(field("username"), "ada");
    assert_eq!(field("remember"), "true");
    assert_eq!(field("password"), "");

    let Body::Multipart { parts, .. } = &by_name(&plan, "Upload avatar").body else {
        panic!()
    };
    assert_eq!(parts[0].kind, MultipartKind::Text);
    assert_eq!(parts[0].value.as_deref(), Some("Me at the beach"));
    assert_eq!(parts[1].kind, MultipartKind::File);
    assert!(parts[1].path.is_none(), "the user picks the file");

    let json = |name: &str| match &by_name(&plan, name).body {
        Body::Json { content, .. } => content.clone(),
        other => panic!("{name}: {other:?}"),
    };
    assert!(json("Media example").contains("\"nested\""));
    assert!(json("Named examples (first wins)").contains("\"me\""));
    assert!(json("Schema-level example").contains("users"));
    assert_eq!(json("External example value"), "{}");
    assert!(json("JSON preferred over XML").contains("\"pet\": true"));
    let vnd = by_name(&plan, "Schema-level example");
    assert!(
        vnd.headers
            .iter()
            .any(|h| h.name == "Content-Type" && h.value == "application/vnd.api+json")
    );
    assert!(matches!(&by_name(&plan, "XML only").body, Body::Xml { content, .. } if content.contains("Rex")));
    assert!(matches!(&by_name(&plan, "Plain text").body, Body::Text { content, .. } if content == "hello"));
    assert!(has_warning(&plan, "imported as text"));
    assert!(has_warning(&plan, "externalValue"));
    // Folder order and descriptions come from the `tags` array.
    let PlanItem::Folder {
        name, description, ..
    } = &plan.tree[0]
    else {
        panic!()
    };
    assert_eq!(name, "forms");
    assert_eq!(description.as_deref(), Some("Form and multipart bodies"));
}

#[test]
fn swagger2_forms_files_body_params_and_security() {
    let plan = plan("swagger2-forms.json");
    assert_eq!(
        plan.servers,
        ["https://legacy.example.com/api", "http://legacy.example.com/api"]
    );
    let list = by_name(&plan, "List users");
    assert_eq!(
        list.params.len(),
        2,
        "path-level $ref parameter plus the op's own"
    );
    assert!(
        list.params
            .iter()
            .any(|p| p.name == "q" && p.value == "ada" && p.enabled)
    );
    assert!(list.params.iter().any(|p| p.name == "page" && p.value == "1"));
    let create = by_name(&plan, "Create user");
    assert!(matches!(&create.body, Body::Json { content, .. } if content.contains("Ada Lovelace")));
    assert!(matches!(create.auth, Auth::Basic { .. }));
    let Body::Multipart { parts, .. } = &by_name(&plan, "Upload avatar").body else {
        panic!()
    };
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[1].kind, MultipartKind::File);
    let Body::FormUrlencoded { fields, .. } = &by_name(&plan, "Log in").body else {
        panic!()
    };
    assert_eq!(fields[0].value, "ada");
    assert!(matches!(by_name(&plan, "Log in").auth, Auth::None { .. }));
    assert!(
        matches!(&plan.root_auth, Some(Auth::ApiKey { key, location: ApiKeyLocation::Query, .. }) if key == "token")
    );
    assert!(matches!(by_name(&plan, "OAuth").auth, Auth::Bearer { .. }));
}

#[test]
fn yaml_anchors_are_expanded() {
    let plan = plan("anchors.yaml");
    for name in ["A", "B"] {
        let r = by_name(&plan, name);
        assert_eq!(
            r.params.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            ["page", "size"]
        );
    }
}

#[test]
fn odd_structures_become_warnings() {
    let plan = plan("invalid-structure.yaml");
    assert_eq!(
        plan.request_count(),
        2,
        "the string operation is skipped; `fetch` isn't a method"
    );
    assert!(has_warning(&plan, "isn't an object"));
    let post = by_name(&plan, "Missing responses is tolerated");
    assert!(
        post.params.is_empty(),
        "parameters without name or valid `in` are skipped"
    );
}

#[test]
fn not_a_spec_is_an_error() {
    let bytes = std::fs::read(fixtures().join("not-a-spec.json")).unwrap();
    assert!(matches!(
        ironcall_import::openapi::plan(&bytes),
        Err(ImportError::NotASpec)
    ));
}

#[test]
fn snapshots() {
    let mut files: Vec<_> = std::fs::read_dir(fixtures())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != "not-a-spec.json")
        .collect();
    files.sort();
    assert!(files.len() >= 12);
    for file in files {
        let plan = plan(&file);
        insta::assert_json_snapshot!(file.replace('.', "_"), plan);
    }
}
