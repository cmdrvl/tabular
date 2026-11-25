//! Format resolution (extension, then magic bytes) and text-encoding checks.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::Format;
use crate::refusal::{Refusal, RefusalCode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Magic {
    Parquet,
    Zip,
    Ole,
    Text,
}

impl Magic {
    fn as_str(self) -> &'static str {
        match self {
            Self::Parquet => "parquet",
            Self::Zip => "zip",
            Self::Ole => "ole",
            Self::Text => "text",
        }
    }
}

pub(crate) fn format_from_extension(extension: &str) -> Option<Format> {
    match extension {
        "csv" => Some(Format::Csv),
        "tsv" => Some(Format::Tsv),
        "jsonl" | "ndjson" => Some(Format::Jsonl),
        "parquet" => Some(Format::Parquet),
        "xlsx" | "xlsm" | "xls" => Some(Format::Xlsx),
        _ => None,
    }
}

pub(crate) fn sniff(path: &Path) -> Result<Magic, Refusal> {
    let mut file = File::open(path).map_err(|e| Refusal::io(&e))?;
    let len = file.metadata().map_err(|e| Refusal::io(&e))?.len();
    let mut head = [0u8; 8];
    let got = read_up_to(&mut file, &mut head).map_err(|e| Refusal::io(&e))?;
    let head = &head[..got];
    if head.starts_with(b"PAR1") && len >= 8 {
        let mut tail = [0u8; 4];
        file.seek(SeekFrom::End(-4)).map_err(|e| Refusal::io(&e))?;
        let got = read_up_to(&mut file, &mut tail).map_err(|e| Refusal::io(&e))?;
        if &tail[..got] == b"PAR1" {
            return Ok(Magic::Parquet);
        }
    }
    if head.starts_with(b"PK\x03\x04") {
        return Ok(Magic::Zip);
    }
    if head.starts_with(&[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) {
        return Ok(Magic::Ole);
    }
    Ok(Magic::Text)
}

fn read_up_to(file: &mut File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        let n = file.read(&mut buf[total..])?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

/// Resolve the format from an explicit override or the extension, then require
/// the magic bytes to agree.
pub(crate) fn resolve(path: &Path, requested: Option<Format>) -> Result<Format, Refusal> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let format = match requested.or_else(|| format_from_extension(&extension)) {
        Some(format) => format,
        None => {
            return Err(Refusal::new(
                RefusalCode::EFormat,
                "unsupported or missing file extension",
            )
            .with("extension", extension));
        }
    };
    let magic = sniff(path)?;
    let agrees = match format {
        Format::Parquet => magic == Magic::Parquet,
        Format::Xlsx => matches!(magic, Magic::Zip | Magic::Ole),
        Format::Csv | Format::Tsv | Format::Jsonl => magic == Magic::Text,
    };
    if agrees {
        Ok(format)
    } else {
        Err(Refusal::new(
            RefusalCode::EFormat,
            "file content does not match the format",
        )
        .with("expected", format.as_str())
        .with("detected", magic.as_str())
        .with("extension", extension))
    }
}

/// NUL bytes are looked for in this many bytes after the BOM, as shape does (8 KiB).
const NUL_SCAN_BYTES: usize = 8 * 1024;

/// Check a leading sample of a text file. Returns the number of BOM bytes to skip
/// (3 for UTF-8, else 0). UTF-16/32 BOMs, and NUL bytes in the first 8 KiB after the BOM
/// (shape's window), refuse `E_ENCODING`; NULs after that pass through as bytes.
pub(crate) fn check_text_sample(sample: &[u8]) -> Result<usize, Refusal> {
    let bad_bom = sample.starts_with(&[0xFF, 0xFE])
        || sample.starts_with(&[0xFE, 0xFF])
        || sample.starts_with(&[0x00, 0x00, 0xFE, 0xFF]);
    let after_bom = sample.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(sample);
    if bad_bom || after_bom.iter().take(NUL_SCAN_BYTES).any(|&b| b == 0) {
        return Err(Refusal::new(
            RefusalCode::EEncoding,
            "unsupported encoding (UTF-16/32 BOM or NUL bytes)",
        ));
    }
    Ok(if sample.starts_with(&[0xEF, 0xBB, 0xBF]) {
        3
    } else {
        0
    })
}
