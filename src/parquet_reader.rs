//! Parquet reader over the record API (no arrow). Renders integers, floats,
//! booleans, strings, decimals, dates, times, timestamps (the canonical text is
//! defined in `temporal.rs`) and float16; binary and nested types refuse
//! `E_UNSUPPORTED_VALUE`.
//!
//! Column logical types come from the schema, not from the record API's field
//! variants: the record API only maps UTC-adjusted millisecond/microsecond
//! timestamps, so a nanosecond or non-UTC (naive) timestamp would otherwise
//! surface as a bare integer and render as a number, silently.

use std::fs::File;
use std::path::Path;

use bytes::Bytes;
use parquet::basic::{LogicalType, TimeUnit};
use parquet::file::reader::{ChunkReader, FileReader, SerializedFileReader};
use parquet::record::{Field, reader::RowIter};

use crate::cell::{canon_f32, canon_f64};
use crate::refusal::{Refusal, RefusalCode};
use crate::temporal::{Unit, date_text, decimal_text, time_text, timestamp_text};
use crate::{Cell, Opened, Row, missing_headers, too_many_rows};

fn parse_refusal(detail: &str) -> Refusal {
    // Fixed text only: library errors are not forwarded (they could quote data).
    Refusal::new(RefusalCode::EParquetParse, detail)
}

/// What a top-level column's schema says about how to read its values.
#[derive(Clone, Copy, Debug)]
enum ColSpec {
    Plain,
    Timestamp { utc: bool, unit: Unit },
    Time { unit: Unit },
}

fn unit_of(unit: &TimeUnit) -> Unit {
    match unit {
        TimeUnit::MILLIS => Unit::Millis,
        TimeUnit::MICROS => Unit::Micros,
        TimeUnit::NANOS => Unit::Nanos,
    }
}

fn spec_of(field: &parquet::schema::types::Type) -> ColSpec {
    match field.get_basic_info().logical_type_ref() {
        Some(LogicalType::Timestamp(t)) => ColSpec::Timestamp {
            utc: t.is_adjusted_to_u_t_c,
            unit: unit_of(&t.unit),
        },
        Some(LogicalType::Time(t)) => ColSpec::Time {
            unit: unit_of(&t.unit),
        },
        _ => ColSpec::Plain,
    }
}

pub(crate) fn open(path: &Path, max_rows: Option<u64>) -> Result<Opened, Refusal> {
    let file = File::open(path).map_err(|e| Refusal::io(&e))?;
    let reader = SerializedFileReader::new(file)
        .map_err(|_| parse_refusal("not a readable Parquet file"))?;
    open_reader(reader, max_rows)
}

pub(crate) fn open_bytes(bytes: Vec<u8>, max_rows: Option<u64>) -> Result<Opened, Refusal> {
    let reader = SerializedFileReader::new(Bytes::from(bytes))
        .map_err(|_| parse_refusal("not a readable Parquet file"))?;
    open_reader(reader, max_rows)
}

fn open_reader<R: ChunkReader + 'static>(
    reader: SerializedFileReader<R>,
    max_rows: Option<u64>,
) -> Result<Opened, Refusal> {
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
    let specs: Vec<ColSpec> = meta
        .schema()
        .get_fields()
        .iter()
        .map(|f| spec_of(f))
        .collect();
    // Refuse on the footer's row count before decoding a single page.
    if let Some(max) = max_rows
        && meta.num_rows() > 0
        && meta.num_rows() as u64 > max
    {
        return Err(too_many_rows(max));
    }

    // Owning row iterator: decodes one row group at a time.
    let iter = RowIter::from_file_into(Box::new(reader));
    let mut index = 0u64;
    let rows = iter.map(move |row| {
        let row = row.map_err(|_| parse_refusal("cannot decode Parquet page"))?;
        index += 1;
        let mut cells = Vec::with_capacity(specs.len());
        for (position, (name, field)) in row.get_column_iter().enumerate() {
            let spec = specs.get(position).copied().unwrap_or(ColSpec::Plain);
            cells.push(cell(field, spec, name, index)?);
        }
        Ok(Row { index, cells })
    });
    Ok(Opened {
        headers,
        rows: Box::new(rows),
        delimiter: None,
        sheet: None,
    })
}

fn cell(field: &Field, spec: ColSpec, column: &str, row: u64) -> Result<Cell, Refusal> {
    let text = |s: String| Cell::from_bytes(s);
    let out_of_range = |kind: &str| unsupported(kind, column, row);
    // Schema-declared temporal columns first (covers nanoseconds and non-UTC
    // timestamps, which the record API hands over as plain integers).
    match (spec, field) {
        (ColSpec::Timestamp { utc, unit }, Field::Long(v))
        | (ColSpec::Timestamp { utc, unit }, Field::TimestampMillis(v))
        | (ColSpec::Timestamp { utc, unit }, Field::TimestampMicros(v)) => {
            return timestamp_text(*v, unit, utc)
                .map(text)
                .ok_or_else(|| out_of_range("timestamp out of range"));
        }
        (ColSpec::Time { unit }, Field::Long(v)) => {
            return time_text(*v, unit)
                .map(text)
                .ok_or_else(|| out_of_range("time out of range"));
        }
        (ColSpec::Time { unit }, Field::Int(v)) => {
            return time_text(i64::from(*v), unit)
                .map(text)
                .ok_or_else(|| out_of_range("time out of range"));
        }
        _ => {}
    }
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
        Field::Float16(v) => text(
            canon_f32(f32::from(*v)).ok_or_else(|| unsupported("non-finite float", column, row))?,
        ),
        Field::Str(v) => Cell::from_bytes(v.as_bytes()),
        Field::Date(days) => date_text(i64::from(*days))
            .map(text)
            .ok_or_else(|| out_of_range("date out of range"))?,
        Field::Decimal(d) => decimal_text(d.data(), d.scale())
            .map(text)
            .ok_or_else(|| out_of_range("decimal"))?,
        Field::TimeMillis(v) => time_text(i64::from(*v), Unit::Millis)
            .map(text)
            .ok_or_else(|| out_of_range("time out of range"))?,
        Field::TimeMicros(v) => time_text(*v, Unit::Micros)
            .map(text)
            .ok_or_else(|| out_of_range("time out of range"))?,
        // UTC-adjusted ms/us timestamps whose schema had no logical type (converted type only).
        Field::TimestampMillis(v) => timestamp_text(*v, Unit::Millis, true)
            .map(text)
            .ok_or_else(|| out_of_range("timestamp out of range"))?,
        Field::TimestampMicros(v) => timestamp_text(*v, Unit::Micros, true)
            .map(text)
            .ok_or_else(|| out_of_range("timestamp out of range"))?,
        other => return Err(unsupported(kind(other), column, row)),
    })
}

/// Type name only: refusal context must never carry cell values.
fn kind(field: &Field) -> &'static str {
    match field {
        Field::Bytes(_) => "binary",
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
