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

pub fn refusal(name: &str, opts: &OpenOptions) -> Refusal {
    open(&fx(name), opts).expect_err("expected a refusal")
}

/// Write `bytes` to a fresh temp file named `name` and return (guard, path).
pub fn temp(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).unwrap();
    (dir, path)
}
