//! CSV/TSV reader with delimiter auto-detection.
//!
//! The detector re-implements `shape`'s scoring (candidates `, \t ; | ^`, 64 KiB
//! sample, records-parsed / mode-count / mode-fields ordering, `E_DIALECT` on an
//! ambiguous tie or a single-column winner). It is hand-written here, not
//! diffed against `shape`; see the README for what is and is not verified.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::detect::check_text_sample;
use crate::refusal::{Refusal, RefusalCode};
use crate::{Cell, CsvOptions, Format, Opened, Row, RowStream, missing_headers};

const CANDIDATE_DELIMITERS: [u8; 5] = *b",\t;|^";
const SAMPLE_BYTES: usize = 64 * 1024;
const MAX_SAMPLE_RECORDS: usize = 200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Escape {
    None,
    Backslash,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Dialect {
    delimiter: u8,
    escape: Escape,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct Score {
    records_parsed: u64,
    mode_count: u64,
    mode_fields: u64,
}

struct Probe {
    delimiter: u8,
    escape: Escape,
    score: Score,
    projection: Vec<Vec<u8>>,
}

pub(crate) fn open(path: &Path, format: Format, opts: &CsvOptions) -> Result<Opened, Refusal> {
    let mut file = File::open(path).map_err(|e| Refusal::io(&e))?;
    let file_len = file.metadata().map_err(|e| Refusal::io(&e))?.len();

    let head = read_sample(&mut file, 0)?;
    let bom = check_text_sample(&head)?;

    let mut body_offset = bom as u64;
    if opts.skip_rows > 0 {
        file.seek(SeekFrom::Start(body_offset))
            .map_err(|e| Refusal::io(&e))?;
        let mut lines = BufReader::new(&mut file);
        let mut scratch = Vec::new();
        for _ in 0..opts.skip_rows {
            scratch.clear();
            let n = lines
                .read_until(b'\n', &mut scratch)
                .map_err(|e| Refusal::io(&e))?;
            body_offset += n as u64;
            if n == 0 {
                break;
            }
        }
    }

    let sample = read_sample(&mut file, body_offset)?;
    if sample.is_empty() {
        return Err(missing_headers());
    }
    check_text_sample(&sample)?;

    // Same precedence as shape: a pinned delimiter, then an Excel `sep=<byte>` first line,
    // then auto-detection. A pinned or `sep=` delimiter tries RFC 4180 quoting first and
    // falls back to backslash escaping if the header or first record will not parse;
    // an auto-detected dialect already carries its escape mode.
    let pinned = opts.delimiter.or(if format == Format::Tsv {
        Some(b'\t')
    } else {
        None
    });
    let sep = sep_directive(&sample);
    let (delimiter, consumed, modes): (u8, u64, &[Escape]) = if let Some(delimiter) = pinned {
        (
            delimiter,
            sep.map_or(0, |s| s.consumed),
            &[Escape::None, Escape::Backslash],
        )
    } else if let Some(sep) = sep {
        (
            sep.delimiter,
            sep.consumed,
            &[Escape::None, Escape::Backslash],
        )
    } else {
        let dialect = detect_dialect(path, &sample, body_offset, file_len)?;
        let modes: &[Escape] = match dialect.escape {
            Escape::None => &[Escape::None],
            Escape::Backslash => &[Escape::Backslash],
        };
        (dialect.delimiter, 0, modes)
    };

    for (attempt, escape) in modes.iter().enumerate() {
        match start(path, body_offset + consumed, delimiter, *escape) {
            Ok((headers, rows)) => {
                return Ok(Opened {
                    headers,
                    rows,
                    delimiter: Some(delimiter),
                    sheet: None,
                });
            }
            Err(refusal) if attempt + 1 < modes.len() && refusal.code == RefusalCode::ECsvParse => {
                continue;
            }
            Err(refusal) => return Err(refusal),
        }
    }
    Err(missing_headers())
}

/// Open the body at `offset`, read the header and probe the first data record (a parse
/// error there is reported now, so the escape fallback can react), and return the headers
/// plus a lazy stream that starts with that first record.
fn start(
    path: &Path,
    offset: u64,
    delimiter: u8,
    escape: Escape,
) -> Result<(Vec<Vec<u8>>, RowStream), Refusal> {
    let mut file = File::open(path).map_err(|e| Refusal::io(&e))?;
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| Refusal::io(&e))?;
    let mut builder = csv::ReaderBuilder::new();
    builder
        .has_headers(false)
        .flexible(false)
        .delimiter(delimiter)
        .escape(match escape {
            Escape::None => None,
            Escape::Backslash => Some(b'\\'),
        });
    let reader = builder.from_reader(BufReader::new(file));
    let mut records = reader.into_byte_records();

    let headers: Vec<Vec<u8>> = match records.next() {
        Some(Ok(record)) => record.iter().map(<[u8]>::to_vec).collect(),
        Some(Err(error)) => return Err(csv_refusal(&error)),
        None => return Err(missing_headers()),
    };
    let first = match records.next() {
        Some(Err(error)) => return Err(csv_refusal(&error)),
        other => other,
    };

    let mut index = 0u64;
    let rows = first.into_iter().chain(records).map(move |record| {
        let record = record.map_err(|e| csv_refusal(&e))?;
        index += 1;
        Ok(Row {
            index,
            cells: record.iter().map(Cell::from_bytes).collect(),
        })
    });
    Ok((headers, Box::new(rows)))
}

/// An Excel `sep=<byte>` first line, exactly as shape reads it: five bytes (an optional
/// `\r` before the newline is ignored), a supported delimiter byte.
#[derive(Clone, Copy)]
struct Sep {
    delimiter: u8,
    consumed: u64,
}

fn sep_directive(sample: &[u8]) -> Option<Sep> {
    let line_end = sample
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(sample.len());
    let consumed = if line_end < sample.len() {
        line_end + 1
    } else {
        line_end
    };
    let mut line = &sample[..line_end];
    if line.last() == Some(&b'\r') {
        line = &line[..line.len() - 1];
    }
    if line.len() != 5 || !line.starts_with(b"sep=") {
        return None;
    }
    let delimiter = line[4];
    let supported = delimiter.is_ascii() && !matches!(delimiter, b'"' | b'\n' | b'\r' | 0 | 0x7f);
    supported.then_some(Sep {
        delimiter,
        consumed: consumed as u64,
    })
}

fn read_sample(file: &mut File, offset: u64) -> Result<Vec<u8>, Refusal> {
    file.seek(SeekFrom::Start(offset))
        .map_err(|e| Refusal::io(&e))?;
    let mut sample = Vec::new();
    file.by_ref()
        .take(SAMPLE_BYTES as u64)
        .read_to_end(&mut sample)
        .map_err(|e| Refusal::io(&e))?;
    Ok(sample)
}

fn csv_refusal(error: &csv::Error) -> Refusal {
    match error.kind() {
        csv::ErrorKind::Io(io) => Refusal::io(io),
        csv::ErrorKind::UnequalLengths {
            pos,
            expected_len,
            len,
        } => {
            let refusal = Refusal::new(
                RefusalCode::ECsvParse,
                "record has a different field count than the header",
            )
            .with("expected", *expected_len)
            .with("found", *len);
            match pos {
                Some(pos) => refusal.with("line", pos.line()),
                None => refusal,
            }
        }
        _ => {
            let refusal = Refusal::new(RefusalCode::ECsvParse, "malformed CSV record");
            match error.position() {
                Some(pos) => refusal.with("line", pos.line()),
                None => refusal,
            }
        }
    }
}

/// shape's detection flow: score the first 64 KiB (cut mid-record or not); if that does
/// not yield a dialect and the file is longer, score the first records of the WHOLE file
/// instead. Only the first 200 records are ever parsed, so the fallback streams the file
/// rather than loading it.
fn detect_dialect(
    path: &Path,
    sample: &[u8],
    body_offset: u64,
    file_len: u64,
) -> Result<Dialect, Refusal> {
    match detect(&|| Ok(Box::new(sample))) {
        Err(_) if body_offset + (sample.len() as u64) < file_len => detect(&|| {
            let mut file = File::open(path).map_err(|e| Refusal::io(&e))?;
            file.seek(SeekFrom::Start(body_offset))
                .map_err(|e| Refusal::io(&e))?;
            Ok(Box::new(BufReader::new(file)))
        }),
        other => other,
    }
}

type MakeReader<'a> = &'a dyn Fn() -> Result<Box<dyn Read + 'a>, Refusal>;

fn detect(make: MakeReader<'_>) -> Result<Dialect, Refusal> {
    let mut probes: Vec<Probe> = Vec::new();
    for &delimiter in &CANDIDATE_DELIMITERS {
        let found = match probe(make()?, delimiter, Escape::None) {
            Some(p) => Some(p),
            None => probe(make()?, delimiter, Escape::Backslash),
        };
        probes.extend(found);
    }
    let all_candidates = || {
        CANDIDATE_DELIMITERS
            .iter()
            .map(|&d| hex(d))
            .collect::<Vec<_>>()
    };

    let Some(best) = probes.iter().map(|p| p.score).max() else {
        return Err(dialect_refusal(all_candidates()));
    };
    let tied: Vec<&Probe> = probes.iter().filter(|p| p.score == best).collect();
    let winner = tied[0];
    if tied.len() > 1 && !tied.iter().all(|p| p.projection == winner.projection) {
        return Err(dialect_refusal(
            tied.iter().map(|p| hex(p.delimiter)).collect(),
        ));
    }
    if winner.score.mode_fields <= 1 {
        return Err(dialect_refusal(vec![hex(winner.delimiter)]));
    }
    Ok(Dialect {
        delimiter: winner.delimiter,
        escape: winner.escape,
    })
}

fn dialect_refusal(candidates: Vec<String>) -> Refusal {
    Refusal::new(RefusalCode::EDialect, "delimiter ambiguous or undetectable")
        .with("candidates", candidates)
}

fn hex(byte: u8) -> String {
    format!("0x{byte:02x}")
}

fn probe(source: Box<dyn Read + '_>, delimiter: u8, escape: Escape) -> Option<Probe> {
    let mut builder = csv::ReaderBuilder::new();
    builder
        .has_headers(false)
        .delimiter(delimiter)
        .escape(match escape {
            Escape::None => None,
            Escape::Backslash => Some(b'\\'),
        });
    let mut reader = builder.from_reader(source);
    let mut histogram: BTreeMap<u64, u64> = BTreeMap::new();
    let mut parsed = 0u64;
    let mut projection = Vec::new();
    for record in reader.byte_records().take(MAX_SAMPLE_RECORDS) {
        let record = record.ok()?;
        parsed += 1;
        *histogram.entry(record.len() as u64).or_insert(0) += 1;
        let mut row = Vec::new();
        for (i, field) in record.iter().enumerate() {
            if i > 0 {
                row.push(0x1f);
            }
            row.extend_from_slice(field);
        }
        projection.push(row);
    }
    if parsed == 0 {
        return None;
    }
    let (mode_fields, mode_count) = histogram
        .into_iter()
        .max_by_key(|&(fields, count)| (count, fields))?;
    Some(Probe {
        delimiter,
        escape,
        score: Score {
            records_parsed: parsed,
            mode_count,
            mode_fields,
        },
        projection,
    })
}
