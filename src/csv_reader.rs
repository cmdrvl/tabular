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
use crate::{Cell, CsvOptions, Format, Grid, Row, check_row_limit, missing_headers, no_data_rows};

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

pub(crate) fn read(
    path: &Path,
    format: Format,
    opts: &CsvOptions,
    max_rows: Option<u64>,
) -> Result<(Grid, u8), Refusal> {
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

    let mut sample = read_sample(&mut file, body_offset)?;
    if sample.is_empty() {
        return Err(missing_headers());
    }
    check_text_sample(&sample)?;
    if body_offset + sample.len() as u64 >= file_len {
        // whole remainder fits in the sample
    } else if let Some(last_newline) = sample.iter().rposition(|&b| b == b'\n') {
        // Avoid probing a record cut in half by the sample boundary.
        sample.truncate(last_newline + 1);
    }

    let pinned = opts.delimiter.or(if format == Format::Tsv {
        Some(b'\t')
    } else {
        None
    });
    let dialect = match pinned {
        Some(delimiter) => Dialect {
            delimiter,
            escape: Escape::None,
        },
        None => detect(&sample)?,
    };

    file.seek(SeekFrom::Start(body_offset))
        .map_err(|e| Refusal::io(&e))?;
    let mut builder = csv::ReaderBuilder::new();
    builder
        .has_headers(false)
        .flexible(false)
        .delimiter(dialect.delimiter)
        .escape(match dialect.escape {
            Escape::None => None,
            Escape::Backslash => Some(b'\\'),
        });
    let mut reader = builder.from_reader(BufReader::new(file));
    let mut records = reader.byte_records();

    let headers: Vec<Vec<u8>> = match records.next() {
        Some(Ok(record)) => record.iter().map(<[u8]>::to_vec).collect(),
        Some(Err(error)) => return Err(csv_refusal(&error)),
        None => return Err(missing_headers()),
    };

    let mut rows = Vec::new();
    for record in records {
        let record = record.map_err(|e| csv_refusal(&e))?;
        let index = rows.len() as u64 + 1;
        check_row_limit(index, max_rows)?;
        rows.push(Row {
            index,
            cells: record.iter().map(Cell::from_bytes).collect(),
        });
    }
    if rows.is_empty() {
        return Err(no_data_rows());
    }
    Ok(((headers, rows), dialect.delimiter))
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

fn detect(sample: &[u8]) -> Result<Dialect, Refusal> {
    let probes: Vec<Probe> = CANDIDATE_DELIMITERS
        .iter()
        .filter_map(|&delimiter| {
            probe(sample, delimiter, Escape::None)
                .or_else(|| probe(sample, delimiter, Escape::Backslash))
        })
        .collect();
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

fn probe(sample: &[u8], delimiter: u8, escape: Escape) -> Option<Probe> {
    let mut builder = csv::ReaderBuilder::new();
    builder
        .has_headers(false)
        .delimiter(delimiter)
        .escape(match escape {
            Escape::None => None,
            Escape::Backslash => Some(b'\\'),
        });
    let mut reader = builder.from_reader(sample);
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
