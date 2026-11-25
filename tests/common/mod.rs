#![allow(dead_code)]
use std::path::PathBuf;

use cmdrvl_tabular::{Cell, OpenOptions, Refusal, SheetSel, Table, open};

pub fn fx(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

pub fn t(s: &str) -> Cell {
    Cell::from_bytes(s.as_bytes())
}

pub fn grid(table: &mut Table) -> (Vec<Vec<u8>>, Vec<Vec<Cell>>) {
    let headers = table.headers().to_vec();
    let rows = table.rows().map(|r| r.unwrap().cells).collect();
    (headers, rows)
}

pub fn sheet(name: &str) -> OpenOptions {
    OpenOptions {
        sheet: Some(SheetSel::Name(name.into())),
        ..OpenOptions::default()
    }
}

/// The first refusal for this input, whether `open` raised it or the row stream did.
pub fn refusal(name: &str, opts: &OpenOptions) -> Refusal {
    first_refusal(&fx(name), opts).expect("expected a refusal")
}

pub fn first_refusal(path: &std::path::Path, opts: &OpenOptions) -> Option<Refusal> {
    match open(path, opts) {
        Err(refusal) => Some(refusal),
        Ok(mut table) => table.rows().find_map(Result::err),
    }
}

/// Number of data rows, draining the stream (every row must be `Ok`).
pub fn count_rows(table: &mut Table) -> usize {
    let mut n = 0;
    for row in table.rows() {
        row.expect("every row must be Ok");
        n += 1;
    }
    n
}

/// Write `bytes` to a fresh temp file named `name` and return (guard, path).
pub fn temp(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}

/// Compare `file` (opened with `opts`) cell by cell to `expected_file`, a JSON
/// `{"headers": [...], "rows": [[...]]}` whose text was written independently of this crate
/// (an empty string means `Null`).
pub fn assert_grid_matches(file: &str, expected_file: &str, opts: &OpenOptions) {
    let want: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fx(expected_file)).unwrap()).unwrap();
    let strings = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect()
    };
    let want_headers = strings(&want["headers"]);
    let want_rows: Vec<Vec<String>> = want["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(strings)
        .collect();

    let mut table = open(&fx(file), opts).unwrap_or_else(|e| panic!("{file}: {e}"));
    let (headers, rows) = grid(&mut table);
    let headers: Vec<String> = headers
        .iter()
        .map(|h| String::from_utf8(h.clone()).unwrap())
        .collect();
    assert_eq!(headers, want_headers, "{file} headers");
    assert_eq!(rows.len(), want_rows.len(), "{file} row count");
    for (r, (got, want_row)) in rows.iter().zip(&want_rows).enumerate() {
        assert_eq!(got.len(), want_row.len(), "{file} row {} width", r + 1);
        for (c, (cell, want_text)) in got.iter().zip(want_row).enumerate() {
            assert_eq!(
                cell,
                &Cell::from_bytes(want_text.as_bytes()),
                "{file} row {} col {} ({}): got {:?}, want {:?}",
                r + 1,
                c,
                want_headers[c],
                String::from_utf8_lossy(cell.as_bytes()),
                want_text
            );
        }
    }
}
