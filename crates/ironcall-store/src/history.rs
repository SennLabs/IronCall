//! Request history in SQLite (PLAN.md Phase 5).
//!
//! The database lives in the app data dir, readable only by the current user. The
//! store is deliberately dumb about content: snapshots are opaque JSON strings and the
//! app is responsible for redacting them before calling [`History::insert`]. Only the
//! columns needed to filter and search are interpreted here.
//!
//! Schema (version 1):
//! - `entries`: one row per send, newest found via the `(at, id)` index;
//! - `bodies`: optional stored response body (already capped by the caller);
//! - `entries_fts`: FTS5 over method, URL, request name and header text (never bodies),
//!   keyed by `entries.rowid` and kept in sync by a delete trigger.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ironcall_core::ItemId;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

/// Schema version written to `PRAGMA user_version`.
pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("history database: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(
        "the history database was created by a newer version of IronCall (schema {0}); it can't be opened by this version"
    )]
    NewerSchema(i64),
    #[error("invalid history cursor")]
    BadCursor,
}

pub type Result<T> = std::result::Result<T, HistoryError>;

/// A stored response body, capped by the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredBody {
    pub bytes: Vec<u8>,
    /// The original body was larger than the cap.
    pub truncated: bool,
    /// Body kind as the app names it (e.g. "json"), opaque here.
    pub kind: String,
    pub content_type: Option<String>,
}

/// Everything recorded for one send. Snapshots must already be redacted.
#[derive(Clone, Debug, PartialEq)]
pub struct EntryInput {
    pub at: SystemTime,
    pub workspace_id: Option<ItemId>,
    pub workspace_name: Option<String>,
    pub request_id: Option<ItemId>,
    pub request_name: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub error_kind: Option<String>,
    pub total_ms: Option<f64>,
    pub size: Option<u64>,
    /// JSON of the request as authored.
    pub request_authored: String,
    /// JSON of the request as sent.
    pub request_sent: Option<String>,
    /// JSON of the response metadata.
    pub response_meta: Option<String>,
    pub error_message: Option<String>,
    /// Header lines for full-text search (`name: value` per line).
    pub request_headers_text: String,
    pub response_headers_text: String,
    pub body: Option<StoredBody>,
}

impl Default for EntryInput {
    fn default() -> Self {
        Self {
            at: SystemTime::now(),
            workspace_id: None,
            workspace_name: None,
            request_id: None,
            request_name: String::new(),
            method: String::new(),
            url: String::new(),
            status: None,
            error_kind: None,
            total_ms: None,
            size: None,
            request_authored: String::new(),
            request_sent: None,
            response_meta: None,
            error_message: None,
            request_headers_text: String::new(),
            response_headers_text: String::new(),
            body: None,
        }
    }
}

/// One row of a history listing.
#[derive(Clone, Debug, PartialEq)]
pub struct SummaryRow {
    pub id: String,
    pub at: SystemTime,
    pub workspace_id: Option<ItemId>,
    pub workspace_name: Option<String>,
    pub request_id: Option<ItemId>,
    pub request_name: String,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
    pub error_kind: Option<String>,
    pub total_ms: Option<f64>,
    pub size: Option<u64>,
    pub has_body: bool,
}

/// A full entry (without its body; see [`History::body`]).
#[derive(Clone, Debug, PartialEq)]
pub struct EntryRow {
    pub summary: SummaryRow,
    pub request_authored: String,
    pub request_sent: Option<String>,
    pub response_meta: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusClass {
    /// 1xx and 2xx.
    Success,
    /// 3xx.
    Redirect,
    /// 4xx.
    ClientError,
    /// 5xx.
    ServerError,
    /// No response at all.
    Failed,
}

impl StatusClass {
    fn condition(self) -> &'static str {
        match self {
            StatusClass::Success => "(status >= 100 AND status < 300)",
            StatusClass::Redirect => "(status >= 300 AND status < 400)",
            StatusClass::ClientError => "(status >= 400 AND status < 500)",
            StatusClass::ServerError => "(status >= 500)",
            StatusClass::Failed => "(status IS NULL)",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// Full-text search; empty matches everything.
    pub text: String,
    /// Only this workspace's entries.
    pub workspace: Option<ItemId>,
    /// Any of these methods (case-insensitive); empty means all.
    pub methods: Vec<String>,
    /// Any of these classes; empty means all.
    pub status: Vec<StatusClass>,
    /// Entries strictly older than this cursor (from a previous page).
    pub before: Option<String>,
    /// Page size (clamped to 1..=500).
    pub limit: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    pub max_entries: u32,
    pub max_age_days: u32,
    pub max_total_body_bytes: u64,
}

/// What [`History::apply_retention`] removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RetentionReport {
    pub entries_deleted: usize,
    pub bodies_deleted: usize,
}

#[derive(Debug)]
pub struct History {
    conn: Mutex<Connection>,
}

/// Rows deleted before an incremental vacuum is worth running.
const VACUUM_AFTER_ROWS: usize = 200;

impl History {
    /// Opens (creating or migrating) the database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        create_private(path)?;
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let history = Self {
            conn: Mutex::new(conn),
        };
        history.migrate()?;
        Ok(history)
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn.lock().expect("history lock poisoned");
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(HistoryError::NewerSchema(version));
        }
        if version < 1 {
            // auto_vacuum only takes effect before the first table is created.
            conn.pragma_update(None, "auto_vacuum", "INCREMENTAL")?;
            conn.execute_batch(SCHEMA_V1)?;
            conn.pragma_update(None, "user_version", 1)?;
        }
        // Future migrations: `if version < 2 { ... }`.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // With WAL this can only lose the last commits on power loss, never corrupt the
        // database; that's fine for history, and it makes inserts much cheaper.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Ok(())
    }

    /// Records one send and returns its ID (a ULID).
    pub fn insert(&self, entry: &EntryInput) -> Result<String> {
        let id = ItemId::new().to_string();
        let mut conn = self.conn.lock().expect("history lock poisoned");
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO entries (id, at, workspace_id, workspace_name, request_id, request_name, method, url,
                 status, error_kind, total_ms, size, has_body, request_authored, request_sent, response_meta,
                 error_message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                id,
                to_ms(entry.at),
                entry.workspace_id.map(|w| w.to_string()),
                entry.workspace_name,
                entry.request_id.map(|r| r.to_string()),
                entry.request_name,
                entry.method,
                entry.url,
                entry.status,
                entry.error_kind,
                entry.total_ms,
                entry.size.map(|s| s as i64),
                entry.body.is_some(),
                entry.request_authored,
                entry.request_sent,
                entry.response_meta,
                entry.error_message,
            ],
        )?;
        let rowid = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO entries_fts (rowid, method, url, request_name, request_headers, response_headers)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                rowid,
                entry.method,
                entry.url,
                entry.request_name,
                entry.request_headers_text,
                entry.response_headers_text,
            ],
        )?;
        if let Some(body) = &entry.body {
            tx.execute(
                "INSERT INTO bodies (entry_id, bytes, len, truncated, kind, content_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    body.bytes,
                    body.bytes.len() as i64,
                    body.truncated,
                    body.kind,
                    body.content_type
                ],
            )?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// Newest-first page of entries matching `query`, plus the cursor for the next page.
    pub fn list(&self, query: &Query) -> Result<(Vec<SummaryRow>, Option<String>)> {
        let limit = query.limit.clamp(1, 500) as usize;
        let mut sql = format!("SELECT {SUMMARY_COLUMNS} FROM entries WHERE 1=1");
        let mut args: Vec<rusqlite::types::Value> = Vec::new();

        if let Some(fts) = fts_query(&query.text) {
            sql.push_str(" AND rowid IN (SELECT rowid FROM entries_fts WHERE entries_fts MATCH ?)");
            args.push(fts.into());
        }
        if let Some(ws) = query.workspace {
            sql.push_str(" AND workspace_id = ?");
            args.push(ws.to_string().into());
        }
        if !query.methods.is_empty() {
            sql.push_str(&format!(" AND method IN ({})", placeholders(query.methods.len())));
            args.extend(query.methods.iter().map(|m| m.to_ascii_uppercase().into()));
        }
        if !query.status.is_empty() {
            let any: Vec<&str> = query.status.iter().map(|c| c.condition()).collect();
            sql.push_str(&format!(" AND ({})", any.join(" OR ")));
        }
        if let Some(cursor) = &query.before {
            let (at, id) = parse_cursor(cursor)?;
            sql.push_str(" AND (at < ? OR (at = ? AND id < ?))");
            args.extend([at.into(), at.into(), id.into()]);
        }
        sql.push_str(" ORDER BY at DESC, id DESC LIMIT ?");
        args.push(((limit + 1) as i64).into());

        let conn = self.conn.lock().expect("history lock poisoned");
        let mut stmt = conn.prepare(&sql)?;
        let mut rows: Vec<SummaryRow> = stmt
            .query_map(params_from_iter(args), summary_from_row)?
            .collect::<rusqlite::Result<_>>()?;
        let next = if rows.len() > limit {
            rows.truncate(limit);
            rows.last().map(|r| format!("{}:{}", to_ms(r.at), r.id))
        } else {
            None
        };
        Ok((rows, next))
    }

    pub fn get(&self, id: &str) -> Result<Option<EntryRow>> {
        let conn = self.conn.lock().expect("history lock poisoned");
        let row = conn
            .query_row(
                &format!(
                    "SELECT {SUMMARY_COLUMNS}, request_authored, request_sent, response_meta, error_message
                     FROM entries WHERE id = ?1"
                ),
                params![id],
                |r| {
                    Ok(EntryRow {
                        summary: summary_from_row(r)?,
                        request_authored: r.get(13)?,
                        request_sent: r.get(14)?,
                        response_meta: r.get(15)?,
                        error_message: r.get(16)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    pub fn body(&self, id: &str) -> Result<Option<StoredBody>> {
        let conn = self.conn.lock().expect("history lock poisoned");
        let body = conn
            .query_row(
                "SELECT bytes, truncated, kind, content_type FROM bodies WHERE entry_id = ?1",
                params![id],
                |r| {
                    Ok(StoredBody {
                        bytes: r.get(0)?,
                        truncated: r.get(1)?,
                        kind: r.get(2)?,
                        content_type: r.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(body)
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().expect("history lock poisoned");
        Ok(conn.execute("DELETE FROM entries WHERE id = ?1", params![id])? > 0)
    }

    /// Deletes every entry, or only one workspace's.
    pub fn clear(&self, workspace: Option<ItemId>) -> Result<usize> {
        let conn = self.conn.lock().expect("history lock poisoned");
        let deleted = match workspace {
            Some(ws) => conn.execute(
                "DELETE FROM entries WHERE workspace_id = ?1",
                params![ws.to_string()],
            )?,
            None => conn.execute("DELETE FROM entries", [])?,
        };
        if deleted > 0 {
            conn.execute_batch("PRAGMA incremental_vacuum")?;
        }
        Ok(deleted)
    }

    /// Removes the oldest data beyond the limits. Cheap when nothing is over: one
    /// indexed delete plus two aggregate reads.
    pub fn apply_retention(&self, r: Retention) -> Result<RetentionReport> {
        let conn = self.conn.lock().expect("history lock poisoned");
        let mut report = RetentionReport::default();

        let cutoff = to_ms(SystemTime::now()) - i64::from(r.max_age_days) * 86_400_000;
        report.entries_deleted += conn.execute("DELETE FROM entries WHERE at < ?1", params![cutoff])?;

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM entries", [], |row| row.get(0))?;
        let excess = count - i64::from(r.max_entries);
        if excess > 0 {
            report.entries_deleted += conn.execute(
                "DELETE FROM entries WHERE rowid IN (SELECT rowid FROM entries ORDER BY at ASC, id ASC LIMIT ?1)",
                params![excess],
            )?;
        }

        let total: i64 = conn.query_row("SELECT COALESCE(SUM(len), 0) FROM bodies", [], |row| row.get(0))?;
        let mut over = total - r.max_total_body_bytes.min(i64::MAX as u64) as i64;
        if over > 0 {
            // Drop the oldest bodies (keeping their entries) until under the cap.
            let mut stmt = conn.prepare(
                "SELECT b.entry_id, b.len FROM bodies b JOIN entries e ON e.id = b.entry_id
                 ORDER BY e.at ASC, e.id ASC",
            )?;
            let mut victims = Vec::new();
            let mut rows = stmt.query([])?;
            while over > 0
                && let Some(row) = rows.next()?
            {
                let id: String = row.get(0)?;
                let len: i64 = row.get(1)?;
                victims.push(id);
                over -= len;
            }
            drop(rows);
            drop(stmt);
            for id in &victims {
                conn.execute("DELETE FROM bodies WHERE entry_id = ?1", params![id])?;
                conn.execute("UPDATE entries SET has_body = 0 WHERE id = ?1", params![id])?;
            }
            report.bodies_deleted = victims.len();
        }

        if report.entries_deleted + report.bodies_deleted >= VACUUM_AFTER_ROWS || report.bodies_deleted > 0 {
            conn.execute_batch("PRAGMA incremental_vacuum")?;
        }
        Ok(report)
    }
}

const SCHEMA_V1: &str = "
CREATE TABLE entries (
    id TEXT PRIMARY KEY NOT NULL,
    at INTEGER NOT NULL,
    workspace_id TEXT,
    workspace_name TEXT,
    request_id TEXT,
    request_name TEXT NOT NULL,
    method TEXT NOT NULL,
    url TEXT NOT NULL,
    status INTEGER,
    error_kind TEXT,
    total_ms REAL,
    size INTEGER,
    has_body INTEGER NOT NULL DEFAULT 0,
    request_authored TEXT NOT NULL,
    request_sent TEXT,
    response_meta TEXT,
    error_message TEXT
);
CREATE INDEX entries_at ON entries (at DESC, id DESC);
CREATE INDEX entries_workspace_at ON entries (workspace_id, at DESC);

CREATE TABLE bodies (
    entry_id TEXT PRIMARY KEY NOT NULL REFERENCES entries (id) ON DELETE CASCADE,
    bytes BLOB NOT NULL,
    len INTEGER NOT NULL,
    truncated INTEGER NOT NULL,
    kind TEXT NOT NULL,
    content_type TEXT
);

CREATE VIRTUAL TABLE entries_fts USING fts5 (
    method, url, request_name, request_headers, response_headers,
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER entries_fts_delete AFTER DELETE ON entries BEGIN
    DELETE FROM entries_fts WHERE rowid = old.rowid;
END;
";

const SUMMARY_COLUMNS: &str = "id, at, workspace_id, workspace_name, request_id, request_name, method, url, \
     status, error_kind, total_ms, size, has_body";

fn summary_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SummaryRow> {
    let parse_id = |s: Option<String>| s.and_then(|s| s.parse::<ItemId>().ok());
    Ok(SummaryRow {
        id: r.get(0)?,
        at: from_ms(r.get(1)?),
        workspace_id: parse_id(r.get(2)?),
        workspace_name: r.get(3)?,
        request_id: parse_id(r.get(4)?),
        request_name: r.get(5)?,
        method: r.get(6)?,
        url: r.get(7)?,
        status: r.get(8)?,
        error_kind: r.get(9)?,
        total_ms: r.get(10)?,
        size: r.get::<_, Option<i64>>(11)?.map(|s| s as u64),
        has_body: r.get(12)?,
    })
}

/// Turns user input into a safe FTS5 query: each whitespace-separated term becomes a
/// quoted phrase (so `*`, `-`, `:`, `"` and keywords like `NOT` are literal), the last
/// one with a prefix match. Terms without letters or digits are dropped. `None` if
/// nothing searchable remains.
fn fts_query(text: &str) -> Option<String> {
    let terms: Vec<&str> = text
        .split_whitespace()
        .filter(|t| t.chars().any(char::is_alphanumeric))
        .collect();
    let last = terms.len().checked_sub(1)?;
    let quoted: Vec<String> = terms
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let phrase = format!("\"{}\"", t.replace('"', "\"\""));
            if i == last { format!("{phrase}*") } else { phrase }
        })
        .collect();
    Some(quoted.join(" "))
}

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(", ")
}

fn parse_cursor(cursor: &str) -> Result<(i64, String)> {
    let (at, id) = cursor.split_once(':').ok_or(HistoryError::BadCursor)?;
    Ok((at.parse().map_err(|_| HistoryError::BadCursor)?, id.to_string()))
}

fn to_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn from_ms(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)
}

/// Creates the database file readable only by the current user, if it doesn't exist.
/// SQLite gives the `-wal` and `-shm` files the same permissions.
fn create_private(path: &Path) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open() -> (tempfile::TempDir, History) {
        let tmp = tempfile::tempdir().unwrap();
        let h = History::open(&tmp.path().join("history.sqlite")).unwrap();
        (tmp, h)
    }

    fn entry(method: &str, url: &str, status: Option<u16>, at: SystemTime) -> EntryInput {
        EntryInput {
            at,
            request_name: format!("{method} thing"),
            method: method.into(),
            url: url.into(),
            status,
            error_kind: status.is_none().then(|| "connect".into()),
            total_ms: Some(12.5),
            size: Some(42),
            request_authored: r#"{"url":"{{base}}/x"}"#.into(),
            request_sent: Some(r#"{"url":"https://api.example.com/x"}"#.into()),
            response_meta: status.map(|s| format!(r#"{{"status":{s}}}"#)),
            request_headers_text: "Accept: application/json".into(),
            response_headers_text: "Content-Type: application/json\nX-Trace: zürich-42".into(),
            ..EntryInput::default()
        }
    }

    fn ago(secs: u64) -> SystemTime {
        SystemTime::now() - Duration::from_secs(secs)
    }

    fn q(text: &str) -> Query {
        Query {
            text: text.into(),
            limit: 100,
            ..Query::default()
        }
    }

    #[test]
    fn schema_is_created_from_an_empty_v0_database_and_reopens() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("history.sqlite");
        // A v0 database: exists, but has no tables and user_version 0.
        Connection::open(&path)
            .unwrap()
            .execute_batch("SELECT 1")
            .unwrap();
        {
            let h = History::open(&path).unwrap();
            h.insert(&entry("GET", "https://a.example/x", Some(200), ago(1)))
                .unwrap();
        }
        let h = History::open(&path).unwrap();
        assert_eq!(h.list(&q("")).unwrap().0.len(), 1);
        let conn = Connection::open(&path).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
    }

    #[test]
    fn newer_schema_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("history.sqlite");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert!(matches!(History::open(&path), Err(HistoryError::NewerSchema(99))));
    }

    #[cfg(unix)]
    #[test]
    fn database_file_is_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let (tmp, _h) = open();
        let mode = std::fs::metadata(tmp.path().join("history.sqlite"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn insert_get_body_round_trip_stores_exactly_what_is_given() {
        let (_tmp, h) = open();
        let ws = ItemId::new();
        let mut e = entry("POST", "https://api.example.com/login", Some(201), ago(5));
        e.workspace_id = Some(ws);
        e.workspace_name = Some("Demo".into());
        e.request_id = Some(ItemId::new());
        e.body = Some(StoredBody {
            bytes: b"{\"ok\":true}".to_vec(),
            truncated: true,
            kind: "json".into(),
            content_type: Some("application/json".into()),
        });
        let id = h.insert(&e).unwrap();

        let got = h.get(&id).unwrap().unwrap();
        assert_eq!(got.summary.workspace_id, Some(ws));
        assert_eq!(got.summary.workspace_name.as_deref(), Some("Demo"));
        assert_eq!(got.summary.request_id, e.request_id);
        assert_eq!(got.summary.status, Some(201));
        assert!(got.summary.has_body);
        // The store never rewrites snapshots: redaction is the app's job.
        assert_eq!(got.request_authored, e.request_authored);
        assert_eq!(got.request_sent, e.request_sent);
        assert_eq!(got.response_meta, e.response_meta);
        assert_eq!(h.body(&id).unwrap(), e.body);
        assert!(h.get("nope").unwrap().is_none());
    }

    #[test]
    fn full_text_search_with_tricky_input() {
        let (_tmp, h) = open();
        h.insert(&entry(
            "GET",
            "https://api.example.com/users/42",
            Some(200),
            ago(3),
        ))
        .unwrap();
        h.insert(&entry("POST", "https://other.test/orders", Some(500), ago(2)))
            .unwrap();

        let urls =
            |text: &str| -> Vec<String> { h.list(&q(text)).unwrap().0.into_iter().map(|r| r.url).collect() };
        assert_eq!(urls("users"), ["https://api.example.com/users/42"]);
        assert_eq!(urls("api.example"), ["https://api.example.com/users/42"]);
        assert_eq!(
            urls("ord"),
            ["https://other.test/orders"],
            "last term is a prefix"
        );
        assert_eq!(urls("POST orders"), ["https://other.test/orders"]);
        assert_eq!(urls("zurich").len(), 2, "diacritics folded, headers searched");
        // Operators and quotes are literal, never syntax errors.
        for tricky in [
            "\"", "*", "-", "a\"b", "NOT", "users*", "-users", "(", "OR AND", "📦", "",
        ] {
            h.list(&q(tricky)).unwrap();
        }
        assert_eq!(
            urls("NOT users").len(),
            0,
            "NOT is a literal word, not an operator"
        );
        assert_eq!(urls("-").len(), 2, "punctuation-only input matches everything");
    }

    #[test]
    fn filters_combine() {
        let (_tmp, h) = open();
        let ws = ItemId::new();
        for (m, s, w) in [
            ("GET", Some(200), Some(ws)),
            ("GET", Some(404), Some(ws)),
            ("POST", Some(503), None),
            ("DELETE", None, Some(ws)),
            ("GET", Some(302), None),
        ] {
            let mut e = entry(m, "https://x.example/", s, ago(1));
            e.workspace_id = w;
            h.insert(&e).unwrap();
        }
        let count = |query: Query| h.list(&query).unwrap().0.len();
        let base = q("");
        assert_eq!(
            count(Query {
                workspace: Some(ws),
                ..base.clone()
            }),
            3
        );
        assert_eq!(
            count(Query {
                methods: vec!["get".into()],
                ..base.clone()
            }),
            3
        );
        assert_eq!(
            count(Query {
                status: vec![StatusClass::ClientError, StatusClass::ServerError],
                ..base.clone()
            }),
            2
        );
        assert_eq!(
            count(Query {
                status: vec![StatusClass::Failed],
                ..base.clone()
            }),
            1
        );
        assert_eq!(
            count(Query {
                status: vec![StatusClass::Redirect],
                ..base.clone()
            }),
            1
        );
        assert_eq!(
            count(Query {
                workspace: Some(ws),
                methods: vec!["GET".into()],
                status: vec![StatusClass::Success],
                ..base.clone()
            }),
            1
        );
    }

    #[test]
    fn pagination_is_stable_and_complete() {
        let (_tmp, h) = open();
        let same_ms = ago(10);
        // Several entries in the same millisecond, ordered by ID within it.
        for i in 0..23 {
            let at = if i % 3 == 0 { same_ms } else { ago(100 + i) };
            h.insert(&entry("GET", &format!("https://x.example/{i}"), Some(200), at))
                .unwrap();
        }
        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let (page, next) = h
                .list(&Query {
                    before: cursor.clone(),
                    limit: 5,
                    ..q("")
                })
                .unwrap();
            assert!(page.len() <= 5);
            // A new entry arriving mid-paging (newer) must not shift later pages.
            if seen.is_empty() {
                h.insert(&entry(
                    "GET",
                    "https://x.example/new",
                    Some(200),
                    SystemTime::now(),
                ))
                .unwrap();
            }
            seen.extend(page.into_iter().map(|r| r.id));
            match next {
                Some(n) => cursor = Some(n),
                None => break,
            }
        }
        let unique: std::collections::HashSet<_> = seen.iter().collect();
        assert_eq!(seen.len(), 23);
        assert_eq!(unique.len(), 23);
        assert!(
            h.list(&Query {
                before: Some("garbage".into()),
                ..q("")
            })
            .is_err()
        );
    }

    #[test]
    fn retention_by_age_count_and_body_size() {
        let (_tmp, h) = open();
        h.insert(&entry(
            "GET",
            "https://x.example/old",
            Some(200),
            ago(10 * 86_400),
        ))
        .unwrap();
        for i in 0..5 {
            let mut e = entry("GET", &format!("https://x.example/{i}"), Some(200), ago(100 - i));
            e.body = Some(StoredBody {
                bytes: vec![b'x'; 1000],
                truncated: false,
                kind: "text".into(),
                content_type: None,
            });
            h.insert(&e).unwrap();
        }
        let all = |h: &History| h.list(&q("")).unwrap().0;

        let report = h
            .apply_retention(Retention {
                max_entries: 100,
                max_age_days: 7,
                max_total_body_bytes: u64::MAX,
            })
            .unwrap();
        assert_eq!(report.entries_deleted, 1, "the 10-day-old entry");
        assert_eq!(all(&h).len(), 5);

        let report = h
            .apply_retention(Retention {
                max_entries: 100,
                max_age_days: 7,
                max_total_body_bytes: 2500,
            })
            .unwrap();
        assert_eq!(
            report.bodies_deleted, 3,
            "oldest bodies dropped until <= 2500 bytes"
        );
        let rows = all(&h);
        assert_eq!(rows.len(), 5, "entries are kept without their bodies");
        assert_eq!(rows.iter().filter(|r| r.has_body).count(), 2);
        assert!(rows[0].has_body, "newest keeps its body");

        let report = h
            .apply_retention(Retention {
                max_entries: 2,
                max_age_days: 7,
                max_total_body_bytes: u64::MAX,
            })
            .unwrap();
        assert_eq!(report.entries_deleted, 3);
        let rows = all(&h);
        assert_eq!(
            rows.iter().map(|r| r.url.as_str()).collect::<Vec<_>>(),
            ["https://x.example/4", "https://x.example/3"]
        );
        // Deleted entries are gone from search too (trigger keeps FTS in sync).
        assert_eq!(h.list(&q("x.example")).unwrap().0.len(), 2);
    }

    #[test]
    fn delete_and_clear_per_workspace() {
        let (_tmp, h) = open();
        let a = ItemId::new();
        let b = ItemId::new();
        let mut ids = Vec::new();
        for ws in [Some(a), Some(a), Some(b), None] {
            let mut e = entry("GET", "https://x.example/", Some(200), ago(1));
            e.workspace_id = ws;
            e.body = Some(StoredBody {
                bytes: b"x".to_vec(),
                truncated: false,
                kind: "text".into(),
                content_type: None,
            });
            ids.push(h.insert(&e).unwrap());
        }
        assert!(h.delete(&ids[0]).unwrap());
        assert!(!h.delete(&ids[0]).unwrap());
        assert!(h.body(&ids[0]).unwrap().is_none(), "body cascades");
        assert_eq!(h.clear(Some(a)).unwrap(), 1);
        assert_eq!(h.list(&q("")).unwrap().0.len(), 2);
        assert_eq!(h.clear(None).unwrap(), 2);
        assert!(h.list(&q("")).unwrap().0.is_empty());
        assert!(h.list(&q("x.example")).unwrap().0.is_empty());
    }

    #[test]
    fn perf_10k_entries_fts_page_under_50ms() {
        let (_tmp, h) = open();
        let start = std::time::Instant::now();
        for i in 0..10_000u64 {
            let url = format!("https://api{}.example.com/v1/items/{i}", i % 37);
            h.insert(&entry(
                if i % 3 == 0 { "POST" } else { "GET" },
                &url,
                Some(200),
                ago(i),
            ))
            .unwrap();
        }
        let insert = start.elapsed();

        let start = std::time::Instant::now();
        let (page, next) = h
            .list(&Query {
                text: "api7 items".into(),
                methods: vec!["GET".into()],
                limit: 50,
                ..Query::default()
            })
            .unwrap();
        let list = start.elapsed();
        let start = std::time::Instant::now();
        h.list(&Query {
            limit: 50,
            ..Query::default()
        })
        .unwrap();
        let list_plain = start.elapsed();
        eprintln!(
            "history perf: 10k inserts {insert:?} ({:?}/insert), FTS page {list:?}, plain page {list_plain:?}",
            insert / 10_000
        );
        assert_eq!(page.len(), 50);
        assert!(next.is_some());
        assert!(list < Duration::from_millis(50), "FTS page took {list:?}");
    }
}
