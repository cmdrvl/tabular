//! The cell model: every format renders to the same two-variant cell.

/// One cell. `Text` is never empty: zero-length input is `Null` in every format.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum Cell {
    Null,
    Text(Vec<u8>),
}

/// Type class of a cell, a pure function of its canonical bytes so it is
/// identical across formats by construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellClass {
    Null,
    /// `-?(0|[1-9][0-9]*)`, excluding `-0`.
    Integer,
    /// `-?(0|[1-9][0-9]*)\.[0-9]+`.
    Decimal,
    /// Exactly `true` or `false`.
    Bool,
    Text,
}

impl Cell {
    /// Build from raw bytes; zero-length becomes `Null`.
    pub fn from_bytes(bytes: impl Into<Vec<u8>>) -> Cell {
        let bytes = bytes.into();
        if bytes.is_empty() {
            Cell::Null
        } else {
            Cell::Text(bytes)
        }
    }

    /// The cell's bytes; `Null` is the empty slice. This is the composite-key
    /// hand-off: the key layer sees `Null` as an empty (incomplete) component.
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Cell::Null => &[],
            Cell::Text(bytes) => bytes,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Cell::Null)
    }

    pub fn classify(&self) -> CellClass {
        let Cell::Text(bytes) = self else {
            return CellClass::Null;
        };
        match bytes.as_slice() {
            b"true" | b"false" => CellClass::Bool,
            b"-0" => CellClass::Text,
            other => classify_number(other),
        }
    }
}

fn classify_number(bytes: &[u8]) -> CellClass {
    let digits = bytes.strip_prefix(b"-").unwrap_or(bytes);
    let (int_part, frac_part) = match digits.iter().position(|&b| b == b'.') {
        Some(dot) => (&digits[..dot], Some(&digits[dot + 1..])),
        None => (digits, None),
    };
    let int_ok = !int_part.is_empty()
        && int_part.iter().all(u8::is_ascii_digit)
        && (int_part == b"0" || int_part[0] != b'0');
    if !int_ok {
        return CellClass::Text;
    }
    match frac_part {
        None => CellClass::Integer,
        Some(frac) if !frac.is_empty() && frac.iter().all(u8::is_ascii_digit) => CellClass::Decimal,
        Some(_) => CellClass::Text,
    }
}

/// Canonical float text: integral values with `abs < 1e15` print as integers
/// (`2000.0` -> `2000`); everything else uses the shortest round-trip form.
/// Returns `None` for NaN and infinities.
pub(crate) fn canon_f64(v: f64) -> Option<String> {
    if !v.is_finite() {
        return None;
    }
    Some(if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    })
}

/// Same rule as [`canon_f64`] but shortest round-trip for the `f32` itself, so a
/// Parquet `float` of `0.1` renders `0.1` rather than `0.10000000149011612`.
pub(crate) fn canon_f32(v: f32) -> Option<String> {
    if !v.is_finite() {
        return None;
    }
    Some(if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn class(s: &str) -> CellClass {
        Cell::from_bytes(s.as_bytes()).classify()
    }

    #[test]
    fn empty_is_null_and_text_is_never_empty() {
        assert_eq!(Cell::from_bytes(Vec::new()), Cell::Null);
        assert_eq!(Cell::Null.as_bytes(), b"");
        assert_eq!(Cell::from_bytes(*b" "), Cell::Text(b" ".to_vec()));
    }

    #[test]
    fn classification_is_a_function_of_bytes() {
        assert_eq!(Cell::Null.classify(), CellClass::Null);
        assert_eq!(class("0"), CellClass::Integer);
        assert_eq!(class("-12"), CellClass::Integer);
        assert_eq!(class("100.5"), CellClass::Decimal);
        assert_eq!(class("-0.1"), CellClass::Decimal);
        assert_eq!(class("true"), CellClass::Bool);
        for text in [
            "007", "-0", "1.", ".5", "1e3", "NA", " 1", "1,000", "--1", "-",
        ] {
            assert_eq!(class(text), CellClass::Text, "{text}");
        }
    }

    #[test]
    fn float_rendering_rules() {
        assert_eq!(canon_f64(2000.0).as_deref(), Some("2000"));
        assert_eq!(canon_f64(-0.0).as_deref(), Some("0"));
        assert_eq!(canon_f64(100.5).as_deref(), Some("100.5"));
        assert_eq!(canon_f64(0.1).as_deref(), Some("0.1"));
        assert_eq!(canon_f64(1e21).as_deref(), Some("1000000000000000000000"));
        assert_eq!(canon_f64(f64::NAN), None);
        assert_eq!(canon_f64(f64::INFINITY), None);
        assert_eq!(canon_f32(0.1).as_deref(), Some("0.1"));
        assert_eq!(canon_f32(2.0).as_deref(), Some("2"));
    }
}
