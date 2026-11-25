# cmdrvl-tabular

A small Rust library that reads **CSV, TSV, JSONL, Parquet and XLSX** into one
cell model, so the same dataset gives the same headers and cells whichever file
format it arrives in. It is used by [CMD+RVL](https://cmdrvl.com) tools that
need that guarantee; the design is written up in
[spine-rules/docs/cmdrvl-tabular-design.md](https://github.com/cmdrvl/spine-rules/blob/main/docs/cmdrvl-tabular-design.md).

**Status: pre-1.0 (v0.0.1). The API may change without notice. Not published on
crates.io** (`publish = false`); depend on it by git if you want to try it.

```rust
use cmdrvl_tabular::{open, OpenOptions, SheetSel};

let opts = OpenOptions {
    sheet: Some(SheetSel::parse("loans")),   // required for multi-sheet workbooks
    max_rows: Some(1_000_000),
    ..Default::default()
};
let mut table = open(path, &opts)?;          // Result<Table, Refusal>
let headers = table.headers();               // raw bytes, not normalized
for row in table.rows() {                    // single pass
    let row = row?;
    let key = row.key_components(&[1, 0]);   // raw bytes per column, for composite keys
}
```

## What it does

- Reads five formats: CSV and TSV (delimiter auto-detected or pinned, BOM
  stripped, optional preamble skip), JSONL / NDJSON, Parquet (via the `parquet`
  crate's record API, no Arrow types exposed), and XLSX/XLS (via `calamine`).
- Every cell is `Null` or non-empty `Text(bytes)`. Zero-length input is `Null` in
  every format. `NA`, `NULL` and similar are ordinary text; whitespace is kept.
  Numbers and booleans render to one canonical text form (`2000.0` and `2000`
  both become `2000`), so typed formats match a CSV of the same data.
- Excel workbooks with more than one sheet need an explicit sheet (name or
  0-based index); otherwise `E_AMBIGUOUS_SHEET` lists the sheets.

## Temporal and decimal values

Rendered as fixed, ASCII text so equal values are equal bytes. Temporal values
are text, never numbers (`classify` says `Text`); decimals classify as
`Integer` / `Decimal`.

| Value | Text |
|---|---|
| date | `2024-02-29` (years 0000-9999; outside that refuses) |
| time of day | `12:30:15.250` / `.250000` / `.250000000` by source unit |
| timestamp, UTC-adjusted | `2018-11-02T19:22:09.153Z`; 3, 6 or 9 fraction digits by unit |
| timestamp, not UTC-adjusted | the same without `Z` (a local wall clock) |
| decimal | exact digits, scale preserved: `12.50`, `-0.05`, `0.0000000000` |

- **Parquet:** `DECIMAL` (any precision, any physical type, including 256-bit),
  `DATE`, `TIME` and `TIMESTAMP` in millisecond, microsecond and nanosecond units,
  and `FLOAT16` render. Column types come from the schema, so nanosecond and
  non-UTC timestamps (which the record API hands over as plain integers) are not
  mistaken for numbers. Legacy INT96 timestamps render as UTC milliseconds.
- **XLSX:** datetime cells render as `YYYY-MM-DD` when the serial is a whole day,
  otherwise as a naive millisecond timestamp (`2025-10-13T12:59:02.400`); a
  time-only serial renders `HH:MM:SS.fff`. Both the 1900 and 1904 date systems
  work. Excel cannot tell a date from a midnight datetime, so neither can this
  reader. Refused: serial 60 (Excel's nonexistent 1900-02-29), serials outside
  Excel's 0 to 9999-12-31 range, a time-only serial in a 1904 workbook (it is
  also a valid 1904-01-01 datetime, so the file cannot say which was meant),
  duration cells and error cells. ISO-8601 cells (`t="d"`) render only when they
  carry no time zone.
- **Not checked:** the expected text comes from Python's `datetime` and `decimal`
  on files written by pyarrow and openpyxl. Other writers (Spark, DuckDB,
  Excel itself, LibreOffice) have not been run through it.

## Refusals

`open` returns a typed `Refusal { code, detail, context }` instead of guessing.
There are 15 codes (`E_IO`, `E_ENCODING`, `E_CSV_PARSE`, `E_EMPTY`, `E_HEADERS`,
`E_DIALECT`, `E_TOO_LARGE`, `E_FORMAT`, `E_JSONL_PARSE`, `E_PARQUET_PARSE`,
`E_XLSX_PARSE`, `E_AMBIGUOUS_SHEET`, `E_SHEET_NOT_FOUND`, `E_UNSUPPORTED_VALUE`,
`E_ENCRYPTED`). `E_HEADERS` means no header row; `E_EMPTY` means a header but no
data rows. A refusal carries codes, type names, row/column coordinates, sheet and
header names only: never cell values, library error text or OS error text.

The reader writes no files, keeps no cache, uses no network and reads no
environment variables.

## Known limits (v0)

- **Not streaming.** `Table::rows()` has a streaming-shaped signature, but `open`
  reads the whole table into memory first. Every refusal, including per-value
  ones, comes from `open`; the iterator never yields `Err`. `max_rows` and
  `max_bytes` bound memory, they do not stream. XLSX is O(sheet).
- **Still refused:** Parquet `binary` and nested types (struct, list, map), XLSX
  duration cells and error cells, and the XLSX cases listed under "Temporal and
  decimal values" below. They refuse `E_UNSUPPORTED_VALUE` with the type name and
  coordinates only.
- **The CSV delimiter detector has not been checked against `shape`.** It is a
  re-implementation, written from reading `shape`'s source, of the same scoring
  (candidates `, \t ; | ^`, 64 KiB sample). It was not diffed or run against
  `shape`. Two consequences of that scoring are tested and accepted: a ragged
  record inside the first 200 sampled records makes auto-detection refuse
  `E_DIALECT` (pin `CsvOptions::delimiter` to get the precise `E_CSV_PARSE`), and
  on very small files a quoted embedded newline can make the wrong delimiter win.
- **No zip-bomb guard.** `max_bytes` checks the compressed file size only.
- JSON numbers beyond i64/u64/f64 precision lose digits; carry exact decimals as
  strings.
- The text-encoding check looks at the first 64 KiB: UTF-16/32 BOMs and NUL bytes
  there are refused; later NULs pass through as bytes.

## Not verified

`.xls` (BIFF) and `.xlsm` (no fixtures); Parquet brotli (not compiled in); XLSX
formulas and merged cells; backslash-escaped CSV; the `zstd` codec on a machine
without a C compiler (`--no-default-features` drops it); binary size and build
time.

## Tests

```
cargo test
```

Runs 9 unit tests, 9 cross-format conformance tests, 30 refusal/edge tests and 13
temporal/decimal tests.
The conformance tests read the same dataset as CSV, TSV, JSONL, Parquet and XLSX,
compare each to a hard-coded expected grid, and check composite-key behaviour with
`spine_rules::composite_key` (a git dev-dependency pinned to a commit).

Fixtures in `tests/fixtures/` are small synthetic files, written by independent
libraries (csv, json, pyarrow, openpyxl, msoffcrypto) so the readers are not
validated against their own output. Regenerate them with
`tests/fixtures/make_fixtures.py` (the `uv run` line is in its header). The
temporal and decimal fixtures and their expected text come from
`tests/fixtures/make_temporal_fixtures.py`.

## License

MIT. See [LICENSE](LICENSE).
