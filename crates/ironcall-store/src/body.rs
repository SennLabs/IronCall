//! Response body store with a row index for paged viewing (PLAN.md §3.1, Phase 0 Spike C).
//!
//! Bodies are never sent to the frontend whole. A [`RowIndex`] records where each display
//! row starts, so the UI can fetch any range of rows from a file of any size. A row is a
//! line, or a slice of at most [`MAX_ROW_BYTES`] of a longer line (minified JSON is often
//! one huge line), split on a UTF-8 character boundary.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Longest row, in bytes, before a line is split into continuation rows.
pub const MAX_ROW_BYTES: usize = 1024;

/// Most rows returned by one [`Body::rows`] call.
pub const MAX_ROWS_PER_PAGE: usize = 2000;

/// A row holds at most MAX_ROW_BYTES of content plus a `\r\n` line ending.
const ROW_WINDOW: usize = MAX_ROW_BYTES + 2;
const READ_CHUNK: usize = 1 << 20;
const SEARCH_CHUNK: usize = 4 << 20;
/// A line-number checkpoint is stored every this many rows.
const CHECKPOINT_EVERY: usize = 1024;
/// Top bit of a row start marks a continuation row (the rest of a split line).
const CONTINUATION: u64 = 1 << 63;

/// Where each display row of a file starts.
#[derive(Debug, Default)]
pub struct RowIndex {
    /// Byte offset of each row; the top bit is set for continuation rows.
    starts: Vec<u64>,
    /// 0-based line number of row `k * CHECKPOINT_EVERY`.
    checkpoints: Vec<u64>,
    len: u64,
}

impl RowIndex {
    /// Builds the index by streaming `reader`; memory use is 8 bytes per row.
    pub fn build(mut reader: impl Read) -> io::Result<Self> {
        let mut index = RowIndex::default();
        // The current row's start always stays in the buffer, so a split can look back
        // for a UTF-8 boundary and see the byte after the row.
        let mut buf = vec![0u8; READ_CHUNK + ROW_WINDOW];
        let mut buf_offset: u64 = 0; // file offset of buf[0]
        let mut filled = 0usize;
        let mut row = 0usize; // start of the current row, relative to buf
        let mut eof = false;
        let mut continuation = false;
        let mut line: u64 = 0;

        loop {
            if !eof && filled - row < ROW_WINDOW {
                buf.copy_within(row..filled, 0);
                buf_offset += row as u64;
                filled -= row;
                row = 0;
                let n = read_full(&mut reader, &mut buf[filled..])?;
                eof = n == 0;
                filled += n;
                continue;
            }
            if row >= filled {
                break;
            }
            index.push(buf_offset + row as u64, continuation, line);

            let window_end = filled.min(row + ROW_WINDOW);
            if let Some(pos) = memchr::memchr(b'\n', &buf[row..window_end]) {
                row += pos + 1;
                continuation = false;
                line += 1;
            } else if filled - row < ROW_WINDOW {
                // Last row, no trailing newline (only reachable at EOF).
                row = filled;
            } else {
                let mut split = row + MAX_ROW_BYTES;
                while split > row + 1 && is_utf8_continuation(buf[split]) {
                    split -= 1;
                }
                row = split;
                continuation = true;
            }
        }
        index.len = buf_offset + filled as u64;
        Ok(index)
    }

    fn push(&mut self, start: u64, continuation: bool, line: u64) {
        if self.starts.len().is_multiple_of(CHECKPOINT_EVERY) {
            self.checkpoints.push(line);
        }
        self.starts
            .push(if continuation { start | CONTINUATION } else { start });
    }

    pub fn row_count(&self) -> usize {
        self.starts.len()
    }

    pub fn byte_len(&self) -> u64 {
        self.len
    }

    /// Byte offset where `row` starts; `row_count()` maps to the end of the file.
    fn start(&self, row: usize) -> u64 {
        self.starts.get(row).map_or(self.len, |s| s & !CONTINUATION)
    }

    fn is_continuation(&self, row: usize) -> bool {
        self.starts.get(row).is_some_and(|s| s & CONTINUATION != 0)
    }

    /// 0-based line number of `row`.
    pub fn line_of(&self, row: usize) -> u64 {
        let checkpoint = row / CHECKPOINT_EVERY;
        let base = checkpoint * CHECKPOINT_EVERY;
        let new_lines = (base + 1..=row).filter(|&r| !self.is_continuation(r)).count() as u64;
        self.checkpoints[checkpoint] + new_lines
    }

    /// Row containing byte `offset`.
    pub fn row_of(&self, offset: u64) -> usize {
        self.starts
            .partition_point(|s| (s & !CONTINUATION) <= offset)
            .saturating_sub(1)
    }
}

fn is_utf8_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

fn read_full(reader: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// One display row returned to the viewer.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    /// 0-based line number this row belongs to.
    pub line: u64,
    /// True when this row continues a line split at [`MAX_ROW_BYTES`].
    pub continuation: bool,
    pub text: String,
}

/// A search hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub row: usize,
    /// Byte offset of the match in the file. Pass `offset + 1` to find the next hit.
    pub offset: u64,
    /// Character column of the match within its row.
    pub column: usize,
}

/// An indexed body file.
#[derive(Debug)]
pub struct Body {
    path: PathBuf,
    file: Mutex<File>,
    index: RowIndex,
}

impl Body {
    pub fn open(path: &Path) -> io::Result<Self> {
        let index = RowIndex::build(io::BufReader::with_capacity(READ_CHUNK, File::open(path)?))?;
        Ok(Self {
            path: path.to_path_buf(),
            file: Mutex::new(File::open(path)?),
            index,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn index(&self) -> &RowIndex {
        &self.index
    }

    /// Reads `len` bytes at `offset`, clamped to the end of the file.
    pub fn read_range(&self, offset: u64, len: u64) -> io::Result<Vec<u8>> {
        let end = offset.saturating_add(len).min(self.index.len);
        if offset >= end {
            return Ok(Vec::new());
        }
        let mut buf = vec![0u8; (end - offset) as usize];
        let mut file = self.file.lock().expect("body file lock poisoned");
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Returns up to `count` rows starting at `from` (at most [`MAX_ROWS_PER_PAGE`]).
    pub fn rows(&self, from: usize, count: usize) -> io::Result<Vec<Row>> {
        let to = from
            .saturating_add(count.min(MAX_ROWS_PER_PAGE))
            .min(self.index.row_count());
        if from >= to {
            return Ok(Vec::new());
        }
        let base = self.index.start(from);
        let bytes = self.read_range(base, self.index.start(to) - base)?;

        let mut line = self.index.line_of(from);
        let mut rows = Vec::with_capacity(to - from);
        for row in from..to {
            if row > from && !self.index.is_continuation(row) {
                line += 1;
            }
            let s = (self.index.start(row) - base) as usize;
            let e = (self.index.start(row + 1) - base) as usize;
            let mut text = &bytes[s..e];
            // Strip the line ending unless the next row continues this line.
            if !self.index.is_continuation(row + 1) {
                text = text.strip_suffix(b"\n").unwrap_or(text);
                text = text.strip_suffix(b"\r").unwrap_or(text);
            }
            rows.push(Row {
                line,
                continuation: self.index.is_continuation(row),
                text: String::from_utf8_lossy(text).into_owned(),
            });
        }
        Ok(rows)
    }

    /// Finds the first match of `query` at or after byte `from_offset`.
    /// Equivalent to [`Body::find_next`] without cancellation.
    pub fn search(&self, query: &str, from_offset: u64, ignore_case: bool) -> io::Result<Option<Hit>> {
        Ok(self
            .find_next(query, from_offset, ignore_case, &AtomicBool::new(false))?
            .hit())
    }

    /// Finds the first match of `query` starting at or after byte `from_offset`.
    ///
    /// `ignore_case` uses Unicode lowercase folding ([`fold_case`]). `cancel` is checked
    /// between chunks, so a search over a huge body stops promptly.
    pub fn find_next(
        &self,
        query: &str,
        from_offset: u64,
        ignore_case: bool,
        cancel: &AtomicBool,
    ) -> io::Result<SearchOutcome> {
        let Some(matcher) = Matcher::new(query, ignore_case) else {
            return Ok(SearchOutcome::NotFound);
        };
        let mut offset = from_offset;
        while offset < self.index.len {
            if cancel.load(Ordering::Relaxed) {
                return Ok(SearchOutcome::Cancelled);
            }
            let window = self.read_range(offset, SEARCH_CHUNK as u64 + matcher.overlap)?;
            if let Some(pos) = matcher.matches(&window).into_iter().next() {
                return self.hit_at(offset + pos as u64).map(SearchOutcome::Found);
            }
            offset += SEARCH_CHUNK as u64;
        }
        Ok(SearchOutcome::NotFound)
    }

    /// Finds the last match of `query` starting before byte `before_offset`, for
    /// "previous" in the search bar. Pass a hit's `offset` to get the one before it.
    pub fn find_prev(
        &self,
        query: &str,
        before_offset: u64,
        ignore_case: bool,
        cancel: &AtomicBool,
    ) -> io::Result<SearchOutcome> {
        let Some(matcher) = Matcher::new(query, ignore_case) else {
            return Ok(SearchOutcome::NotFound);
        };
        let mut limit = before_offset.min(self.index.len);
        while limit > 0 {
            if cancel.load(Ordering::Relaxed) {
                return Ok(SearchOutcome::Cancelled);
            }
            // Matches must *start* before `limit`, but may extend past it.
            let start = limit.saturating_sub(SEARCH_CHUNK as u64);
            let window = self.read_range(start, limit - start + matcher.overlap)?;
            let hit = matcher
                .matches(&window)
                .into_iter()
                .rfind(|&pos| start + (pos as u64) < limit);
            if let Some(pos) = hit {
                return self.hit_at(start + pos as u64).map(SearchOutcome::Found);
            }
            limit = start;
        }
        Ok(SearchOutcome::NotFound)
    }

    fn hit_at(&self, offset: u64) -> io::Result<Hit> {
        let row = self.index.row_of(offset);
        let row_start = self.index.start(row);
        let prefix = self.read_range(row_start, offset - row_start)?;
        let column = String::from_utf8_lossy(&prefix).chars().count();
        Ok(Hit { row, offset, column })
    }

    /// Copies the body file to `dest` (for "Save response to file").
    pub fn copy_to(&self, dest: &Path) -> io::Result<u64> {
        std::fs::copy(&self.path, dest)?;
        // `copy`'s own count comes from a Windows progress callback that not every
        // implementation calls; the file's length is always right.
        Ok(std::fs::metadata(dest)?.len())
    }
}

/// Result of a cancellable search.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOutcome {
    Found(Hit),
    NotFound,
    Cancelled,
}

impl SearchOutcome {
    pub fn hit(self) -> Option<Hit> {
        match self {
            SearchOutcome::Found(hit) => Some(hit),
            _ => None,
        }
    }
}

/// Lowercases `text` for case-insensitive search (Unicode simple lowercase).
///
/// TODO: switch to `ironcall_core::content::fold_case` once it exists, so search and the
/// UI fold the same way.
pub fn fold_case(text: &str) -> String {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// Finds a query in byte windows, exactly or with case folding.
struct Matcher {
    needle: Vec<u8>,
    mode: Mode,
    /// Extra bytes read past each window so matches straddling a boundary are found.
    overlap: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Exact,
    /// The needle is ASCII: fold ASCII letters only (fast, in place). Non-ASCII text
    /// can't fold to an ASCII needle except for rare cases like the Kelvin sign.
    Ascii,
    /// Fold every character, keeping a map back to original byte offsets.
    Unicode,
}

impl Matcher {
    fn new(query: &str, ignore_case: bool) -> Option<Self> {
        if query.is_empty() {
            return None;
        }
        let (needle, mode) = match (ignore_case, query.is_ascii()) {
            (false, _) => (query.as_bytes().to_vec(), Mode::Exact),
            (true, true) => (query.to_ascii_lowercase().into_bytes(), Mode::Ascii),
            (true, false) => (fold_case(query).into_bytes(), Mode::Unicode),
        };
        // Folding can change byte lengths (e.g. 'İ' is 2 bytes, its lowercase 3), so allow
        // up to 4 bytes per needle character in the original text.
        let overlap = (query.chars().count() * 4).max(needle.len()) as u64;
        Some(Self {
            needle,
            mode,
            overlap,
        })
    }

    /// Start offsets (within `window`) of every match, in order.
    fn matches(&self, window: &[u8]) -> Vec<usize> {
        let finder = memchr::memmem::Finder::new(&self.needle);
        match self.mode {
            Mode::Exact => finder.find_iter(window).collect(),
            Mode::Ascii => {
                let lower = window.to_ascii_lowercase();
                finder.find_iter(&lower).collect()
            }
            Mode::Unicode => {
                let (folded, origin) = fold_with_offsets(window);
                let mut out: Vec<usize> = finder.find_iter(&folded).map(|i| origin[i]).collect();
                out.dedup();
                out
            }
        }
    }
}

/// Folds `bytes` to lowercase, returning the folded bytes and, for each folded byte, the
/// offset in `bytes` of the character it came from. Invalid UTF-8 (including a character
/// cut off at either end of the window) is copied through unchanged, so it can't match a
/// valid needle but keeps offsets aligned.
fn fold_with_offsets(bytes: &[u8]) -> (Vec<u8>, Vec<usize>) {
    let mut folded = Vec::with_capacity(bytes.len());
    let mut origin = Vec::with_capacity(bytes.len());
    let mut pos = 0;
    for chunk in bytes.utf8_chunks() {
        for (i, c) in chunk.valid().char_indices() {
            let mut buf = [0u8; 4];
            for lower in c.to_lowercase() {
                let encoded = lower.encode_utf8(&mut buf);
                folded.extend_from_slice(encoded.as_bytes());
                origin.extend(std::iter::repeat_n(pos + i, encoded.len()));
            }
        }
        pos += chunk.valid().len();
        for (i, &b) in chunk.invalid().iter().enumerate() {
            folded.push(b);
            origin.push(pos + i);
        }
        pos += chunk.invalid().len();
    }
    (folded, origin)
}

/// ID of an open body, unique for the life of the process.
pub type BodyId = u32;

/// Open bodies, keyed by ID. Cheap to share between threads.
#[derive(Debug, Default)]
pub struct BodyStore {
    next_id: AtomicU32,
    bodies: Mutex<HashMap<BodyId, Arc<Body>>>,
}

impl BodyStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Indexes the file at `path` and returns its ID. The caller must check the path
    /// with the [`PathGuard`](crate::PathGuard) first.
    pub fn open(&self, path: &Path) -> io::Result<(BodyId, Arc<Body>)> {
        let body = Arc::new(Body::open(path)?);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.bodies
            .lock()
            .expect("body store lock poisoned")
            .insert(id, body.clone());
        Ok((id, body))
    }

    pub fn get(&self, id: BodyId) -> Option<Arc<Body>> {
        self.bodies
            .lock()
            .expect("body store lock poisoned")
            .get(&id)
            .cloned()
    }

    pub fn close(&self, id: BodyId) {
        self.bodies.lock().expect("body store lock poisoned").remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn body_from(bytes: &[u8]) -> (tempfile::NamedTempFile, Body) {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(bytes).unwrap();
        file.flush().unwrap();
        let body = Body::open(file.path()).unwrap();
        (file, body)
    }

    fn texts(body: &Body) -> Vec<String> {
        body.rows(0, MAX_ROWS_PER_PAGE)
            .unwrap()
            .into_iter()
            .map(|r| r.text)
            .collect()
    }

    #[test]
    fn empty_file_has_no_rows() {
        let (_f, body) = body_from(b"");
        assert_eq!(body.index().row_count(), 0);
        assert!(body.rows(0, 10).unwrap().is_empty());
    }

    #[test]
    fn splits_lines_with_and_without_trailing_newline() {
        let (_f, body) = body_from(b"a\nbb\nccc");
        assert_eq!(texts(&body), ["a", "bb", "ccc"]);
        let (_f, body) = body_from(b"a\nbb\n");
        assert_eq!(texts(&body), ["a", "bb"]);
    }

    #[test]
    fn strips_crlf_and_keeps_empty_lines() {
        let (_f, body) = body_from(b"a\r\n\r\nb\r\n");
        assert_eq!(texts(&body), ["a", "", "b"]);
        let rows = body.rows(0, 10).unwrap();
        assert_eq!(rows.iter().map(|r| r.line).collect::<Vec<_>>(), [0, 1, 2]);
    }

    #[test]
    fn long_lines_split_into_continuation_rows() {
        let mut data = vec![b'x'; MAX_ROW_BYTES * 2 + 10];
        data.extend_from_slice(b"\nend");
        let (_f, body) = body_from(&data);
        let rows = body.rows(0, 10).unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows.iter().map(|r| r.text.len()).collect::<Vec<_>>(),
            [MAX_ROW_BYTES, MAX_ROW_BYTES, 10, 3]
        );
        assert_eq!(
            rows.iter().map(|r| r.continuation).collect::<Vec<_>>(),
            [false, true, true, false]
        );
        assert_eq!(rows.iter().map(|r| r.line).collect::<Vec<_>>(), [0, 0, 0, 1]);
    }

    #[test]
    fn long_line_split_never_breaks_utf8() {
        // 'é' is 2 bytes; offset by one so a naive split lands mid-character.
        let mut data = b"a".to_vec();
        for _ in 0..MAX_ROW_BYTES {
            data.extend_from_slice("é".as_bytes());
        }
        let (_f, body) = body_from(&data);
        let joined: String = texts(&body).concat();
        assert!(!joined.contains('\u{FFFD}'));
        assert_eq!(joined.len(), data.len());
    }

    #[test]
    fn line_exactly_max_bytes_is_not_split() {
        let mut data = vec![b'y'; MAX_ROW_BYTES];
        data.extend_from_slice(b"\r\nz");
        let (_f, body) = body_from(&data);
        assert_eq!(body.index().row_count(), 2);
        assert_eq!(texts(&body)[1], "z");
    }

    #[test]
    fn rows_spanning_read_chunks_are_indexed() {
        let line = "0123456789abcdef\n";
        let data = line.repeat(READ_CHUNK / line.len() * 3);
        let (_f, body) = body_from(data.as_bytes());
        let n = body.index().row_count();
        assert_eq!(n, data.len() / line.len());
        let last = body.rows(n - 1, 5).unwrap();
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].text, "0123456789abcdef");
        assert_eq!(last[0].line, (n - 1) as u64);
    }

    #[test]
    fn paging_bounds_are_clamped() {
        let (_f, body) = body_from(b"1\n2\n3\n");
        assert_eq!(body.rows(1, 100).unwrap().len(), 2);
        assert!(body.rows(3, 10).unwrap().is_empty());
        assert!(body.rows(usize::MAX, 10).unwrap().is_empty());
        let data = "x\n".repeat(MAX_ROWS_PER_PAGE + 5);
        let (_f, body) = body_from(data.as_bytes());
        assert_eq!(body.rows(0, usize::MAX).unwrap().len(), MAX_ROWS_PER_PAGE);
    }

    #[test]
    fn line_numbers_survive_checkpoints() {
        let mut data = String::new();
        for i in 0..CHECKPOINT_EVERY * 3 {
            if i % 7 == 0 {
                data.push_str(&"w".repeat(MAX_ROW_BYTES + 1));
            }
            data.push_str(&format!("{i}\n"));
        }
        let (_f, body) = body_from(data.as_bytes());
        let rows = body.rows(0, MAX_ROWS_PER_PAGE).unwrap();
        for (expected_line, row) in rows.iter().filter(|r| !r.continuation).enumerate() {
            assert_eq!(row.line, expected_line as u64);
        }
        let n = body.index().row_count();
        let tail = body.rows(n - 1, 1).unwrap();
        assert_eq!(tail[0].line, (CHECKPOINT_EVERY * 3 - 1) as u64);
    }

    #[test]
    fn search_finds_successive_hits_with_row_and_column() {
        let (_f, body) = body_from("héllo needle\nno\nneedle again".as_bytes());
        let first = body.search("needle", 0, false).unwrap().unwrap();
        assert_eq!((first.row, first.column), (0, 6));
        let second = body.search("needle", first.offset + 1, false).unwrap().unwrap();
        assert_eq!((second.row, second.column), (2, 0));
        assert!(body.search("needle", second.offset + 1, false).unwrap().is_none());
    }

    #[test]
    fn search_ignores_ascii_case_and_handles_empty_query() {
        let (_f, body) = body_from(b"Alpha\nBETA");
        assert_eq!(body.search("beta", 0, true).unwrap().unwrap().row, 1);
        assert!(body.search("beta", 0, false).unwrap().is_none());
        assert!(body.search("", 0, false).unwrap().is_none());
    }

    #[test]
    fn search_finds_match_across_search_chunks() {
        let mut data = vec![b'.'; SEARCH_CHUNK - 3];
        data.extend_from_slice(b"needle");
        let (_f, body) = body_from(&data);
        let hit = body.search("needle", 0, false).unwrap().unwrap();
        assert_eq!(hit.offset, (SEARCH_CHUNK - 3) as u64);
    }

    fn no_cancel() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn unicode_case_insensitive_search_maps_back_to_original_offsets() {
        let text = "Zürich CAFÉ İstanbul\nnext ÉCOLE";
        let (_f, body) = body_from(text.as_bytes());
        let hit = body
            .find_next("café", 0, true, &no_cancel())
            .unwrap()
            .hit()
            .unwrap();
        assert_eq!(hit.offset as usize, text.find("CAFÉ").unwrap());
        assert_eq!((hit.row, hit.column), (0, 7));
        // 'İ' lowercases to two chars (3 bytes); offsets after it must still be right.
        let hit = body
            .find_next("école", 0, true, &no_cancel())
            .unwrap()
            .hit()
            .unwrap();
        assert_eq!(hit.offset as usize, text.find("ÉCOLE").unwrap());
        assert_eq!((hit.row, hit.column), (1, 5));
        assert!(
            body.find_next("café", 0, false, &no_cancel())
                .unwrap()
                .hit()
                .is_none()
        );
    }

    #[test]
    fn unicode_search_across_chunk_boundary_with_split_character() {
        // Put a 2-byte 'É' straddling the SEARCH_CHUNK boundary.
        let mut data = vec![b'.'; SEARCH_CHUNK - 1];
        data.extend_from_slice("ÉCOLE".as_bytes());
        let (_f, body) = body_from(&data);
        let hit = body
            .find_next("école", 0, true, &no_cancel())
            .unwrap()
            .hit()
            .unwrap();
        assert_eq!(hit.offset, (SEARCH_CHUNK - 1) as u64);
    }

    #[test]
    fn find_prev_walks_backwards_including_across_chunks() {
        let mut data = b"needle ".to_vec();
        data.extend(vec![b'.'; SEARCH_CHUNK + 10]);
        data.extend_from_slice(b"needle needle");
        let (_f, body) = body_from(&data);
        let len = data.len() as u64;
        let last = body
            .find_prev("needle", len, false, &no_cancel())
            .unwrap()
            .hit()
            .unwrap();
        assert_eq!(last.offset, len - 6);
        let middle = body
            .find_prev("needle", last.offset, false, &no_cancel())
            .unwrap()
            .hit()
            .unwrap();
        assert_eq!(middle.offset, len - 13);
        let first = body
            .find_prev("needle", middle.offset, false, &no_cancel())
            .unwrap()
            .hit()
            .unwrap();
        assert_eq!(first.offset, 0);
        assert_eq!(
            body.find_prev("needle", 0, false, &no_cancel()).unwrap(),
            SearchOutcome::NotFound
        );
        // Case-insensitive and Unicode backwards too.
        let (_f, body) = body_from("Éa éa".as_bytes());
        let hit = body
            .find_prev("éa", 100, true, &no_cancel())
            .unwrap()
            .hit()
            .unwrap();
        assert_eq!(hit.offset, "Éa ".len() as u64);
    }

    #[test]
    fn search_can_be_cancelled() {
        let (_f, body) = body_from(&vec![b'.'; 1024]);
        let cancel = AtomicBool::new(true);
        assert_eq!(
            body.find_next("x", 0, false, &cancel).unwrap(),
            SearchOutcome::Cancelled
        );
        assert_eq!(
            body.find_prev("x", 1024, false, &cancel).unwrap(),
            SearchOutcome::Cancelled
        );
    }

    #[test]
    fn copy_to_writes_the_body() {
        let (_f, body) = body_from(b"saved bytes");
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out.bin");
        assert_eq!(body.copy_to(&dest).unwrap(), 11);
        assert_eq!(std::fs::read(dest).unwrap(), b"saved bytes");
    }

    #[test]
    fn store_assigns_ids_and_closes() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"x").unwrap();
        let store = BodyStore::new();
        let (a, _) = store.open(file.path()).unwrap();
        let (b, _) = store.open(file.path()).unwrap();
        assert_ne!(a, b);
        assert!(store.get(a).is_some());
        store.close(a);
        assert!(store.get(a).is_none());
    }

    /// Index and paging timings for a 100MB pretty-printed JSON file. Run with:
    /// `cargo test -p ironcall-store --release -- --ignored bench_100mb --nocapture`
    #[test]
    #[ignore]
    fn bench_100mb() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        let mut w = io::BufWriter::new(file.as_file_mut());
        write_test_json(&mut w, 100 * 1024 * 1024).unwrap();
        drop(w);

        let t = std::time::Instant::now();
        let body = Body::open(file.path()).unwrap();
        let index_ms = t.elapsed().as_secs_f64() * 1000.0;

        let n = body.index().row_count();
        let mut page_us = Vec::new();
        for i in 0..500 {
            let from = (i * 7919 * 13) % n;
            let t = std::time::Instant::now();
            body.rows(from, 200).unwrap();
            page_us.push(t.elapsed().as_micros());
        }
        page_us.sort_unstable();
        let t = std::time::Instant::now();
        let miss = body.search("not-present-anywhere", 0, false).unwrap();
        let search_ms = t.elapsed().as_secs_f64() * 1000.0;
        assert!(miss.is_none());
        println!(
            "rows={n} bytes={} index_ms={index_ms:.0} page200_p50_us={} page200_p95_us={} full_search_ms={search_ms:.0}",
            body.index().byte_len(),
            page_us[page_us.len() / 2],
            page_us[page_us.len() * 95 / 100],
        );
    }

    fn write_test_json(w: &mut impl Write, target: usize) -> io::Result<()> {
        let mut written = 0;
        written += w.write(b"[\n")?;
        let mut i = 0u64;
        while written < target {
            let s = format!(
                "  {{\n    \"id\": {i},\n    \"name\": \"user-{i}\",\n    \"email\": \"user{i}@example.com\",\n    \"active\": {}\n  }},\n",
                i.is_multiple_of(2)
            );
            written += s.len();
            w.write_all(s.as_bytes())?;
            i += 1;
        }
        w.write_all(b"  {}\n]\n")
    }
}
