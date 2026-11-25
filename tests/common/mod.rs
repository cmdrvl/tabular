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
