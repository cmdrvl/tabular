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
use crate::{Cell, Row, check_row_limit, missing_headers};

pub(crate) fn read(
    path: &Path,
    max_rows: Option<u64>,
) -> Result<(Vec<Vec<u8>>, Vec<Row>), Refusal> {
    let mut file = File::open(path).map_err(|e| Refusal::io(&e))?;
    let mut head = Vec::new();
    file.by_ref()
        .take(64 * 1024)
        .read_to_end(&mut head)
        .map_err(|e| Refusal::io(&e))?;
    let bom = check_text_sample(&head)?;
    let mut reader = BufReader::new(File::open(path).map_err(|e| Refusal::io(&e))?);
    let mut skip = vec![0u8; bom];
    reader.read_exact(&mut skip).map_err(|e| Refusal::io(&e))?;

    let mut headers: Vec<Vec<u8>> = Vec::new();
    let mut positions: BTreeMap<String, usize> = BTreeMap::new();
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut line_no = 0u64;
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|e| Refusal::io(&e))?
            == 0
        {
            break;
        }
        line_no += 1;
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let value: Value = serde_json::from_slice(&line).map_err(|e| {
            Refusal::new(RefusalCode::EJsonlParse, "invalid JSON")
                .with("line", line_no)
                .with("column", e.column())
        })?;
        let Value::Object(object) = value else {
            return Err(
                Refusal::new(RefusalCode::EJsonlParse, "line is not a JSON object")
                    .with("line", line_no),
            );
        };
        check_row_limit(rows.len() as u64 + 1, max_rows)?;
        let mut cells = vec![Cell::Null; headers.len()];
        for (key, value) in &object {
            let column = *positions.entry(key.clone()).or_insert_with(|| {
                headers.push(key.as_bytes().to_vec());
                headers.len() - 1
            });
            if column >= cells.len() {
                cells.resize(column + 1, Cell::Null);
            }
            cells[column] = json_cell(value, key, rows.len() as u64 + 1)?;
        }
        rows.push(cells);
    }
    if headers.is_empty() {
        return Err(missing_headers());
    }
    let rows = rows
        .into_iter()
        .enumerate()
        .map(|(i, mut cells)| {
            cells.resize(headers.len(), Cell::Null);
            Row {
                index: i as u64 + 1,
                cells,
            }
        })
        .collect();
    Ok((headers, rows))
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
