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

## Streaming

`open` reads the header and the first data row (the first row decides `E_EMPTY`);
`Table::rows()` then decodes lazily and is single-pass. A refusal found later
arrives as an `Err` item after the rows before it, and the iterator ends after
the first `Err`:

- ragged or malformed CSV records, a nested JSON value, `max_rows` exceeded (the
  row after `max_rows` is the `E_TOO_LARGE` refusal), a bad Parquet page, an
  unsupported Parquet value in a later row, an unsupported XLSX cell in a later row;
- still raised by `open`: unreadable file, encoding, no header, no data rows,
  ambiguous or unknown sheet, `max_bytes`, `max_uncompressed_bytes`, a Parquet
  footer row count over `max_rows`, and anything wrong with the first data row.

JSONL takes two passes. Its headers are the union of keys in first-seen order, so
pass 1 (inside `open`) parses every line to collect them in O(columns) memory, which
also means a malformed JSONL line refuses at `open`; pass 2 yields rows.
Measured once (not a test): a 171 MB, 3,000,000-row CSV read to the end peaked at
2.9 MB resident. Parquet decodes one row group at a time, so its peak depends on
the row-group size; that was not measured.

`SourceInfo.bytes_read` is now `file_size` and `Table::row_count()` is gone: a
stream does not know its length.

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

- **XLSX is not memory-streaming.** CSV/TSV, JSONL and Parquet decode lazily
  (see "Streaming"), but calamine loads the selected sheet, so XLSX memory is
  O(sheet); its cells are still converted one row per `next()`, so an unsupported
  cell refuses on its row, after the rows before it. Legacy `.xls` is the same.
- **Still refused:** Parquet `binary` and nested types (struct, list, map), XLSX
  duration cells and error cells, and the XLSX cases listed under "Temporal and
  decimal values" below. They refuse `E_UNSUPPORTED_VALUE` with the type name and
  coordinates only.
- **CSV dialect: matches `shape` on the cases tried, including shape's quirks.**
  The detector follows shape's flow: a pinned delimiter, then an Excel `sep=<byte>`
  first line (consumed, not a header), then scoring of the first 64 KiB (candidates
  `, \t ; | ^`, first 200 records), with a retry over the whole file when the
  prefix does not decide; a pinned or `sep=` delimiter retries with backslash
  escaping if the header or first record will not parse. It was run against
  `shape`'s real input pipeline (shape 0.9.0, 40587f3) on shape's own 16 CSV
  fixtures plus 3,000 generated files (all five delimiters, quoting, embedded
  newlines, ragged rows, CRLF, BOMs, `sep=` lines, files straddling 64 KiB, stray
  NULs): 0 disagreements on refusal code, delimiter, header count and `E_DIALECT`
  candidates, down from 443 of 3,016 before this was reconciled. The generator
  and comparison harness were a scratch program, not committed; what is committed
  is `tests/fixtures/shape/` with 25 files (16 of shape's own, 9 edge cases) and
  `shape.expected.json`, shape's own outcome for each, which `tests/shape_parity.rs`
  compares to. Two quirks of shape's scoring are therefore also this crate's:
  a ragged record inside the first 200 sampled records makes auto-detection
  refuse `E_DIALECT` (pin `CsvOptions::delimiter` to get the precise
  `E_CSV_PARSE`), and on very small files a quoted embedded newline can make the
  wrong delimiter win. Differences kept on purpose: `skip_rows` (preamble) exists
  only here, and a header-only file is `E_EMPTY` from `open` where shape's input
  layer reports it OK and decides later.
- **Zip-bomb guard is declared-size only.** `max_uncompressed_bytes` (default 2 GiB)
  sums the sizes the XLSX central directory declares. A lying header cannot add
  output: the `zip` crate (9.x) stops an entry at its declared size and errors,
  which refuses as `E_XLSX_PARSE`. That behavior belongs to the dependency, so
  upgrading `zip` should re-run `zip_bomb_lying.xlsx`.
- JSON numbers beyond i64/u64/f64 precision lose digits; carry exact decimals as
  strings.
- The text-encoding check refuses UTF-16/32 BOMs and NUL bytes within the first
  8 KiB after the BOM (shape's window); later NULs pass through as bytes.

## Not verified

The `zstd` codec on a machine without a C compiler (`--no-default-features`
drops it); binary size and build time; files from writers other than the ones
named in "Tests" (Spark, DuckDB, Excel, LibreOffice); `.xls` files beyond one
small xlwt-written sheet (no multi-sheet, formula or merged-cell BIFF fixtures);
XLSX formulas whose cached value is missing (openpyxl writes none, calamine
returns empty: not tested here).

## Workbook features covered by fixtures

`.xls` (BIFF8, xlwt: numbers, text, dates, timestamps), `.xlsm` (macro-enabled
content type plus a `vbaProject` part), XLSX formulas (xlsxwriter writes the
cached result; the cached value is what is read, an error result refuses
`E_UNSUPPORTED_VALUE` as an error cell), merged cells (the value stays in the
top-left cell, the merged-away cells read as `Null`), Parquet brotli (the
`brotli` feature of the `parquet` crate is enabled), and backslash-escaped CSV
(see the `shape` parity fixtures). Generated by `tests/fixtures/make_format_fixtures.py`.

## Tests

```
cargo test
```

Runs 9 unit tests, 9 cross-format conformance tests, 35 refusal/edge tests, 4 shape-parity tests, 8
streaming tests, 13 temporal/decimal tests and 6 format/workbook-feature tests.
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
