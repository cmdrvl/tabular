//! Cross-format conformance: the same dataset as CSV, TSV, JSONL, Parquet and
//! XLSX must produce identical headers and cells, and those cells must satisfy
//! the shared composite-key contract. Fixtures come from independent writers
//! (see tests/fixtures/make_fixtures.py).

mod common;

use cmdrvl_tabular::{Cell, CellClass, Format, OpenOptions, open};
use common::*;
use spine_rules::composite_key::{assert_ordered_tuple_semantics, reference_key_tuple};

fn expected_headers() -> Vec<Vec<u8>> {
    ["id", "region", "amount", "active", "note"]
        .iter()
        .map(|h| h.as_bytes().to_vec())
        .collect()
}

/// Hard-coded golden, not derived from any reader.
fn expected_rows() -> Vec<Vec<Cell>> {
    vec![
        vec![t("1"), t("East"), t("100.5"), t("true"), t("NA")],
        vec![t("2"), t("West"), t("2000"), t("false"), t("NULL")],
        vec![t("3"), t(" North "), t("0.1"), t("true"), Cell::Null],
    ]
}

fn five() -> [(&'static str, Format, OpenOptions); 5] {
    [
        ("sample.csv", Format::Csv, OpenOptions::default()),
        ("sample.tsv", Format::Tsv, OpenOptions::default()),
        ("sample.jsonl", Format::Jsonl, OpenOptions::default()),
        ("sample.parquet", Format::Parquet, OpenOptions::default()),
        ("sample.xlsx", Format::Xlsx, sheet("loans")),
    ]
}

#[test]
fn all_five_formats_match_the_golden_grid() {
    for (file, format, opts) in five() {
        let mut table = open(&fx(file), &opts).unwrap_or_else(|e| unreachable!("{file}: {e}"));
        assert_eq!(table.format(), format, "{file}");
        let (headers, rows) = grid(&mut table);
        assert_eq!(headers, expected_headers(), "{file} headers");
        assert_eq!(rows, expected_rows(), "{file} rows");
    }
}

#[test]
fn rows_have_one_based_indexes_and_header_width() {
    for (file, _, opts) in five() {
        let mut table = open(&fx(file), &opts).unwrap();
        let width = table.headers().len();
        let rows: Vec<_> = table.rows().map(Result::unwrap).collect();
        assert_eq!(
            rows.iter().map(|r| r.index).collect::<Vec<_>>(),
            [1, 2, 3],
            "{file}"
        );
        assert!(rows.iter().all(|r| r.cells.len() == width), "{file}");
    }
}

#[test]
fn rows_is_single_pass() {
    let mut table = open(&fx("sample.csv"), &OpenOptions::default()).unwrap();
    assert_eq!(table.rows().count(), 3);
    assert_eq!(table.rows().count(), 0);
}

#[test]
fn missing_tokens_stay_text_and_padding_is_preserved_in_every_format() {
    for (file, _, opts) in five() {
        let (_, rows) = grid(&mut open(&fx(file), &opts).unwrap());
        assert_eq!(rows[0][4], t("NA"), "{file}");
        assert_eq!(rows[1][4], t("NULL"), "{file}");
        assert_eq!(rows[2][4], Cell::Null, "{file}");
        assert_eq!(rows[2][1], t(" North "), "{file}");
    }
}

#[test]
fn classification_agrees_across_formats() {
    let want = [
        CellClass::Integer,
        CellClass::Text,
        CellClass::Decimal,
        CellClass::Bool,
        CellClass::Text,
    ];
    for (file, _, opts) in five() {
        let (_, rows) = grid(&mut open(&fx(file), &opts).unwrap());
        let got: Vec<_> = rows[0].iter().map(Cell::classify).collect();
        assert_eq!(got, want, "{file}");
    }
}

#[test]
fn composite_keys_are_identical_across_formats() {
    // Key = (region, id). Row 3 has padded " North ": trimmed by the shared reference
    // normalization, not by the reader. Key = (id, note): row 3 is incomplete.
    let mut region_keys = Vec::new();
    let mut note_keys = Vec::new();
    for (file, _, opts) in five() {
        let mut table = open(&fx(file), &opts).unwrap();
        let rows: Vec<_> = table.rows().map(Result::unwrap).collect();
        region_keys.push(
            rows.iter()
                .map(|r| reference_key_tuple(&r.key_components(&[1, 0]).unwrap()))
                .collect::<Vec<_>>(),
        );
        note_keys.push(
            rows.iter()
                .map(|r| reference_key_tuple(&r.key_components(&[0, 4]).unwrap()))
                .collect::<Vec<_>>(),
        );
        assert!(
            rows[0].key_components(&[9]).is_none(),
            "{file}: out-of-range column"
        );
    }
    assert!(region_keys.windows(2).all(|w| w[0] == w[1]));
    assert!(note_keys.windows(2).all(|w| w[0] == w[1]));
    assert_eq!(
        region_keys[0][2],
        Some(vec![b"North".to_vec(), b"3".to_vec()])
    );
    assert_eq!(
        note_keys[0][2], None,
        "Null note makes the tuple incomplete"
    );
    assert_eq!(
        note_keys[0][0],
        Some(vec![b"1".to_vec(), b"NA".to_vec()]),
        "NA literal is a real key byte string"
    );
}

#[test]
fn cell_bytes_satisfy_the_shared_ordered_tuple_contract() {
    // Cells are the only thing a key layer sees; run the shared fixtures through them.
    assert_ordered_tuple_semantics(|raw| {
        let key = reference_key_tuple(raw)?;
        Some(key.into_iter().map(Cell::from_bytes).collect::<Vec<_>>())
    });
}

#[test]
fn parquet_codecs_and_row_group_boundaries() {
    // 5 rows in row groups of 2, one file per codec; row 3 has a null note.
    let want: Vec<Vec<Cell>> = (1..=5)
        .map(|i| {
            let note = if i == 3 {
                Cell::Null
            } else {
                t(&format!("n{i}"))
            };
            let amount = format!("{}", i as f64 * 1.5).replace(".0", "");
            vec![
                t(&i.to_string()),
                t(&format!("r{i}")),
                t(&amount),
                t(if i % 2 == 0 { "true" } else { "false" }),
                note,
            ]
        })
        .collect();
    let codecs: &[&str] = if cfg!(feature = "zstd") {
        &["zstd", "gzip", "lz4"]
    } else {
        &["gzip", "lz4"]
    };
    for codec in codecs {
        let (_, rows) = grid(
            &mut open(
                &fx(&format!("rowgroups_{codec}.parquet")),
                &OpenOptions::default(),
            )
            .unwrap(),
        );
        assert_eq!(rows, want, "{codec}");
    }
}

#[test]
fn parquet_float32_uses_f32_shortest_form() {
    let (_, rows) = grid(&mut open(&fx("float32.parquet"), &OpenOptions::default()).unwrap());
    assert_eq!(rows, vec![vec![t("0.1")], vec![t("2")], vec![t("100.5")]]);
}
