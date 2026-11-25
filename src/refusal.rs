//! Typed refusals. Details and context carry codes, type names, coordinates,
//! sheet names and header names only, never cell values and never library
//! error text (which could quote data).

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;
use serde_json::Value;

/// Reader-level refusal codes. The first seven match `shape`'s existing codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum RefusalCode {
    EIo,
    EEncoding,
    ECsvParse,
    EEmpty,
    EHeaders,
    EDialect,
    ETooLarge,
    EFormat,
    EJsonlParse,
    EParquetParse,
    EXlsxParse,
    EAmbiguousSheet,
    ESheetNotFound,
    EUnsupportedValue,
    EEncrypted,
}

impl RefusalCode {
    pub const ALL: [RefusalCode; 15] = [
        Self::EIo,
        Self::EEncoding,
        Self::ECsvParse,
        Self::EEmpty,
        Self::EHeaders,
        Self::EDialect,
        Self::ETooLarge,
        Self::EFormat,
        Self::EJsonlParse,
        Self::EParquetParse,
        Self::EXlsxParse,
        Self::EAmbiguousSheet,
        Self::ESheetNotFound,
        Self::EUnsupportedValue,
        Self::EEncrypted,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::EIo => "E_IO",
            Self::EEncoding => "E_ENCODING",
            Self::ECsvParse => "E_CSV_PARSE",
            Self::EEmpty => "E_EMPTY",
            Self::EHeaders => "E_HEADERS",
            Self::EDialect => "E_DIALECT",
            Self::ETooLarge => "E_TOO_LARGE",
            Self::EFormat => "E_FORMAT",
            Self::EJsonlParse => "E_JSONL_PARSE",
            Self::EParquetParse => "E_PARQUET_PARSE",
            Self::EXlsxParse => "E_XLSX_PARSE",
            Self::EAmbiguousSheet => "E_AMBIGUOUS_SHEET",
            Self::ESheetNotFound => "E_SHEET_NOT_FOUND",
            Self::EUnsupportedValue => "E_UNSUPPORTED_VALUE",
            Self::EEncrypted => "E_ENCRYPTED",
        }
    }
}

impl Serialize for RefusalCode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl fmt::Display for RefusalCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Refusal {
    pub code: RefusalCode,
    pub detail: String,
    /// Sorted keys (R-005). See the design doc for the keys per code.
    pub context: BTreeMap<String, Value>,
}

impl Refusal {
    pub(crate) fn new(code: RefusalCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
            context: BTreeMap::new(),
        }
    }

    pub(crate) fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.context.insert(key.to_owned(), value.into());
        self
    }

    pub(crate) fn io(error: &std::io::Error) -> Self {
        // Only the error kind: OS error text can include paths.
        Self::new(RefusalCode::EIo, "cannot read input file")
            .with("kind", format!("{:?}", error.kind()))
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.detail)
    }
}

impl std::error::Error for Refusal {}
