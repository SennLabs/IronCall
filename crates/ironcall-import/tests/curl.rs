//! cURL import corpus (tests/fixtures/curl): snapshots of what each command parses to,
//! and a round trip through `wire::build` and export.

use std::path::{Path, PathBuf};

use ironcall_core::model::RequestFile;
use ironcall_core::wire::{self, WireContext, WireRequest};
use ironcall_import::curl::{self, Dialect, ExportOptions};

fn corpus() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/curl");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "sh" || e == "cmd"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let name = p.file_stem().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read_to_string(&p).unwrap())
        })
        .collect()
}

/// The parsed request without its random ID, plus warnings.
fn snapshot_value(parsed: &curl::Parsed) -> serde_json::Value {
    let mut request = serde_json::to_value(&parsed.request).unwrap();
    let obj = request.as_object_mut().unwrap();
    obj.remove("id");
    obj.remove("schema");
    serde_json::json!({
        "request": request,
        "warnings": parsed.warnings.iter().map(|w| &w.message).collect::<Vec<_>>(),
    })
}

#[test]
fn corpus_parses_as_expected() {
    let corpus = corpus();
    assert!(corpus.len() >= 30, "corpus has {} samples", corpus.len());
    for (name, text) in corpus {
        let parsed = curl::parse(&text).unwrap_or_else(|e| panic!("{name}: {e}"));
        insta::assert_json_snapshot!(name, snapshot_value(&parsed));
    }
}

/// Builds what would be sent. File paths resolve against a fixed root so a relative
/// `@file` and its exported absolute form compare equal.
fn wire_of(request: &RequestFile) -> WireRequest {
    let ctx = WireContext {
        auth: request.auth.clone(),
        // An absolute root on each OS (`/workspace` has no drive letter on Windows).
        root: Some(PathBuf::from(if cfg!(windows) {
            r"C:\workspace"
        } else {
            "/workspace"
        })),
        ..WireContext::default()
    };
    wire::build(request, &ctx).unwrap_or_else(|e| panic!("{}: {e}", request.name))
}

/// Header order differs (auth is applied after user headers); compare as a set.
fn normalized(mut w: WireRequest) -> WireRequest {
    for (name, _) in &mut w.headers {
        *name = name.to_ascii_lowercase();
    }
    w.headers.sort();
    w
}

#[test]
fn round_trip_through_export_is_equivalent() {
    for (name, text) in corpus() {
        let first = curl::parse(&text).unwrap();
        let wire1 = wire_of(&first.request);
        let exported = curl::export(&wire1, Dialect::Posix, ExportOptions::default());
        let second = curl::parse(&exported.command)
            .unwrap_or_else(|e| panic!("{name}: re-parse failed: {e}\n{}", exported.command));
        let wire2 = wire_of(&second.request);
        assert_eq!(
            normalized(wire1.clone()),
            normalized(wire2),
            "{name}: round trip changed the request\n{}",
            exported.command
        );
    }
}

#[test]
fn powershell_export_quotes_and_continues_lines() {
    let parsed = curl::parse(
        r#"curl 'https://api.example.com/it' -H "X-Note: it's" -H 'Content-Type: application/json' --data-raw '{"a":"b"}'"#,
    )
    .unwrap();
    let export = curl::export(
        &wire_of(&parsed.request),
        Dialect::PowerShell,
        ExportOptions::default(),
    );
    assert_eq!(
        export.command,
        "curl.exe 'https://api.example.com/it' `\n  -H 'X-Note: it''s' `\n  -H 'Content-Type: application/json' `\n  --data-raw '{\"a\":\"b\"}' `\n  -L"
    );
    assert!(
        export
            .warnings
            .iter()
            .any(|w| w.message.contains("PowerShell 5.1")),
        "warns about PS 5.1 quote stripping"
    );
}

#[test]
fn posix_export_layout() {
    let parsed = curl::parse(
        "curl -u ada:s3cret -X PUT https://x.example/items/1 -H 'Content-Type: text/plain' -d 'hi there'",
    )
    .unwrap();
    let export = curl::export(
        &wire_of(&parsed.request),
        Dialect::Posix,
        ExportOptions::default(),
    );
    assert_eq!(
        export.command,
        "curl 'https://x.example/items/1' \\\n  -X 'PUT' \\\n  -H 'Content-Type: text/plain' \\\n  -u 'ada:s3cret' \\\n  --data-raw 'hi there' \\\n  -L"
    );
    let one_line = curl::export(
        &wire_of(&parsed.request),
        Dialect::Posix,
        ExportOptions { single_line: true },
    );
    assert!(!one_line.command.contains('\n'));
}

#[test]
fn engine_headers_are_not_exported() {
    let parsed = curl::parse("curl https://x.example/").unwrap();
    let export = curl::export(
        &wire_of(&parsed.request),
        Dialect::Posix,
        ExportOptions::default(),
    );
    for header in ["User-Agent", "Accept-Encoding", "Host", "Content-Length"] {
        assert!(!export.command.contains(header), "{header} in {}", export.command);
    }
}

#[test]
fn errors() {
    assert!(matches!(
        curl::parse("wget https://x"),
        Err(curl::ParseError::NotCurl)
    ));
    assert!(matches!(curl::parse("curl -k"), Err(curl::ParseError::NoUrl)));
    assert!(matches!(
        curl::parse("curl -H"),
        Err(curl::ParseError::MissingValue(_))
    ));
    assert!(matches!(
        curl::parse("curl 'https://x"),
        Err(curl::ParseError::UnterminatedQuote('\''))
    ));
    assert!(matches!(
        curl::parse("curl.exe 'https://x' `\n  -k"),
        Err(curl::ParseError::Invalid(_))
    ));
}

#[test]
fn unknown_flags_are_reported_never_dropped() {
    let parsed = curl::parse("curl --frobnicate -Q https://x.example/").unwrap();
    let all: String = parsed.warnings.iter().map(|w| w.message.clone()).collect();
    assert!(all.contains("--frobnicate") && all.contains("-Q"), "{all}");
    // Harmless flags stay quiet.
    let quiet = curl::parse("curl -sSv --compressed --progress-bar https://x.example/").unwrap();
    assert!(quiet.warnings.is_empty(), "{:?}", quiet.warnings);
}
