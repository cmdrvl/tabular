//! Parquet reader over the record API (no arrow). v0 renders integers, floats,
//! booleans and strings; decimal, date, timestamp, binary, float16 and nested
//! types refuse `E_UNSUPPORTED_VALUE` (the v1 temporal gap, see README).

use std::fs::File;
use std::path::Path;

use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::record::Field;

use crate::cell::{canon_f32, canon_f64};
use crate::refusal::{Refusal, RefusalCode};
use crate::{Cell, Row, check_row_limit, missing_headers, no_data_rows};

fn parse_refusal(detail: &str) -> Refusal {
    // Fixed text only: library errors are not forwarded (they could quote data).
    Refusal::new(RefusalCode::EParquetParse, detail)
}

pub(crate) fn read(
    path: &Path,
    max_rows: Option<u64>,
) -> Result<(Vec<Vec<u8>>, Vec<Row>), Refusal> {
    let file = File::open(path).map_err(|e| Refusal::io(&e))?;
    let reader = SerializedFileReader::new(file)
        .map_err(|_| parse_refusal("not a readable Parquet file"))?;
    let meta = reader.metadata().file_metadata();
    let headers: Vec<Vec<u8>> = meta
        .schema()
        .get_fields()
        .iter()
        .map(|field| field.name().as_bytes().to_vec())
        .collect();
    if headers.is_empty() {
        return Err(missing_headers());
    }
    // Refuse on the footer's row count before decoding a single page.
    if let Some(max) = max_rows
        && meta.num_rows() > 0
        && meta.num_rows() as u64 > max
    {
        check_row_limit(max.saturating_add(1), Some(max))?;
    }

    let iter = reader
        .get_row_iter(None)
        .map_err(|_| parse_refusal("cannot decode Parquet row groups"))?;
    let mut rows = Vec::new();
    for row in iter {
        let row = row.map_err(|_| parse_refusal("cannot decode Parquet page"))?;
        let index = rows.len() as u64 + 1;
        check_row_limit(index, max_rows)?;
        let mut cells = Vec::with_capacity(headers.len());
        for (name, field) in row.get_column_iter() {
            cells.push(cell(field, name, index)?);
        }
        rows.push(Row { index, cells });
    }
    if rows.is_empty() {
        return Err(no_data_rows());
    }
    Ok((headers, rows))
}

fn cell(field: &Field, column: &str, row: u64) -> Result<Cell, Refusal> {
    let text = |s: String| Cell::from_bytes(s);
    Ok(match field {
        Field::Null => Cell::Null,
        Field::Bool(v) => text(v.to_string()),
        Field::Byte(v) => text(v.to_string()),
        Field::Short(v) => text(v.to_string()),
        Field::Int(v) => text(v.to_string()),
        Field::Long(v) => text(v.to_string()),
        Field::UByte(v) => text(v.to_string()),
        Field::UShort(v) => text(v.to_string()),
        Field::UInt(v) => text(v.to_string()),
        Field::ULong(v) => text(v.to_string()),
        Field::Float(v) => {
            text(canon_f32(*v).ok_or_else(|| unsupported("non-finite float", column, row))?)
        }
        Field::Double(v) => {
            text(canon_f64(*v).ok_or_else(|| unsupported("non-finite float", column, row))?)
        }
        Field::Str(v) => Cell::from_bytes(v.as_bytes()),
        other => return Err(unsupported(kind(other), column, row)),
    })
}

/// Type name only: refusal context must never carry cell values.
fn kind(field: &Field) -> &'static str {
    match field {
        Field::Decimal(_) => "decimal",
        Field::Bytes(_) => "binary",
        Field::Date(_) => "date",
        Field::TimestampMillis(_) | Field::TimestampMicros(_) => "timestamp",
        Field::Float16(_) => "float16",
        Field::Group(_) => "struct",
        Field::ListInternal(_) => "list",
        Field::MapInternal(_) => "map",
        _ => "unsupported type",
    }
}

fn unsupported(kind: &str, column: &str, row: u64) -> Refusal {
    Refusal::new(RefusalCode::EUnsupportedValue, "unsupported Parquet type")
        .with("kind", kind)
        .with("column", column)
        .with("row", row)
}
