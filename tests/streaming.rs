//! Streaming contract: rows are produced lazily, a refusal deep in the file
//! arrives as `Err` from the iterator after the earlier rows, and the iterator
//! ends after that first `Err`.

mod common;

use cmdrvl_tabular::{CsvOptions, OpenOptions, RefusalCode, Row, open};
use common::*;
use serde_json::json;

fn comma() -> OpenOptions {
    OpenOptions {
        csv: CsvOptions {
            delimiter: Some(b','),
            skip_rows: 0,
        },
        ..OpenOptions::default()
    }
}

#[test]
fn csv_open_does_not_scan_the_file_and_the_bad_record_arrives_last() {
    // 50_000 good records, then a ragged one. A reader that materialized the table
    // in `open` would refuse here instead of returning a Table.
    let mut body = String::from("a,b\n");
    for i in 0..50_000 {
        body.push_str(&format!("{i},x\n"));
    }
    body.push_str("solo\n");
    let (_d, p) = temp("late.csv", body.as_bytes());

    let mut table = open(&p, &OpenOptions::default()).expect("open must not read the whole file");
    let mut ok = 0u64;
    let mut failure = None;
    for item in table.rows() {
        match item {
            Ok(row) => {
                ok += 1;
                assert_eq!(row.index, ok);
            }
            Err(refusal) => {
                failure = Some(refusal);
                break;
            }
        }
    }
    assert_eq!(ok, 50_000);
    let refusal = failure.expect("the ragged record must refuse mid-stream");
    assert_eq!(refusal.code, RefusalCode::ECsvParse);
    assert_eq!(refusal.context["line"], json!(50_002));
}

#[test]
fn iterator_ends_after_the_first_err_and_a_second_call_is_empty() {
    let (_d, p) = temp("two.csv", b"a,b\n1,2\n3\n4,5\n6,7\n");
    let mut table = open(&p, &comma()).unwrap();
    let items: Vec<_> = table.rows().collect();
    assert_eq!(
        items.len(),
        2,
        "one Ok row, then the Err, then nothing: {items:?}"
    );
    assert!(items[0].is_ok());
    assert_eq!(items[1].as_ref().unwrap_err().code, RefusalCode::ECsvParse);
    assert_eq!(table.rows().count(), 0, "the stream is consumed");
}

#[test]
fn rows_taken_early_are_correct_without_reading_the_tail() {
    let mut body = String::from("a,b\n");
    for i in 0..1000 {
        body.push_str(&format!("{i},y\n"));
    }
    body.push_str("\"unterminated,quote\n");
    let (_d, p) = temp("tail.csv", body.as_bytes());
    let mut table = open(&p, &comma()).unwrap();
    let first: Vec<Row> = table.rows().take(3).map(Result::unwrap).collect();
    assert_eq!(first.len(), 3);
    assert_eq!(first[2].cells[0], t("2"));
}

#[test]
fn jsonl_value_refusal_is_mid_stream_but_a_bad_line_refuses_at_open() {
    let (_d, p) = temp(
        "late.jsonl",
        b"{\"a\":1}\n{\"a\":2}\n{\"a\":{\"nested\":true}}\n{\"a\":4}\n",
    );
    let mut table = open(&p, &OpenOptions::default()).unwrap();
    let items: Vec<_> = table.rows().collect();
    assert_eq!(items.len(), 3);
    assert!(items[0].is_ok() && items[1].is_ok());
    let refusal = items[2].as_ref().unwrap_err();
    assert_eq!(refusal.code, RefusalCode::EUnsupportedValue);
    assert_eq!(refusal.context["row"], json!(3));

    // Pass 1 parses every line to build the header union, so a malformed line
    // refuses before any row is produced.
    let (_d2, p2) = temp("bad.jsonl", b"{\"a\":1}\n{\"a\":2}\nnot json\n");
    let r = open(&p2, &OpenOptions::default()).unwrap_err();
    assert_eq!(r.code, RefusalCode::EJsonlParse);
    assert_eq!(r.context["line"], json!(3));
}

#[test]
fn max_rows_is_enforced_as_a_mid_stream_refusal() {
    let (_d, p) = temp("five.csv", b"a,b\n1,1\n2,2\n3,3\n4,4\n5,5\n");
    let opts = OpenOptions {
        max_rows: Some(3),
        ..comma()
    };
    let mut table = open(&p, &opts).unwrap();
    let items: Vec<_> = table.rows().collect();
    assert_eq!(items.len(), 4, "three rows, then the refusal");
    assert!(items[..3].iter().all(Result::is_ok));
    let refusal = items[3].as_ref().unwrap_err();
    assert_eq!(refusal.code, RefusalCode::ETooLarge);
    assert_eq!(refusal.context["limit"], json!("max_rows"));
    assert_eq!(refusal.context["max"], json!(3));
}

#[test]
fn parquet_late_unsupported_value_arrives_after_the_good_rows() {
    let mut table = open(&fx("late_binary.parquet"), &OpenOptions::default()).unwrap();
    let items: Vec<_> = table.rows().collect();
    assert_eq!(items.len(), 3, "two Ok rows then the Err");
    assert!(items[0].is_ok() && items[1].is_ok());
    let refusal = items[2].as_ref().unwrap_err();
    assert_eq!(refusal.code, RefusalCode::EUnsupportedValue);
    assert_eq!(refusal.context["kind"], json!("binary"));
    assert_eq!(refusal.context["row"], json!(3));
    assert!(!serde_json::to_string(refusal).unwrap().contains("late"));
}

#[test]
fn xlsx_late_unsupported_cell_arrives_after_the_good_rows() {
    let mut table = open(&fx("late_duration.xlsx"), &sheet("t")).unwrap();
    let items: Vec<_> = table.rows().collect();
    assert_eq!(items.len(), 3, "two Ok rows then the Err");
    let refusal = items[2].as_ref().unwrap_err();
    assert_eq!(refusal.code, RefusalCode::EUnsupportedValue);
    assert_eq!(refusal.context["kind"], json!("duration"));
    assert_eq!(
        (
            refusal.context["row"].clone(),
            refusal.context["col"].clone()
        ),
        (json!(4), json!(2))
    );
}

#[test]
fn first_row_refusals_and_empty_inputs_still_come_from_open() {
    let (_d, p) = temp("hdr.csv", b"a,b\n");
    assert_eq!(open(&p, &comma()).unwrap_err().code, RefusalCode::EEmpty);
    let (_d, p) = temp("first.csv", b"a,b\n1\n2,3\n");
    assert_eq!(open(&p, &comma()).unwrap_err().code, RefusalCode::ECsvParse);
    let (_d, p) = temp("e.jsonl", b"{\"a\":{\"x\":1}}\n{\"a\":2}\n");
    assert_eq!(
        open(&p, &OpenOptions::default()).unwrap_err().code,
        RefusalCode::EUnsupportedValue
    );
}
