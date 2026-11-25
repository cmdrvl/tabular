# cmdrvl-tabular

**Local skeleton, pending the operator's repo decision.** This directory is a
git repo with no remote. It has not been pushed anywhere or published, and
`publish = false` is set. Design: `spine-rules/docs/cmdrvl-tabular-design.md`
(bd-e0g).

One deterministic reader for CSV, TSV, JSONL, Parquet and XLSX. The same
dataset in any of the five containers yields identical headers and cells.

```rust
use cmdrvl_tabular::{open, OpenOptions, SheetSel};

let opts = OpenOptions { sheet: Some(SheetSel::parse("loans")), max_rows: Some(1_000_000), ..Default::default() };
let mut table = open(path, &opts)?;               // Result<Table, Refusal>
let headers = table.headers();                    // raw bytes
for row in table.rows() {                         // single pass
    let row = row?;
    let key = row.key_components(&[1, 0]);        // hand-off to spine_rules::composite_key
}
```

## What v0 does not do (plainly)

- **Not streaming.** `Table::rows()` has a streaming-shaped signature, but `open`
  materializes the whole table. Every refusal, including per-value ones, comes
  from `open`; the iterator never yields `Err` in v0. Memory is O(table);
  `max_rows` and `max_bytes` bound it, they do not stream. XLSX is O(sheet)
  because calamine's `worksheet_range` materializes the sheet.
- **Temporal and decimal types are refused (v1 gap, known and sequenced).**
  Parquet `decimal`, `date`, `timestamp`, `binary`, `float16`, nested types,
  and XLSX datetime/duration/error cells all refuse `E_UNSUPPORTED_VALUE` with the
  type name and coordinates. v1 renders them (dates `YYYY-MM-DD`, timestamps
  RFC 3339 UTC, decimals exact, XLSX dates only for true Excel dates). v1 lands
  after the flagship pair (shape, rvl) adopts v0 and before verify, benchmark
  and canon, whose real inputs are typed. Refusing beats guessing a date format.
- **No zip-bomb guard.** `max_bytes` checks the compressed file size only.
- **JSON numbers** beyond i64/u64/f64 precision lose digits; carry exact decimals as strings.
- **Encoding check is sample-based:** UTF-16/32 BOMs and NUL bytes are refused
  in the first 64 KiB; later NULs pass through as bytes.

## CSV delimiter detection: known limits

The detector re-implements `shape`'s scoring from reading its source (candidates
`, \t ; | ^`, 64 KiB sample cut at the last newline, ordering by records parsed,
then mode count, then mode fields). It has **not** been diffed or run against
`shape`. Two behaviors follow from that scoring and are pinned or avoided in tests:

- A ragged record inside the first 200 sampled records drops that delimiter's
  probe, so detection refuses `E_DIALECT`. Pin `CsvOptions::delimiter` to get the
  precise `E_CSV_PARSE` with line and field counts.
- On very small files, a quoted embedded newline can make a one-field probe parse
  more records than the right delimiter and win. Pin the delimiter.

## Rules

- `Cell::Text` is never empty: zero-length input is `Null` in every format.
- `NA`, `NULL`, `N/A` are ordinary text. Whitespace is preserved; trimming is the key layer's job.
- Refusal `detail` and `context` carry codes, type names, coordinates, sheet
  and header names only. No cell values, no library error text, no OS error text.
- No files written, no temp files, no cache, no network, no env reads.
- A name that matches a sheet beats an index reading of the same text. Multi-sheet
  workbook without a sheet: `E_AMBIGUOUS_SHEET` listing every sheet (hidden included).
- 15 refusal codes: shape's seven (`E_IO E_ENCODING E_CSV_PARSE E_EMPTY E_HEADERS E_DIALECT E_TOO_LARGE`)
  plus `E_FORMAT E_JSONL_PARSE E_PARQUET_PARSE E_XLSX_PARSE E_AMBIGUOUS_SHEET E_SHEET_NOT_FOUND E_UNSUPPORTED_VALUE E_ENCRYPTED`.
  `E_HEADERS` = no header row at all; `E_EMPTY` = header but no data rows (as in shape).

## Tests

```
cargo test        # 3 unit, 9 cross-format conformance, 32 refusal/edge tests
```

`tests/conformance.rs` reads the same dataset as CSV, TSV, JSONL, Parquet and XLSX
and compares each to a hard-coded golden grid, then runs the cells through
`spine_rules::composite_key` (local path dev-dependency on `../spine-rules`).
Fixtures in `tests/fixtures/` are written by independent libraries (csv, json,
pyarrow, openpyxl, msoffcrypto); regenerate with `make_fixtures.py` (header comment
has the `uv run` line).

## Not verified

`.xls` (BIFF) and `.xlsm` (no fixture); `.ndjson` beyond one temp-file test; Parquet
brotli (not compiled in); `zstd` on a build without a C compiler; XLSX
formulas/merged cells; backslash-escaped CSV; detector parity against `shape`;
`fingerprint`'s calamine 0.26 code against 0.38; release binary size and build time.
