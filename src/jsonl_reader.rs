//! JSONL / NDJSON reader. Headers are the union of object keys in first-seen
//! order; rows that lack a later key are `Null`-padded. Number values are
//! captured as original JSON tokens, parsed by `cmdrvl-core::ExactDecimal`, and
//! rendered as exact semantic bytes for the shared key/cell API.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read};
use std::path::Path;

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::{Value, value::RawValue};

use cmdrvl_core::exact_decimal::{DecimalError, ExactDecimal};

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
    let mut lines = Lines::new(
        BufReader::new(File::open(path).map_err(|e| Refusal::io(&e))?),
        bom,
    )?;
    while let Some(object) = lines.next_object()? {
        for (key, _) in &object {
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
    let mut pass2 = Lines::new(
        BufReader::new(File::open(path).map_err(|e| Refusal::io(&e))?),
        bom,
    )?;
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

pub(crate) fn open_bytes(bytes: Vec<u8>) -> Result<Opened, Refusal> {
    let head = &bytes[..bytes.len().min(64 * 1024)];
    let bom = check_text_sample(head)?;
    let mut headers: Vec<Vec<u8>> = Vec::new();
    let mut positions: BTreeMap<String, usize> = BTreeMap::new();
    let mut lines = Lines::new(BufReader::new(Cursor::new(bytes.clone())), bom)?;
    while let Some(object) = lines.next_object()? {
        for (key, _) in &object {
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
    let mut pass2 = Lines::new(BufReader::new(Cursor::new(bytes)), bom)?;
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

/// A JSON object's fields in the order the line wrote them. `serde_json::Map`
/// keeps that order only under `serde_json/preserve_order`, which is a switch for
/// the whole build, so headers do not depend on it. A repeated key keeps its first
/// position and its last value.
type JsonFields = Vec<(String, Box<RawValue>)>;

struct OrderedObject(JsonFields);

impl<'de> Deserialize<'de> for OrderedObject {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Fields;
        impl<'de> Visitor<'de> for Fields {
            type Value = OrderedObject;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields: Vec<(String, Box<RawValue>)> = Vec::new();
                let mut seen: BTreeMap<String, usize> = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, Box<RawValue>>()? {
                    match seen.get(&key) {
                        Some(&at) => fields[at].1 = value,
                        None => {
                            seen.insert(key.clone(), fields.len());
                            fields.push((key, value));
                        }
                    }
                }
                Ok(OrderedObject(fields))
            }
        }
        deserializer.deserialize_map(Fields)
    }
}

/// Line-by-line object reader shared by both passes.
struct Lines<R> {
    reader: R,
    line_no: u64,
    buf: Vec<u8>,
}

impl<R: BufRead> Lines<R> {
    fn new(mut reader: R, bom: usize) -> Result<Self, Refusal> {
        let mut skip = vec![0u8; bom];
        reader.read_exact(&mut skip).map_err(|e| Refusal::io(&e))?;
        Ok(Self {
            reader,
            line_no: 0,
            buf: Vec::new(),
        })
    }

    /// The next JSON object (blank lines skipped), or `None` at end of file.
    fn next_object(&mut self) -> Result<Option<JsonFields>, Refusal> {
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
            let line_no = self.line_no;
            let invalid = |e: serde_json::Error| {
                Refusal::new(RefusalCode::EJsonlParse, "invalid JSON")
                    .with("line", line_no)
                    .with("column", e.column())
            };
            let first = self.buf.iter().find(|byte| !byte.is_ascii_whitespace());
            if first != Some(&b'{') {
                // Malformed text and a well-formed non-object are reported apart.
                serde_json::from_slice::<Value>(&self.buf).map_err(invalid)?;
                return Err(
                    Refusal::new(RefusalCode::EJsonlParse, "line is not a JSON object")
                        .with("line", line_no),
                );
            }
            let OrderedObject(fields) = serde_json::from_slice(&self.buf).map_err(invalid)?;
            return Ok(Some(fields));
        }
    }
}

fn json_cell(value: &RawValue, key: &str, row: u64) -> Result<Cell, Refusal> {
    let unsupported = |kind: &str| {
        Refusal::new(RefusalCode::EUnsupportedValue, "unsupported JSON value")
            .with("kind", kind)
            .with("column", key)
            .with("row", row)
    };
    let raw = value.get();
    match raw.as_bytes().first().copied() {
        Some(b'n') if raw == "null" => Ok(Cell::Null),
        Some(b't') if raw == "true" => Ok(Cell::from_bytes("true")),
        Some(b'f') if raw == "false" => Ok(Cell::from_bytes("false")),
        Some(b'"') => serde_json::from_str::<String>(raw)
            .map(|text| Cell::from_bytes(text.into_bytes()))
            .map_err(|_| {
                Refusal::new(RefusalCode::EJsonlParse, "invalid JSON string")
                    .with("column", key)
                    .with("row", row)
            }),
        Some(b'[') | Some(b'{') => Err(unsupported("nested")),
        Some(_) => ExactDecimal::parse(raw)
            .map(|number| Cell::from_bytes(number.to_key_string()))
            .map_err(|error| decimal_refusal(error, key, row)),
        None => Err(Refusal::new(RefusalCode::EJsonlParse, "invalid JSON value")
            .with("column", key)
            .with("row", row)),
    }
}

fn decimal_refusal(error: DecimalError, key: &str, row: u64) -> Refusal {
    Refusal::new(RefusalCode::EUnsupportedValue, "unsupported JSON number")
        .with("kind", "number")
        .with("reason", decimal_reason(error))
        .with("column", key)
        .with("row", row)
}

const fn decimal_reason(error: DecimalError) -> &'static str {
    match error {
        DecimalError::InvalidToken => "invalid_token",
        DecimalError::TokenTooLong => "token_too_long",
        DecimalError::ExponentOutOfRange => "exponent_out_of_range",
        DecimalError::ScaleOutOfRange => "scale_out_of_range",
        DecimalError::TooManySignificantDigits => "too_many_significant_digits",
        DecimalError::ArithmeticOverflow => "arithmetic_overflow",
        DecimalError::NegativeTolerance => "negative_tolerance",
    }
}
