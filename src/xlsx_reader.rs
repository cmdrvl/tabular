//! XLSX/XLS reader over calamine with deterministic sheet selection.
//!
//! v0 reads the selected sheet with `worksheet_range`, which materializes the
//! whole sheet (O(sheet) memory). Datetimes render as canonical dates and naive
//! timestamps (`temporal.rs`); durations and error cells refuse.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use calamine::{Data, Reader, Xls, Xlsx, open_workbook};

use crate::cell::canon_f64;
use crate::detect::{Magic, sniff};
use crate::refusal::{Refusal, RefusalCode};
use crate::temporal::{excel_text, excel_time_text, iso_datetime_text};
use crate::{Cell, Opened, Row, SheetSel, missing_headers};

fn parse_refusal() -> Refusal {
    Refusal::new(RefusalCode::EXlsxParse, "not a readable workbook")
}

pub(crate) fn open(
    path: &Path,
    sheet: Option<&SheetSel>,
    max_uncompressed: u64,
) -> Result<Opened, Refusal> {
    match sniff(path)? {
        Magic::Zip => {
            guard_uncompressed(path, max_uncompressed)?;
            let workbook: Xlsx<BufReader<File>> =
                open_workbook(path).map_err(|_| parse_refusal())?;
            read_workbook(workbook, sheet)
        }
        _ => {
            // OLE container: legacy BIFF `.xls`, or an encrypted OOXML package.
            if contains_encrypted_package(path)? {
                return Err(Refusal::new(
                    RefusalCode::EEncrypted,
                    "workbook is password-protected",
                ));
            }
            let workbook: Xls<BufReader<File>> =
                open_workbook(path).map_err(|_| parse_refusal())?;
            read_workbook(workbook, sheet)
        }
    }
}

fn too_large(max: u64) -> Refusal {
    Refusal::new(
        RefusalCode::ETooLarge,
        "workbook expands beyond max_uncompressed_bytes",
    )
    .with("limit", "max_uncompressed_bytes")
    .with("max", max)
}

/// Zip-bomb guard. `max_bytes` only sees the compressed size, so this sums the
/// declared decompressed size of every entry in the central directory before calamine
/// reads anything. Headers cannot be used to smuggle extra output: the `zip` crate
/// (9.x) stops an entry at its declared size and errors, which refuses as
/// `E_XLSX_PARSE` (see the `zip_bomb_lying.xlsx` fixture).
fn guard_uncompressed(path: &Path, max: u64) -> Result<(), Refusal> {
    let file = File::open(path).map_err(|e| Refusal::io(&e))?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).map_err(|_| parse_refusal())?;
    let mut declared = 0u64;
    for i in 0..archive.len() {
        let entry = archive.by_index_raw(i).map_err(|_| parse_refusal())?;
        declared = declared.saturating_add(entry.size());
    }
    if declared > max {
        return Err(too_large(max).with("declared", declared));
    }
    Ok(())
}

/// An encrypted `.xlsx` is an OLE file whose directory has an `EncryptedPackage`
/// stream. Directory names are UTF-16LE, so search for that byte pattern.
fn contains_encrypted_package(path: &Path) -> Result<bool, Refusal> {
    let needle: Vec<u8> = "EncryptedPackage"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let bytes = std::fs::read(path).map_err(|e| Refusal::io(&e))?;
    Ok(bytes.windows(needle.len()).any(|w| w == needle.as_slice()))
}

fn read_workbook<R>(mut workbook: R, sheet: Option<&SheetSel>) -> Result<Opened, Refusal>
where
    R: Reader<BufReader<File>>,
{
    let names = workbook.sheet_names();
    let listing = || serde_json::Value::from(names.clone());
    let selected = match sheet {
        None if names.len() == 1 => names[0].clone(),
        None => {
            return Err(Refusal::new(
                RefusalCode::EAmbiguousSheet,
                "workbook has several sheets; pass a sheet",
            )
            .with("sheets", listing()));
        }
        Some(selector) => resolve_sheet(selector, &names).ok_or_else(|| {
            Refusal::new(
                RefusalCode::ESheetNotFound,
                "requested sheet does not exist",
            )
            .with(
                "requested",
                match selector {
                    SheetSel::Name(name) => name.clone(),
                    SheetSel::Index(index) => index.to_string(),
                },
            )
            .with("sheets", listing())
        })?,
    };
    let range = workbook
        .worksheet_range(&selected)
        .map_err(|_| parse_refusal())?;
    let (row0, col0) = range.start().unwrap_or((0, 0));
    let (height, width) = range.get_size();
    if height == 0 {
        return Err(missing_headers());
    }
    let headers = (0..width)
        .map(|c| {
            let data = range.get((0, c)).unwrap_or(&Data::Empty);
            Ok(cell(data, row0, col0, 0, c)?.as_bytes().to_vec())
        })
        .collect::<Result<Vec<_>, Refusal>>()?;
    // calamine holds the whole sheet (O(sheet) memory), but cells are converted lazily:
    // an unsupported cell refuses on the row that holds it, after the rows before it.
    let mut r = 0usize;
    let rows = std::iter::from_fn(move || {
        r += 1;
        if r >= height {
            return None;
        }
        let cells = (0..width)
            .map(|c| cell(range.get((r, c)).unwrap_or(&Data::Empty), row0, col0, r, c))
            .collect::<Result<Vec<_>, _>>();
        Some(cells.map(|cells| Row {
            index: r as u64,
            cells,
        }))
    });
    Ok(Opened {
        headers,
        rows: Box::new(rows),
        delimiter: None,
        sheet: Some(selected),
    })
}

/// Exact name wins; otherwise a 0-based index into workbook order.
fn resolve_sheet(selector: &SheetSel, names: &[String]) -> Option<String> {
    match selector {
        SheetSel::Name(name) => names.contains(name).then(|| name.clone()),
        SheetSel::Index(index) => {
            let as_name = index.to_string();
            if names.contains(&as_name) {
                Some(as_name)
            } else {
                names.get(*index).cloned()
            }
        }
    }
}

/// `r` and `c` are offsets inside the used range; coordinates in refusals are
/// absolute 1-based sheet positions.
fn cell(data: &Data, row0: u32, col0: u32, r: usize, c: usize) -> Result<Cell, Refusal> {
    let unsupported = |kind: &str| {
        Refusal::new(RefusalCode::EUnsupportedValue, "unsupported cell type")
            .with("kind", kind)
            .with("row", u64::from(row0) + r as u64 + 1)
            .with("col", u64::from(col0) + c as u64 + 1)
    };
    Ok(match data {
        Data::Empty => Cell::Null,
        Data::String(s) => Cell::from_bytes(s.as_bytes()),
        Data::Int(i) => Cell::from_bytes(i.to_string()),
        Data::Float(f) => {
            Cell::from_bytes(canon_f64(*f).ok_or_else(|| unsupported("non-finite float"))?)
        }
        Data::Bool(b) => Cell::from_bytes(b.to_string()),
        Data::DateTime(dt) => {
            if dt.is_duration() {
                return Err(unsupported("duration"));
            }
            let serial = dt.as_f64();
            // Excel's own range; calamine clamps outside it, which would hide the problem.
            if !(0.0..2_958_466.0).contains(&serial) {
                return Err(unsupported("datetime out of range"));
            }
            let parts = dt.to_ymd_hms_milli();
            if (parts.0, parts.1, parts.2) == (1900, 2, 29) {
                // Excel's 1900 leap-year bug: serial 60 is a date that never existed.
                return Err(unsupported("nonexistent 1900 leap day"));
            }
            if serial < 1.0 {
                // Time-only value. In a 1904 workbook the same serial is also a legitimate
                // 1904-01-01 datetime and the file cannot say which was meant: refuse.
                if parts.0 == 1904 {
                    return Err(unsupported("ambiguous 1904 serial below 1"));
                }
                Cell::from_bytes(
                    excel_time_text(serial).ok_or_else(|| unsupported("time out of range"))?,
                )
            } else {
                Cell::from_bytes(excel_text(parts))
            }
        }
        Data::DateTimeIso(iso) => {
            Cell::from_bytes(iso_datetime_text(iso).ok_or_else(|| unsupported("datetime"))?)
        }
        Data::DurationIso(_) => return Err(unsupported("duration")),
        Data::Error(_) => return Err(unsupported("error cell")),
    })
}
