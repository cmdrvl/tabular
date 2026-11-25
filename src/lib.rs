//! `cmdrvl-tabular`: one deterministic reader for CSV, TSV, JSONL, Parquet and
//! XLSX that yields identical cells for the same dataset in any container.
//!
//! Design: `spine-rules/docs/cmdrvl-tabular-design.md`.
//!
//! **Streaming.** [`open`] reads the header and the first data row, then hands
//! back a [`Table`] whose [`Table::rows`] is a single-pass iterator of
//! `Result<Row, Refusal>`. CSV/TSV, JSONL and Parquet decode one row at a time
//! (Parquet one row group at a time), so a refusal found deep in the file
//! (ragged CSV record, nested JSON value, `max_rows` exceeded, a bad Parquet
//! page) arrives as an `Err` from the iterator after the earlier rows were
//! yielded; the iterator ends after the first `Err`. Refusals about the file as a
//! whole (unreadable, wrong encoding, no header, no data rows, ambiguous sheet,
//! the first row) still come from `open`. XLSX is not memory-streaming: calamine
//! loads the selected sheet, so memory is O(sheet), but its cells are still
//! converted lazily, one row per `next()`.

mod cell;
mod csv_reader;
mod detect;
mod jsonl_reader;
mod parquet_reader;
mod refusal;
mod temporal;
mod xlsx_reader;

use std::path::Path;

pub use cell::{Cell, CellClass};
pub use refusal::{Refusal, RefusalCode};

/// Default decompressed-size cap for XLSX packages (2 GiB). `max_bytes` bounds the
/// compressed file; this bounds what it expands to.
pub const DEFAULT_MAX_UNCOMPRESSED_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Supported containers. `.ndjson` is `Jsonl`; `.xls` and `.xlsm` are `Xlsx`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Csv,
    Tsv,
    Jsonl,
    Parquet,
    Xlsx,
}

impl Format {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Jsonl => "jsonl",
            Self::Parquet => "parquet",
            Self::Xlsx => "xlsx",
        }
    }
}

/// Worksheet selector for `.xlsx` / `.xls`: an exact name or a 0-based index.
/// A name that matches a sheet always wins over an index reading of the same text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SheetSel {
    Name(String),
    Index(usize),
}

impl SheetSel {
    /// For `--sheet <name|index>`: digits become `Index`, anything else `Name`.
    /// `open` still prefers an exact sheet-name match for digit-only text.
    pub fn parse(text: &str) -> Self {
        text.parse::<usize>()
            .map_or_else(|_| Self::Name(text.to_owned()), Self::Index)
    }
}

#[derive(Clone, Debug, Default)]
pub struct CsvOptions {
    /// `None` auto-detects (`,` `\t` `;` `|` `^`). `.tsv` pins `\t` unless set.
    pub delimiter: Option<u8>,
    /// Physical lines to discard before the header, for preambles.
    pub skip_rows: usize,
}

#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    /// Override the extension sniff. Magic bytes are still checked.
    pub format: Option<Format>,
    pub sheet: Option<SheetSel>,
    /// Data rows, excluding the header. Exceeding it refuses `E_TOO_LARGE`.
    pub max_rows: Option<u64>,
    /// Input file size on disk. Exceeding it refuses `E_TOO_LARGE`.
    pub max_bytes: Option<u64>,
    /// XLSX/XLSM only: total decompressed size of the workbook package. `None` uses
    /// [`DEFAULT_MAX_UNCOMPRESSED_BYTES`]; pass `Some(u64::MAX)` to lift the guard.
    /// Exceeding it refuses `E_TOO_LARGE` (`limit: "max_uncompressed_bytes"`).
    pub max_uncompressed_bytes: Option<u64>,
    pub csv: CsvOptions,
}

/// One data row. `index` is 1-based among data rows (header excluded).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    pub index: u64,
    pub cells: Vec<Cell>,
}

impl Row {
    /// Raw composite-key components for the given columns, in order, in the
    /// shape `spine_rules::composite_key` consumes (`Null` becomes an empty
    /// component). `None` when a column index is out of range.
    pub fn key_components(&self, columns: &[usize]) -> Option<Vec<Vec<u8>>> {
        columns
            .iter()
            .map(|&column| self.cells.get(column).map(|cell| cell.as_bytes().to_vec()))
            .collect()
    }
}

/// What `open` resolved. `file_size` is the input's size on disk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceInfo {
    pub delimiter: Option<u8>,
    pub sheet: Option<String>,
    pub file_size: u64,
}

/// A single-pass stream of data rows. Readers return one; `open` wraps it.
pub(crate) type RowStream = Box<dyn Iterator<Item = Result<Row, Refusal>>>;

/// What every reader hands back: headers, a lazy row stream, and what it resolved.
pub(crate) struct Opened {
    pub headers: Vec<Vec<u8>>,
    pub rows: RowStream,
    pub delimiter: Option<u8>,
    pub sheet: Option<String>,
}

pub struct Table {
    format: Format,
    headers: Vec<Vec<u8>>,
    rows: Option<RowStream>,
    source: SourceInfo,
}

impl std::fmt::Debug for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Table")
            .field("format", &self.format)
            .field("headers", &self.headers.len())
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl Table {
    /// Raw header bytes. Normalization (empty or duplicate headers) stays in the tools.
    pub fn headers(&self) -> &[Vec<u8>] {
        &self.headers
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn source(&self) -> &SourceInfo {
        &self.source
    }

    /// Single-pass iterator over the data rows. Each row has exactly
    /// `headers().len()` cells. A second call yields nothing: the stream is
    /// consumed. The iterator ends after the first `Err` (a refusal found after
    /// earlier rows were already yielded).
    pub fn rows(&mut self) -> impl Iterator<Item = Result<Row, Refusal>> + '_ {
        let mut stream = self.rows.take();
        std::iter::from_fn(move || {
            let item = stream.as_mut()?.next();
            if !matches!(item, Some(Ok(_))) {
                stream = None;
            }
            item
        })
    }
}

/// Open `path` as a table. Pure function of the file bytes and `opts`: no files
/// written, no temp files, no cache, no network, no environment reads.
pub fn open(path: &Path, opts: &OpenOptions) -> Result<Table, Refusal> {
    let size = std::fs::metadata(path).map_err(|e| Refusal::io(&e))?.len();
    if let Some(max) = opts.max_bytes
        && size > max
    {
        return Err(
            Refusal::new(RefusalCode::ETooLarge, "input exceeds max_bytes")
                .with("limit", "max_bytes")
                .with("max", max)
                .with("actual", size),
        );
    }
    let format = detect::resolve(path, opts.format)?;
    let opened = match format {
        Format::Csv | Format::Tsv => csv_reader::open(path, format, &opts.csv)?,
        Format::Jsonl => jsonl_reader::open(path)?,
        Format::Parquet => parquet_reader::open(path, opts.max_rows)?,
        Format::Xlsx => xlsx_reader::open(
            path,
            opts.sheet.as_ref(),
            opts.max_uncompressed_bytes
                .unwrap_or(DEFAULT_MAX_UNCOMPRESSED_BYTES),
        )?,
    };
    let Opened {
        headers,
        rows,
        delimiter,
        sheet,
    } = opened;
    let mut rows = limited(rows, opts.max_rows);
    // The first data row decides E_EMPTY, so it is read here; later rows are lazy.
    let rows: RowStream = match rows.next() {
        None => return Err(no_data_rows()),
        Some(Err(refusal)) => return Err(refusal),
        Some(Ok(first)) => Box::new(std::iter::once(Ok(first)).chain(rows)),
    };
    Ok(Table {
        format,
        headers,
        rows: Some(rows),
        source: SourceInfo {
            delimiter,
            sheet,
            file_size: size,
        },
    })
}

/// Enforce `max_rows` on a stream: row `max + 1` becomes an `E_TOO_LARGE` refusal
/// (and the stream ends there) instead of being yielded.
fn limited(rows: RowStream, max_rows: Option<u64>) -> RowStream {
    let Some(max) = max_rows else {
        return rows;
    };
    let mut rows = rows;
    let mut count = 0u64;
    let mut done = false;
    Box::new(std::iter::from_fn(move || {
        if done {
            return None;
        }
        let item = rows.next()?;
        count += 1;
        if item.is_ok() && count > max {
            done = true;
            return Some(Err(too_many_rows(max)));
        }
        if item.is_err() {
            done = true;
        }
        Some(item)
    }))
}

pub(crate) fn too_many_rows(max: u64) -> Refusal {
    Refusal::new(RefusalCode::ETooLarge, "input exceeds max_rows")
        .with("limit", "max_rows")
        .with("max", max)
}

pub(crate) fn missing_headers() -> Refusal {
    Refusal::new(RefusalCode::EHeaders, "no header row").with("issue", "missing")
}

pub(crate) fn no_data_rows() -> Refusal {
    Refusal::new(RefusalCode::EEmpty, "header present but no data rows")
}
