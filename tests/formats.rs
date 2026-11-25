//! Formats and workbook features: legacy .xls, .xlsm, XLSX formulas (cached results),
//! merged cells, Parquet brotli. Expected text is in `*.expected.json`, written by
//! `tests/fixtures/make_format_fixtures.py` from the values put into the files by
//! independent writers (xlwt, xlsxwriter, openpyxl, pyarrow).

mod common;

use cmdrvl_tabular::{Format, OpenOptions, RefusalCode, open};
use common::*;
use serde_json::json;

#[test]
fn legacy_xls_biff8_reads_numbers_text_dates_and_timestamps() {
    assert_grid_matches("legacy.xls", "legacy.xls.expected.json", &sheet("loans"));
    let table = open(&fx("legacy.xls"), &sheet("loans")).unwrap();
    assert_eq!(
        table.format(),
        Format::Xlsx,
        ".xls is read through the Xlsx path"
    );
}

#[test]
fn xlsm_macro_enabled_workbook_reads_like_xlsx() {
    assert_grid_matches(
        "macro_enabled.xlsm",
        "macro_enabled.xlsm.expected.json",
        &sheet("macro"),
    );
}

#[test]
fn formula_cells_read_their_cached_results() {
    assert_grid_matches("formulas.xlsx", "formulas.xlsx.expected.json", &sheet("f"));
}

#[test]
fn formula_that_cached_an_error_is_refused_not_rendered() {
    let r = refusal("formula_error.xlsx", &sheet("f"));
    assert_eq!(r.code, RefusalCode::EUnsupportedValue);
    assert_eq!(r.context["kind"], json!("error cell"));
    assert_eq!(
        (r.context["row"].clone(), r.context["col"].clone()),
        (json!(2), json!(2))
    );
    assert!(!serde_json::to_string(&r).unwrap().contains("DIV"));
}

#[test]
fn merged_cells_keep_the_value_in_the_top_left_cell_only() {
    // Documented behaviour: the merged-away cells are empty in the file, so they are Null.
    assert_grid_matches(
        "merged_cells.xlsx",
        "merged_cells.xlsx.expected.json",
        &sheet("m"),
    );
}

#[test]
fn parquet_brotli_reads_like_snappy() {
    let mut brotli = open(&fx("brotli.parquet"), &OpenOptions::default()).unwrap();
    let mut snappy = open(&fx("sample.parquet"), &OpenOptions::default()).unwrap();
    assert_eq!(grid(&mut brotli), grid(&mut snappy));
}
