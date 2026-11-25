//! v1 temporal and decimal rendering. Expected text comes from
//! `tests/fixtures/*.expected.json`, produced by Python's datetime/decimal
//! (independent of this crate's arithmetic); the fixtures are written by
//! pyarrow and openpyxl. Regenerate with `make_temporal_fixtures.py`.

mod common;

use cmdrvl_tabular::{Cell, CellClass, OpenOptions, RefusalCode, open};
use common::*;
use serde_json::json;

// ---- Parquet --------------------------------------------------------------

#[test]
fn parquet_temporal_and_decimal_columns_render_canonically() {
    assert_grid_matches(
        "temporal.parquet",
        "temporal.parquet.expected.json",
        &OpenOptions::default(),
    );
}

#[test]
fn parquet_spot_values_are_what_the_design_says() {
    let mut table = open(&fx("temporal.parquet"), &OpenOptions::default()).unwrap();
    let (_, rows) = grid(&mut table);
    let h = ["id", "d", "ts_ms_utc"];
    let _ = h;
    // Row 1, hand-checked: date, UTC ms, naive ns, decimal(10,2), decimal(5,0).
    assert_eq!(rows[0][1], t("2024-02-29"));
    assert_eq!(rows[0][2], t("2018-11-02T18:42:09.153Z"));
    assert_eq!(rows[0][7], t("2018-11-02T18:42:09.153000000"));
    assert_eq!(rows[0][11], t("12.50"));
    assert_eq!(rows[0][12], t("42"));
    // Row 2: pre-epoch, negative decimals keep their sign and scale.
    assert_eq!(rows[1][1], t("1969-12-31"));
    assert_eq!(rows[1][2], t("1969-12-31T23:59:59.999Z"));
    assert_eq!(rows[1][11], t("-0.05"));
    // Row 3: nulls stay Null in every temporal/decimal column.
    assert!(rows[2][1..].iter().all(Cell::is_null));
    // Row 4: year 0001 date; zero decimal keeps scale and never prints "-0".
    assert_eq!(rows[3][1], t("0001-01-01"));
    assert_eq!(rows[3][13], t("0.0000000000"));
}

#[test]
fn rendered_decimals_and_dates_classify_as_expected() {
    let mut table = open(&fx("temporal.parquet"), &OpenOptions::default()).unwrap();
    let (_, rows) = grid(&mut table);
    assert_eq!(rows[0][11].classify(), CellClass::Decimal); // 12.50
    assert_eq!(rows[0][12].classify(), CellClass::Integer); // 42
    assert_eq!(rows[0][1].classify(), CellClass::Text); // dates are text, never numbers
}

#[test]
fn parquet_binary_is_still_refused_without_the_value() {
    let r = refusal("binary.parquet", &OpenOptions::default());
    assert_eq!(r.code, RefusalCode::EUnsupportedValue);
    assert_eq!(
        (r.context["kind"].clone(), r.context["column"].clone()),
        (json!("binary"), json!("blob"))
    );
    let rendered = serde_json::to_string(&r).unwrap();
    assert!(!rendered.contains("secret"), "{rendered}");
}

#[test]
fn parquet_single_date_column_renders_instead_of_refusing() {
    let mut table = open(&fx("unsupported_date.parquet"), &OpenOptions::default()).unwrap();
    let (headers, rows) = grid(&mut table);
    assert_eq!(headers, vec![b"id".to_vec(), b"closed".to_vec()]);
    assert_eq!(rows, vec![vec![t("1"), t("2024-05-17")]]);
}

// ---- XLSX -----------------------------------------------------------------

#[test]
fn xlsx_dates_datetimes_and_times_render_canonically() {
    assert_grid_matches("temporal.xlsx", "temporal.xlsx.expected.json", &sheet("t"));
}

#[test]
fn xlsx_1904_date_system_renders_the_same_dates() {
    assert_grid_matches(
        "temporal_1904.xlsx",
        "temporal_1904.xlsx.expected.json",
        &sheet("t"),
    );
}

#[test]
fn xlsx_single_datetime_renders_instead_of_refusing() {
    let mut table = open(&fx("unsupported_datetime.xlsx"), &OpenOptions::default()).unwrap();
    let (_, rows) = grid(&mut table);
    assert_eq!(rows, vec![vec![t("1"), t("2031-07-09")]]);
}

#[test]
fn xlsx_midnight_datetime_and_date_are_the_same_text() {
    // Documented loss: Excel stores both as a whole-number serial, so the file
    // cannot distinguish them and neither can the reader.
    let mut table = open(&fx("temporal.xlsx"), &sheet("t")).unwrap();
    let (_, rows) = grid(&mut table);
    assert_eq!(rows[1][2], t("1900-03-01"));
}

fn assert_xlsx_refusal(file: &str, kind: &str, row: u64, col: u64, leak: &str) {
    let r = refusal(file, &sheet("t"));
    assert_eq!(r.code, RefusalCode::EUnsupportedValue, "{file}");
    assert_eq!(r.context["kind"], json!(kind), "{file}");
    assert_eq!(
        (r.context["row"].clone(), r.context["col"].clone()),
        (json!(row), json!(col)),
        "{file}"
    );
    let rendered = serde_json::to_string(&r).unwrap();
    assert!(!rendered.contains(leak), "{file} leaked {leak}: {rendered}");
}

#[test]
fn xlsx_nonexistent_1900_02_29_is_refused() {
    assert_xlsx_refusal(
        "excel_1900_leap_bug.xlsx",
        "nonexistent 1900 leap day",
        2,
        2,
        "1900-02",
    );
}

#[test]
fn xlsx_serial_beyond_9999_is_refused() {
    assert_xlsx_refusal(
        "excel_serial_out_of_range.xlsx",
        "datetime out of range",
        2,
        2,
        "3000000",
    );
}

#[test]
fn xlsx_1904_time_of_day_is_ambiguous_and_refused() {
    assert_xlsx_refusal(
        "temporal_1904_time.xlsx",
        "ambiguous 1904 serial below 1",
        2,
        2,
        "12:00",
    );
}

#[test]
fn xlsx_duration_cells_stay_refused() {
    assert_xlsx_refusal("excel_duration.xlsx", "duration", 2, 2, "36");
}
