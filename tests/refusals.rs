//! Planted negatives: every refusal code, and the zero-retention rule.

mod common;

use cmdrvl_tabular::{Cell, CsvOptions, Format, OpenOptions, RefusalCode, SheetSel, open};
use common::*;
use serde_json::json;

fn opts_sheet(sel: SheetSel) -> OpenOptions {
    OpenOptions {
        sheet: Some(sel),
        ..OpenOptions::default()
    }
}

// ---- sheet selection -------------------------------------------------------

#[test]
fn multi_sheet_workbook_without_sheet_is_ambiguous_with_listing() {
    let r = refusal("sample.xlsx", &OpenOptions::default());
    assert_eq!(r.code, RefusalCode::EAmbiguousSheet);
    assert_eq!(r.context["sheets"], json!(["loans", "notes"]));
}

#[test]
fn hidden_sheets_count_as_sheets() {
    let r = refusal("hidden_sheet.xlsx", &OpenOptions::default());
    assert_eq!(r.code, RefusalCode::EAmbiguousSheet);
    assert_eq!(r.context["sheets"], json!(["loans", "scratch"]));
}

#[test]
fn unknown_sheet_is_not_found_and_lists_sheets() {
    let r = refusal("sample.xlsx", &sheet("nope"));
    assert_eq!(r.code, RefusalCode::ESheetNotFound);
    assert_eq!(r.context["requested"], json!("nope"));
    assert_eq!(r.context["sheets"], json!(["loans", "notes"]));
    let r = refusal("sample.xlsx", &opts_sheet(SheetSel::Index(7)));
    assert_eq!(r.code, RefusalCode::ESheetNotFound);
}

#[test]
fn sheet_selected_by_index_or_name_and_reported() {
    let mut table = open(&fx("sample.xlsx"), &opts_sheet(SheetSel::Index(0))).unwrap();
    assert_eq!(count_rows(&mut table), 3);
    assert_eq!(table.source().sheet.as_deref(), Some("loans"));
    let table = open(&fx("sample.xlsx"), &opts_sheet(SheetSel::parse("notes"))).unwrap_err();
    assert_eq!(table.code, RefusalCode::EEmpty, "notes has a header only");
}

#[test]
fn sheet_name_beats_index_reading_of_the_same_text() {
    // Sheets are named "1" (index 0) and "x" (index 1). `--sheet 1` is the sheet NAMED "1".
    let mut table = open(
        &fx("digit_sheet_names.xlsx"),
        &opts_sheet(SheetSel::parse("1")),
    )
    .unwrap();
    let (_, rows) = grid(&mut table);
    assert_eq!(rows, vec![vec![t("from-name-1")]]);
    assert_eq!(table.source().sheet.as_deref(), Some("1"));
}

#[test]
fn used_range_not_starting_at_a1_takes_header_from_first_used_row() {
    let mut table = open(&fx("off_origin.xlsx"), &OpenOptions::default()).unwrap();
    let (headers, rows) = grid(&mut table);
    assert_eq!(headers[0], b"id");
    assert_eq!(rows[2][1], t(" North "));
}

#[test]
fn encrypted_workbook_is_refused() {
    let r = refusal("encrypted.xlsx", &OpenOptions::default());
    assert_eq!(r.code, RefusalCode::EEncrypted);
}

#[test]
fn truncated_workbook_is_xlsx_parse_error() {
    let bytes = std::fs::read(fx("sample.xlsx")).unwrap();
    let (_dir, path) = temp("cut.xlsx", &bytes[..bytes.len() / 2]);
    let r = open(&path, &sheet("loans")).unwrap_err();
    assert_eq!(r.code, RefusalCode::EXlsxParse);
}

// ---- unsupported values: refused, and the refusal never carries the value ---

fn assert_no_leak(refusal: &cmdrvl_tabular::Refusal, secret: &str) {
    let rendered = serde_json::to_string(refusal).unwrap();
    assert!(!rendered.contains(secret), "leaked {secret}: {rendered}");
}

#[test]
fn parquet_nested_list_is_refused() {
    let r = refusal("nested_list.parquet", &OpenOptions::default());
    assert_eq!(r.code, RefusalCode::EUnsupportedValue);
    assert_eq!(r.context["kind"], json!("list"));
    assert_no_leak(&r, "\"x\"");
}

#[test]
fn jsonl_nested_value_is_refused_without_the_value() {
    let (_d, p) = temp("n.jsonl", b"{\"a\":1}\n{\"a\":{\"secret\":\"hunter2\"}}\n");
    let r = first_refusal(&p, &OpenOptions::default()).expect("nested value must refuse");
    assert_eq!(r.code, RefusalCode::EUnsupportedValue);
    assert_eq!(
        (r.context["column"].clone(), r.context["row"].clone()),
        (json!("a"), json!(2))
    );
    assert_no_leak(&r, "hunter2");
}

// ---- limits -----------------------------------------------------------------

#[test]
fn max_rows_boundary_in_every_format() {
    for (file, opts) in [
        ("sample.csv", OpenOptions::default()),
        ("sample.tsv", OpenOptions::default()),
        ("sample.jsonl", OpenOptions::default()),
        ("sample.parquet", OpenOptions::default()),
        ("sample.xlsx", sheet("loans")),
    ] {
        let at_limit = OpenOptions {
            max_rows: Some(3),
            ..opts.clone()
        };
        assert_eq!(
            count_rows(&mut open(&fx(file), &at_limit).unwrap()),
            3,
            "{file}"
        );
        let under = OpenOptions {
            max_rows: Some(2),
            ..opts
        };
        let r = first_refusal(&fx(file), &under).expect("over the limit must refuse");
        assert_eq!(r.code, RefusalCode::ETooLarge, "{file}");
        assert_eq!(r.context["limit"], json!("max_rows"), "{file}");
    }
}

#[test]
fn max_bytes_refuses_before_reading() {
    let opts = OpenOptions {
        max_bytes: Some(10),
        ..OpenOptions::default()
    };
    let r = refusal("sample.csv", &opts);
    assert_eq!(r.code, RefusalCode::ETooLarge);
    assert_eq!(r.context["limit"], json!("max_bytes"));
    assert_eq!(r.context["max"], json!(10));
}

// ---- format detection -------------------------------------------------------

#[test]
fn unknown_extension_and_content_mismatch_are_e_format() {
    let (_d, p) = temp("data.dat", b"a,b\n1,2\n");
    let r = open(&p, &OpenOptions::default()).unwrap_err();
    assert_eq!(r.code, RefusalCode::EFormat);
    assert_eq!(r.context["extension"], json!("dat"));

    let parquet = std::fs::read(fx("sample.parquet")).unwrap();
    let (_d, p) = temp("actually_parquet.csv", &parquet);
    let r = open(&p, &OpenOptions::default()).unwrap_err();
    assert_eq!(
        (r.code, r.context["detected"].clone()),
        (RefusalCode::EFormat, json!("parquet"))
    );

    let (_d, p) = temp("text.parquet", b"a,b\n1,2\n");
    let r = open(&p, &OpenOptions::default()).unwrap_err();
    assert_eq!(r.context["expected"], json!("parquet"));
    assert_eq!(r.context["detected"], json!("text"));
}

#[test]
fn format_override_still_checks_magic() {
    let (_d, p) = temp("data.dat", b"a,b\n1,2\n");
    let opts = OpenOptions {
        format: Some(Format::Csv),
        ..OpenOptions::default()
    };
    assert_eq!(count_rows(&mut open(&p, &opts).unwrap()), 1);
    let opts = OpenOptions {
        format: Some(Format::Parquet),
        ..OpenOptions::default()
    };
    assert_eq!(open(&p, &opts).unwrap_err().code, RefusalCode::EFormat);
}

#[test]
fn corrupt_parquet_footer_is_parquet_parse_error() {
    let (_d, p) = temp("bad.parquet", b"PAR1this is not a footerPAR1");
    assert_eq!(
        open(&p, &OpenOptions::default()).unwrap_err().code,
        RefusalCode::EParquetParse
    );
}

// ---- CSV / TSV --------------------------------------------------------------

fn csv_with(bytes: &[u8], opts: &OpenOptions) -> Result<Vec<Vec<Cell>>, cmdrvl_tabular::Refusal> {
    let (_d, p) = temp("in.csv", bytes);
    let mut table = open(&p, opts)?;
    // Drain the stream: a refusal anywhere in the file counts, whether `open` or a later row raised it.
    table.rows().map(|row| row.map(|r| r.cells)).collect()
}

fn csv_of(bytes: &[u8]) -> Result<Vec<Vec<Cell>>, cmdrvl_tabular::Refusal> {
    csv_with(bytes, &OpenOptions::default())
}

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
fn detects_every_candidate_delimiter() {
    for d in [',', '\t', ';', '|', '^'] {
        let body = format!("a{d}b\n1{d}2\n3{d}4\n");
        let (_dir, p) = temp("in.csv", body.as_bytes());
        let mut table = open(&p, &OpenOptions::default()).unwrap();
        assert_eq!(table.source().delimiter, Some(d as u8), "{d:?}");
        assert_eq!(count_rows(&mut table), 2);
    }
}

#[test]
fn explicit_delimiter_overrides_detection_and_tsv_pins_tab() {
    let (_d, p) = temp("in.csv", b"a;b,c\n1;2,3\n");
    let opts = OpenOptions {
        csv: CsvOptions {
            delimiter: Some(b';'),
            skip_rows: 0,
        },
        ..OpenOptions::default()
    };
    let mut table = open(&p, &opts).unwrap();
    assert_eq!(grid(&mut table).1, vec![vec![t("1"), t("2,3")]]);
    let (_d, p) = temp("in.tsv", b"a,b\tc\n1,x\ty\n");
    assert_eq!(
        open(&p, &OpenOptions::default())
            .unwrap()
            .source()
            .delimiter,
        Some(b'\t')
    );
}

#[test]
fn single_column_file_is_a_dialect_refusal() {
    let r = csv_of(b"a\nb\nc\n").unwrap_err();
    assert_eq!(r.code, RefusalCode::EDialect);
    assert!(r.context["candidates"].is_array());
}

#[test]
fn skip_rows_discards_a_preamble() {
    let (_d, p) = temp(
        "in.csv",
        b"Report: Q3\nGenerated: today\nid,region\n1,East\n",
    );
    let opts = OpenOptions {
        csv: CsvOptions {
            delimiter: None,
            skip_rows: 2,
        },
        ..OpenOptions::default()
    };
    let mut table = open(&p, &opts).unwrap();
    let (headers, rows) = grid(&mut table);
    assert_eq!(headers, vec![b"id".to_vec(), b"region".to_vec()]);
    assert_eq!(rows, vec![vec![t("1"), t("East")]]);
}

#[test]
fn bom_is_stripped_crlf_and_quoted_newlines_survive_and_non_utf8_passes_through() {
    let rows = csv_with(
        b"\xEF\xBB\xBFid,note\r\n1,\"line one\nline two\"\r\n2,\xff\xfe-bytes\r\n",
        &comma(),
    );
    let rows = rows.unwrap();
    assert_eq!(rows[0], vec![t("1"), t("line one\nline two")]);
    assert_eq!(
        rows[1],
        vec![t("2"), Cell::Text(b"\xff\xfe-bytes".to_vec())]
    );
    let (_d, p) = temp("in.csv", b"\xEF\xBB\xBFid,note\n1,x\n");
    assert_eq!(
        open(&p, &OpenOptions::default()).unwrap().headers()[0],
        b"id"
    );
}

#[test]
fn empty_fields_are_null_and_quoted_empty_too() {
    let rows = csv_of(b"a,b,c\n1,,\"\"\n").unwrap();
    assert_eq!(rows[0], vec![t("1"), Cell::Null, Cell::Null]);
}

#[test]
fn utf16_bom_and_nul_bytes_are_encoding_refusals() {
    assert_eq!(
        csv_of(b"\xFF\xFEa\x00,\x00b\x00\n\x001\x00")
            .unwrap_err()
            .code,
        RefusalCode::EEncoding
    );
    assert_eq!(
        csv_of(b"a,b\n1,\x002\n").unwrap_err().code,
        RefusalCode::EEncoding
    );
}

#[test]
fn empty_and_header_only_inputs() {
    assert_eq!(csv_of(b"").unwrap_err().code, RefusalCode::EHeaders);
    assert_eq!(csv_of(b"a,b\n").unwrap_err().code, RefusalCode::EEmpty);
}

#[test]
fn ragged_record_is_a_parse_error_with_counts_when_the_delimiter_is_known() {
    let r = csv_with(b"a,b\n1,2\n3\n", &comma()).unwrap_err();
    assert_eq!(r.code, RefusalCode::ECsvParse);
    assert_eq!(
        (r.context["expected"].clone(), r.context["found"].clone()),
        (json!(2), json!(1))
    );
    assert_eq!(r.context["line"], json!(3));

    // Auto-detect, ragged row beyond the 200-record detection sample: detected fine, then refused.
    let mut body = String::from("a,b\n");
    for i in 0..300 {
        body.push_str(&format!("{i},x\n"));
    }
    body.push_str("solo\n");
    let r = csv_of(body.as_bytes()).unwrap_err();
    assert_eq!(r.code, RefusalCode::ECsvParse);
    assert_eq!(r.context["line"], json!(302));
}

#[test]
fn known_limit_ragged_record_inside_the_detection_sample_refuses_dialect() {
    // Same scoring as shape: a probe that hits a ragged record is dropped, so the
    // single-field tab/etc. probes win and detection refuses. Pin the delimiter to proceed.
    let r = csv_of(b"a,b\n1,2\n3\n").unwrap_err();
    assert_eq!(r.code, RefusalCode::EDialect);
}

// ---- JSONL ------------------------------------------------------------------

#[test]
fn jsonl_ragged_keys_use_first_seen_union_order_with_null_padding() {
    let (_d, p) = temp(
        "r.ndjson",
        b"{\"b\":1}\n\n{\"a\":2,\"b\":3}\n{\"c\":true}\n",
    );
    let mut table = open(&p, &OpenOptions::default()).unwrap();
    let (headers, rows) = grid(&mut table);
    assert_eq!(headers, vec![b"b".to_vec(), b"a".to_vec(), b"c".to_vec()]);
    assert_eq!(rows[0], vec![t("1"), Cell::Null, Cell::Null]);
    assert_eq!(rows[1], vec![t("3"), t("2"), Cell::Null]);
    assert_eq!(rows[2], vec![Cell::Null, Cell::Null, t("true")]);
}

#[test]
fn jsonl_numbers_render_canonically() {
    let (_d, p) = temp(
        "n.jsonl",
        b"{\"v\":2000.0}\n{\"v\":0.1}\n{\"v\":-7}\n{\"v\":18446744073709551615}\n",
    );
    let (_, rows) = grid(&mut open(&p, &OpenOptions::default()).unwrap());
    let got: Vec<_> = rows.iter().map(|r| r[0].clone()).collect();
    assert_eq!(
        got,
        vec![t("2000"), t("0.1"), t("-7"), t("18446744073709551615")]
    );
}

#[test]
fn jsonl_parse_errors_carry_line_numbers_not_content() {
    let (_d, p) = temp("b.jsonl", b"{\"a\":1}\n{\"a\":SECRETTOKEN}\n");
    let r = open(&p, &OpenOptions::default()).unwrap_err();
    assert_eq!(
        (r.code, r.context["line"].clone()),
        (RefusalCode::EJsonlParse, json!(2))
    );
    assert_no_leak(&r, "SECRETTOKEN");
    let (_d, p) = temp("a.jsonl", b"[1,2]\n");
    assert_eq!(
        open(&p, &OpenOptions::default()).unwrap_err().code,
        RefusalCode::EJsonlParse
    );
    let (_d, p) = temp("e.jsonl", b"\n\n");
    assert_eq!(
        open(&p, &OpenOptions::default()).unwrap_err().code,
        RefusalCode::EHeaders
    );
}

// ---- taxonomy ---------------------------------------------------------------

#[test]
fn refusal_codes_have_stable_unique_names() {
    let names: std::collections::BTreeSet<_> =
        RefusalCode::ALL.iter().map(|c| c.as_str()).collect();
    assert_eq!(names.len(), RefusalCode::ALL.len());
    assert!(
        names
            .iter()
            .all(|n| n.starts_with("E_") && n.chars().all(|c| c.is_ascii_uppercase() || c == '_'))
    );
    // Names shape already emits must not move.
    for shape in [
        "E_IO",
        "E_ENCODING",
        "E_CSV_PARSE",
        "E_EMPTY",
        "E_HEADERS",
        "E_DIALECT",
        "E_TOO_LARGE",
    ] {
        assert!(names.contains(shape), "{shape}");
    }
}

#[test]
fn io_refusal_carries_kind_only() {
    let r = open(
        std::path::Path::new("/definitely/not/here/x.csv"),
        &OpenOptions::default(),
    )
    .unwrap_err();
    assert_eq!(r.code, RefusalCode::EIo);
    assert_eq!(r.context["kind"], json!("NotFound"));
    assert_no_leak(&r, "definitely");
}

// ---- XLSX decompressed-size guard -------------------------------------------

#[test]
fn honest_zip_bomb_refuses_on_declared_size_with_a_small_limit() {
    // ~5 KB on disk, 3 MB decompressed: max_bytes alone would wave it through.
    let opts = OpenOptions {
        max_bytes: Some(1_000_000),
        max_uncompressed_bytes: Some(100_000),
        ..sheet("t")
    };
    let r = refusal("zip_bomb_honest.xlsx", &opts);
    assert_eq!(r.code, RefusalCode::ETooLarge);
    assert_eq!(r.context["limit"], json!("max_uncompressed_bytes"));
    assert_eq!(r.context["max"], json!(100_000));
    assert!(r.context["declared"].as_u64().unwrap() > 3_000_000);
}

#[test]
fn honest_bomb_opens_when_the_limit_allows_it() {
    let opts = OpenOptions {
        max_uncompressed_bytes: Some(64 * 1024 * 1024),
        ..sheet("t")
    };
    let mut table = open(&fx("zip_bomb_honest.xlsx"), &opts).unwrap();
    assert_eq!(count_rows(&mut table), 100);
}

#[test]
fn the_default_limit_is_two_gib_and_applies_when_unset() {
    assert_eq!(
        cmdrvl_tabular::DEFAULT_MAX_UNCOMPRESSED_BYTES,
        2 * 1024 * 1024 * 1024
    );
    // Unset means the default, which this 3 MB workbook is far under.
    assert!(open(&fx("zip_bomb_honest.xlsx"), &sheet("t")).is_ok());
}

#[test]
fn lying_headers_cannot_smuggle_extra_output() {
    // The entry declares 1000 bytes but its deflate stream holds 8 MB. The zip crate
    // stops an entry at its declared size and errors, so the workbook refuses as
    // unreadable under the default limit AND under a small one; nothing expands past
    // what the central directory declared.
    for opts in [
        sheet("t"),
        OpenOptions {
            max_uncompressed_bytes: Some(1_000_000),
            ..sheet("t")
        },
    ] {
        let r = refusal("zip_bomb_lying.xlsx", &opts);
        assert_eq!(r.code, RefusalCode::EXlsxParse, "{r:?}");
        let rendered = serde_json::to_string(&r).unwrap();
        assert!(!rendered.contains("aaaa"), "{rendered}");
    }
}

#[test]
fn normal_workbooks_are_unaffected_by_the_default_guard() {
    let mut table = open(&fx("sample.xlsx"), &sheet("loans")).unwrap();
    assert_eq!(count_rows(&mut table), 3);
}
