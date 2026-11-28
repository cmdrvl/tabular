//! In-memory reader parity: the byte API must be the same table contract as `open`.

mod common;

use std::path::Path;

use cmdrvl_tabular::{
    CsvOptions, Format, OpenOptions, Refusal, RefusalCode, Row, SourceInfo, open, open_bytes,
};
use common::{fx, sheet, temp};

type Observation = (Format, Vec<Vec<u8>>, Vec<Result<Row, Refusal>>, SourceInfo);

fn path_observation(path: &Path, opts: &OpenOptions) -> Result<Observation, Refusal> {
    let mut table = open(path, opts)?;
    let format = table.format();
    let headers = table.headers().to_vec();
    let source = table.source().clone();
    let rows = table.rows().collect();
    Ok((format, headers, rows, source))
}

fn bytes_observation(
    bytes: Vec<u8>,
    name_hint: &str,
    opts: &OpenOptions,
) -> Result<Observation, Refusal> {
    let mut table = open_bytes(bytes, Some(name_hint), opts)?;
    let format = table.format();
    let headers = table.headers().to_vec();
    let source = table.source().clone();
    let rows = table.rows().collect();
    Ok((format, headers, rows, source))
}

const BD_TG9_CORRUPT_PARQUET: &str = "UEFSMRUEFRAVFEwVAhUAEgAACBwBAAAAAAAAABUAFRIVFiwVAhUQFQYVBhwYCAEAAAAAAAAAGAgBAAAAAAAAABYAKAgBAAAAAAAAABgIAQAAAAAAAAAREQAAAAkgAgAAAAIBAQIAFQQVCBUMTBUCFQASAAAEDJTdAAAVABUSFRYsFQIVEBUGFQYcGASUTQAAGASUTQAAFgAoBJRNAAAYBJRNAAAREQAAAAkgAgAAAAIBAQIAFQQZPDUAGAZzY2hlbWEVBAAVBCUCGAJpZAAVAiUCGAZjbG9zZWQlDExsAAAAFgIZHBksJgAcFQQZNQAGEBkYAmlkFQIWAha8ARbEASY4JggcGAgBAAAAAAAAABgIAQAAAAAAAAAWACgIAQAAAAAAAAAYCAEAAAAAAAAAEREAGSwVBBUAFQIAFQAVEBUCADwpBhkmAAIAAAAmABwVAhk1AAYQGRgGY2xvc2VkFQIWAhaUARabASb0ASbMARwYBJRNAAAYBJRNAAAWACgElE0AABgElE0AABERABksFQQVABUCABUAFRAVAgA8KQYZJgACAAAAFtACFgImCBbgAgAZHBgMQVJST1c6c2NoZW1hGPgBLy8vLy83QUFBQUFRQUFBQUFBQUtBQXdBQmdBRkFBZ0FDZ0FBQUFBQkJBQU1BQUFBQ0FBSUFBQUFCQUFJQUFBQUJBQUFBQUlBQUFCTUFBQUFCQUFBQU16Ly8vOEFBQUVJRUFBQUFDQUFBQUFFQUFBQUFBQUFBQVlBQUFCamJHOXpaV1FBQUFBQUJnQUlBQVlBQgAAQUFBQUFBQUFRQUJRQUNBQUdBQWNBREFBQUFCQUFFQUFBQUFBQUFRSVFBQUFBSEFBQUFBUUFBQUFBQUFBQUFnQUFBR2xrQUFBSUFBd0FDQUFIQUFnQUFBQUFBQUFCUUFBQUFBPT0AGCBwYXJxdWV0LWNwcC1hcnJvdyB2ZXJzaW9uIDI2LjAuMRksHAAAHAAAADUCAABQQVIx";

fn decode_bd_tg9_fixture() -> Vec<u8> {
    fn value(byte: u8) -> Option<u8> {
        Some(match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    }

    let bytes = BD_TG9_CORRUPT_PARQUET.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let Some(&first) = chunk.first() else {
            return Vec::new();
        };
        let Some(&second) = chunk.get(1) else {
            return Vec::new();
        };
        let Some(a) = value(first) else {
            return Vec::new();
        };
        let Some(b) = value(second) else {
            return Vec::new();
        };
        decoded.push((a << 2) | (b >> 4));
        if let Some(&third) = chunk.get(2)
            && third != b'='
        {
            let Some(c) = value(third) else {
                return Vec::new();
            };
            decoded.push((b << 4) | (c >> 2));
            if let Some(&fourth) = chunk.get(3)
                && fourth != b'='
            {
                let Some(d) = value(fourth) else {
                    return Vec::new();
                };
                decoded.push((c << 6) | d);
            }
        }
    }
    decoded
}

#[test]
fn bd_tg9_corrupt_parquet_is_a_typed_refusal_for_path_and_bytes() {
    let bytes = decode_bd_tg9_fixture();
    assert_eq!(bytes.len(), 753);

    let bytes_refusal = open_bytes(
        bytes.clone(),
        Some("corrupt.parquet"),
        &OpenOptions::default(),
    )
    .unwrap_err();
    assert_eq!(bytes_refusal.code, RefusalCode::EParquetParse);

    let (_dir, path) = temp("corrupt.parquet", &bytes);
    let path_refusal = open(&path, &OpenOptions::default()).unwrap_err();
    assert_eq!(path_refusal, bytes_refusal);
}

fn assert_path_bytes(path: &Path, name_hint: &str, opts: &OpenOptions) {
    let bytes = std::fs::read(path).unwrap();
    let path_result = path_observation(path, opts);
    let bytes_result = bytes_observation(bytes, name_hint, opts);
    assert_eq!(
        path_result.is_ok(),
        bytes_result.is_ok(),
        "{name_hint} result kind"
    );
    match (path_result, bytes_result) {
        (Ok(path_table), Ok(bytes_table)) => assert_eq!(path_table, bytes_table, "{name_hint}"),
        (Err(path_refusal), Err(bytes_refusal)) => {
            assert_eq!(path_refusal, bytes_refusal, "{name_hint} refusal")
        }
        (path_result, bytes_result) => {
            assert_eq!(
                path_result.is_ok(),
                bytes_result.is_ok(),
                "{name_hint} result kind"
            );
        }
    }
}

#[test]
fn all_five_formats_have_identical_headers_cells_and_source_metadata() {
    for (file, opts) in [
        ("sample.csv", OpenOptions::default()),
        ("sample.tsv", OpenOptions::default()),
        ("sample.jsonl", OpenOptions::default()),
        ("sample.parquet", OpenOptions::default()),
        ("sample.xlsx", sheet("loans")),
    ] {
        assert_path_bytes(&fx(file), file, &opts);
    }
}

#[test]
fn explicit_format_allows_a_missing_name_hint() {
    let bytes = std::fs::read(fx("sample.csv")).unwrap();
    let opts = OpenOptions {
        format: Some(Format::Csv),
        csv: CsvOptions {
            delimiter: Some(b','),
            ..CsvOptions::default()
        },
        ..OpenOptions::default()
    };
    let table = open_bytes(bytes, None, &opts).unwrap();
    assert_eq!(table.format(), Format::Csv);
    assert_eq!(
        table.headers(),
        &[
            b"id".to_vec(),
            b"region".to_vec(),
            b"amount".to_vec(),
            b"active".to_vec(),
            b"note".to_vec()
        ]
    );
}

#[test]
fn max_rows_and_max_bytes_have_path_identical_refusals_in_every_format() {
    for (file, opts) in [
        ("sample.csv", OpenOptions::default()),
        ("sample.tsv", OpenOptions::default()),
        ("sample.jsonl", OpenOptions::default()),
        ("sample.parquet", OpenOptions::default()),
        ("sample.xlsx", sheet("loans")),
    ] {
        let limited_rows = OpenOptions {
            max_rows: Some(2),
            ..opts.clone()
        };
        assert_path_bytes(&fx(file), file, &limited_rows);

        let limited_bytes = OpenOptions {
            max_bytes: Some(1),
            ..opts
        };
        assert_path_bytes(&fx(file), file, &limited_bytes);
    }
}

#[test]
fn xlsx_uncompressed_limit_and_zip_bomb_refusal_match_open() {
    let opts = OpenOptions {
        max_uncompressed_bytes: Some(100_000),
        ..OpenOptions::default()
    };
    assert_path_bytes(&fx("zip_bomb_honest.xlsx"), "zip_bomb_honest.xlsx", &opts);

    let bytes = std::fs::read(fx("zip_bomb_lying.xlsx")).unwrap();
    let opts = OpenOptions {
        max_uncompressed_bytes: Some(1_000_000),
        ..OpenOptions::default()
    };
    let (_dir, path) = temp("lying.xlsx", &bytes);
    assert_path_bytes(&path, "zip_bomb_lying.xlsx", &opts);
}

#[test]
fn bad_inputs_have_identical_typed_refusals() {
    let (_dir, path) = temp("bad.jsonl", b"{\"a\":1}\nnot json\n");
    assert_path_bytes(&path, "bad.jsonl", &OpenOptions::default());

    let parquet = std::fs::read(fx("sample.parquet")).unwrap();
    let (_dir, path) = temp("bad.parquet", &parquet[..parquet.len() / 2]);
    assert_path_bytes(&path, "bad.parquet", &OpenOptions::default());

    let (_dir, path) = temp("bad.csv", b"a,b\n1\n");
    let opts = OpenOptions {
        csv: CsvOptions {
            delimiter: Some(b','),
            ..CsvOptions::default()
        },
        ..OpenOptions::default()
    };
    assert_path_bytes(&path, "bad.csv", &opts);

    let bytes = std::fs::read(fx("sample.csv")).unwrap();
    let (_dir, path) = temp("wrong.parquet", &bytes);
    let path_result = path_observation(&path, &OpenOptions::default()).unwrap_err();
    let bytes_result =
        bytes_observation(bytes, "wrong.parquet", &OpenOptions::default()).unwrap_err();
    assert_eq!(path_result.code, RefusalCode::EFormat);
    assert_eq!(path_result, bytes_result);
}
