//! JSONL / NDJSON reader. Headers are the union of object keys in first-seen
//! order; rows that lack a later key are `Null`-padded.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

use serde_json::Value;

use crate::cell::canon_f64;
use crate::detect::check_text_sample;
use crate::refusal::{Refusal, RefusalCode};
use crate::{Cell, Opened, Row, missing_headers};

/// Two passes, both streaming. Pass 1 (in `open`) parses every line to collect the
/// header union in first-seen order, so memory is O(columns) and a malformed line
/// refuses before any row is produced. Pass 2 (the row stream) re-reads the file
/// and yields one padded row per object; per-value refusals (a nested value, a
/// non-finite number) surface there, after the earlier rows.
pub(crate) fn open(path: &Path) -> Result<Opened, Refusal> {
    let bom = {
        let mut head = Vec::new();
        File::open(path)
            .map_err(|e| Refusal::io(&e))?
            .take(64 * 1024)
            .read_to_end(&mut head)
            .map_err(|e| Refusal::io(&e))?;
        check_text_sample(&head)?
    };

    let mut headers: Vec<Vec<u8>> = Vec::new();
    let mut positions: BTreeMap<String, usize> = BTreeMap::new();
    let mut lines = Lines::new(path, bom)?;
    while let Some(object) = lines.next_object()? {
        for key in object.keys() {
            positions.entry(key.clone()).or_insert_with(|| {
                headers.push(key.as_bytes().to_vec());
                headers.len() - 1
            });
        }
    }
    if headers.is_empty() {
        return Err(missing_headers());
    }

    let width = headers.len();
    let mut pass2 = Lines::new(path, bom)?;
    let mut index = 0u64;
    let rows = std::iter::from_fn(move || {
        let object = match pass2.next_object() {
            Ok(Some(next)) => next,
            Ok(None) => return None,
            Err(refusal) => return Some(Err(refusal)),
        };
        index += 1;
        let mut cells = vec![Cell::Null; width];
        for (key, value) in &object {
            // Every key was seen in pass 1, so it has a position; a miss would mean the
            // file changed between passes, which is an I/O-level refusal.
            let Some(&column) = positions.get(key) else {
                return Some(Err(Refusal::new(
                    RefusalCode::EJsonlParse,
                    "file changed while reading",
                )));
            };
            match json_cell(value, key, index) {
                Ok(cell) => cells[column] = cell,
                Err(refusal) => return Some(Err(refusal)),
            }
        }
        Some(Ok(Row { index, cells }))
    });
    Ok(Opened {
        headers,
        rows: Box::new(rows),
        delimiter: None,
        sheet: None,
    })
}

/// Line-by-line object reader shared by both passes.
struct Lines {
    reader: BufReader<File>,
    line_no: u64,
    buf: Vec<u8>,
}

impl Lines {
    fn new(path: &Path, bom: usize) -> Result<Self, Refusal> {
        let mut reader = BufReader::new(File::open(path).map_err(|e| Refusal::io(&e))?);
        let mut skip = vec![0u8; bom];
        reader.read_exact(&mut skip).map_err(|e| Refusal::io(&e))?;
        Ok(Self {
            reader,
            line_no: 0,
            buf: Vec::new(),
        })
    }

    /// The next JSON object (blank lines skipped), or `None` at end of file.
    fn next_object(&mut self) -> Result<Option<serde_json::Map<String, Value>>, Refusal> {
        loop {
            self.buf.clear();
            let n = self
                .reader
                .read_until(b'\n', &mut self.buf)
                .map_err(|e| Refusal::io(&e))?;
            if n == 0 {
                return Ok(None);
            }
            self.line_no += 1;
            if self.buf.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            let value: Value = serde_json::from_slice(&self.buf).map_err(|e| {
                Refusal::new(RefusalCode::EJsonlParse, "invalid JSON")
                    .with("line", self.line_no)
                    .with("column", e.column())
            })?;
            let Value::Object(object) = value else {
                return Err(
                    Refusal::new(RefusalCode::EJsonlParse, "line is not a JSON object")
                        .with("line", self.line_no),
                );
            };
            return Ok(Some(object));
        }
    }
}

fn json_cell(value: &Value, key: &str, row: u64) -> Result<Cell, Refusal> {
    let unsupported = |kind: &str| {
        Refusal::new(RefusalCode::EUnsupportedValue, "unsupported JSON value")
            .with("kind", kind)
            .with("column", key)
            .with("row", row)
    };
    Ok(match value {
        Value::Null => Cell::Null,
        Value::Bool(b) => Cell::from_bytes(b.to_string()),
        Value::String(s) => Cell::from_bytes(s.as_bytes()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Cell::from_bytes(i.to_string())
            } else if let Some(u) = n.as_u64() {
                Cell::from_bytes(u.to_string())
            } else {
                match n.as_f64().and_then(canon_f64) {
                    Some(text) => Cell::from_bytes(text),
                    None => return Err(unsupported("non-finite number")),
                }
            }
        }
        Value::Array(_) | Value::Object(_) => return Err(unsupported("nested")),
    })
}
