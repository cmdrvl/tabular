//! `cmdrvl-tabular`: one deterministic reader for CSV, TSV, JSONL, Parquet and
//! XLSX that yields identical cells for the same dataset in any container.
//!
//! Design: `spine-rules/docs/cmdrvl-tabular-design.md`.
//!
//! **v0 limit, stated plainly:** [`Table::rows`] has a streaming-shaped signature
//! (a single-pass iterator of `Result<Row, Refusal>`), but v0 materializes the
//! whole table inside [`open`]. Every refusal, including per-value ones, is
//! therefore raised by `open`, never by the iterator. Memory is O(table).
//! `max_rows` / `max_bytes` bound that cost; they do not make it streaming.

mod cell;
mod csv_reader;
mod detect;
mod jsonl_reader;
mod parquet_reader;
mod refusal;
mod xlsx_reader;

use std::path::Path;

pub use cell::{Cell, CellClass};
pub use refusal::{Refusal, RefusalCode};

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

/// What `open` resolved. `bytes_read` is the file size: v0 readers consume the whole file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceInfo {
    pub delimiter: Option<u8>,
    pub sheet: Option<String>,
    pub bytes_read: u64,
}

#[derive(Debug)]
pub struct Table {
    format: Format,
    headers: Vec<Vec<u8>>,
    rows: Vec<Row>,
    source: SourceInfo,
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

    /// Number of data rows (known because v0 materializes).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Single-pass iterator over the data rows. Each row has exactly
    /// `headers().len()` cells. A second call yields nothing: rows are drained.
    /// v0 never yields `Err` (see the crate docs).
    pub fn rows(&mut self) -> impl Iterator<Item = Result<Row, Refusal>> + '_ {
        std::mem::take(&mut self.rows).into_iter().map(Ok)
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
    let max_rows = opts.max_rows;
    let (headers, rows, delimiter, sheet) = match format {
        Format::Csv | Format::Tsv => {
            let ((h, r), d) = csv_reader::read(path, format, &opts.csv, max_rows)?;
            (h, r, Some(d), None)
        }
        Format::Jsonl => {
            let (h, r) = jsonl_reader::read(path, max_rows)?;
            (h, r, None, None)
        }
        Format::Parquet => {
            let (h, r) = parquet_reader::read(path, max_rows)?;
            (h, r, None, None)
        }
        Format::Xlsx => {
            let ((h, r), s) = xlsx_reader::read(path, opts.sheet.as_ref(), max_rows)?;
            (h, r, None, Some(s))
        }
    };
    Ok(Table {
        format,
        headers,
        rows,
        source: SourceInfo {
            delimiter,
            sheet,
            bytes_read: size,
        },
    })
}

/// Shared row-limit check used by every reader before it stores row `count`.
pub(crate) fn check_row_limit(count: u64, max_rows: Option<u64>) -> Result<(), Refusal> {
    match max_rows {
        Some(max) if count > max => Err(Refusal::new(
            RefusalCode::ETooLarge,
            "input exceeds max_rows",
        )
        .with("limit", "max_rows")
        .with("max", max)),
        _ => Ok(()),
    }
}

/// Headers plus data rows, as every reader returns them.
pub(crate) type Grid = (Vec<Vec<u8>>, Vec<Row>);

pub(crate) fn missing_headers() -> Refusal {
    Refusal::new(RefusalCode::EHeaders, "no header row").with("issue", "missing")
}

pub(crate) fn no_data_rows() -> Refusal {
    Refusal::new(RefusalCode::EEmpty, "header present but no data rows")
}
