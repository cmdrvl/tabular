//! Canonical text for dates, times, timestamps and decimals (v1).
//!
//! Every function here is a pure function of its numeric inputs: no clock, no
//! locale, no time zone database. Output is ASCII and fixed-width where noted.
//!
//! | value                         | canonical text                                   |
//! |-------------------------------|--------------------------------------------------|
//! | date                          | `YYYY-MM-DD` (years 0000-9999 only)              |
//! | time of day                   | `HH:MM:SS.fff` / `.ffffff` / `.fffffffff`        |
//! | timestamp, UTC-adjusted       | `YYYY-MM-DDTHH:MM:SS.fff[fff[fff]]Z`             |
//! | timestamp, not UTC-adjusted   | same without the `Z` (a local/naive wall clock)  |
//! | decimal                       | exact digits, scale preserved: `-12.50`, `0.003` |
//!
//! The fraction width follows the source unit (milliseconds 3, microseconds 6,
//! nanoseconds 9) and never varies with the value, so equal instants in equal
//! units are equal bytes.

const MS_PER_DAY: i64 = 86_400_000;

/// Days since 1970-01-01 to `(year, month, day)` in the proleptic Gregorian
/// calendar (Howard Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `YYYY-MM-DD`, or `None` when the year is outside 0000..=9999.
pub(crate) fn date_text(days_since_epoch: i64) -> Option<String> {
    let (y, m, d) = civil_from_days(days_since_epoch);
    (0..=9999)
        .contains(&y)
        .then(|| format!("{y:04}-{m:02}-{d:02}"))
}

/// Fraction digits for a sub-second unit: 3, 6 or 9.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Unit {
    Millis,
    Micros,
    Nanos,
}

impl Unit {
    const fn per_second(self) -> i64 {
        match self {
            Self::Millis => 1_000,
            Self::Micros => 1_000_000,
            Self::Nanos => 1_000_000_000,
        }
    }

    const fn digits(self) -> usize {
        match self {
            Self::Millis => 3,
            Self::Micros => 6,
            Self::Nanos => 9,
        }
    }
}

fn clock(units_of_day: i64, unit: Unit) -> String {
    let per_s = unit.per_second();
    let secs = units_of_day.div_euclid(per_s);
    let frac = units_of_day.rem_euclid(per_s);
    format!(
        "{:02}:{:02}:{:02}.{frac:0width$}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60,
        width = unit.digits()
    )
}

/// Time of day from units since midnight; `None` outside `[0, 24h)`.
pub(crate) fn time_text(units_since_midnight: i64, unit: Unit) -> Option<String> {
    let day = 86_400 * unit.per_second();
    (0..day)
        .contains(&units_since_midnight)
        .then(|| clock(units_since_midnight, unit))
}

/// Timestamp from units since the Unix epoch. `utc` appends `Z`.
pub(crate) fn timestamp_text(units_since_epoch: i64, unit: Unit, utc: bool) -> Option<String> {
    let per_day = 86_400 * unit.per_second();
    let days = units_since_epoch.div_euclid(per_day);
    let in_day = units_since_epoch.rem_euclid(per_day);
    let date = date_text(days)?;
    Some(format!(
        "{date}T{}{}",
        clock(in_day, unit),
        if utc { "Z" } else { "" }
    ))
}

/// Excel serial (already split by calamine into civil parts) to canonical text.
/// A whole-day value is a date; otherwise a naive millisecond timestamp.
pub(crate) fn excel_text(
    (year, month, day, hour, minute, second, milli): (u16, u8, u8, u8, u8, u8, u16),
) -> String {
    if hour == 0 && minute == 0 && second == 0 && milli == 0 {
        format!("{year:04}-{month:02}-{day:02}")
    } else {
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milli:03}")
    }
}

/// An ISO-8601 cell (`t="d"` in XLSX) re-rendered canonically: `YYYY-MM-DD`, or
/// `YYYY-MM-DDTHH:MM:SS[.f{1,3}]` padded to three fraction digits. Anything
/// with a zone designator or another shape is refused (`None`): the file does
/// not carry the zone, so inventing one would be a guess.
pub(crate) fn iso_datetime_text(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let digits = |s: &[u8]| s.iter().all(u8::is_ascii_digit);
    let num = |s: &[u8]| -> u32 { s.iter().fold(0, |a, d| a * 10 + u32::from(d - b'0')) };
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    if !(digits(&b[0..4]) && digits(&b[5..7]) && digits(&b[8..10])) {
        return None;
    }
    let (y, mo, d) = (num(&b[0..4]), num(&b[5..7]), num(&b[8..10]));
    if !(1..=12).contains(&mo) || !(1..=days_in_month(y, mo)).contains(&d) {
        return None;
    }
    let date = format!("{y:04}-{mo:02}-{d:02}");
    if b.len() == 10 {
        return Some(date);
    }
    if b[10] != b'T' || b.len() < 19 || b[13] != b':' || b[16] != b':' {
        return None;
    }
    if !(digits(&b[11..13]) && digits(&b[14..16]) && digits(&b[17..19])) {
        return None;
    }
    let (h, mi, s) = (num(&b[11..13]), num(&b[14..16]), num(&b[17..19]));
    if h > 23 || mi > 59 || s > 59 {
        return None;
    }
    let frac = match &b[19..] {
        [] => 0,
        [b'.', rest @ ..] if (1..=3).contains(&rest.len()) && digits(rest) => {
            num(rest) * 10u32.pow(3 - rest.len() as u32)
        }
        _ => return None,
    };
    Some(format!("{date}T{h:02}:{mi:02}:{s:02}.{frac:03}"))
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `HH:MM:SS.fff` from an Excel time-only serial in `[0, 1)`, rounded to the millisecond.
pub(crate) fn excel_time_text(fraction_of_day: f64) -> Option<String> {
    if !(0.0..1.0).contains(&fraction_of_day) {
        return None;
    }
    let ms = (fraction_of_day * MS_PER_DAY as f64).round() as i64;
    // Rounding up to a full day would be 24:00:00; refuse rather than wrap.
    time_text(ms, Unit::Millis)
}

/// Exact decimal text from a big-endian two's-complement unscaled value and a
/// non-negative `scale`. Scale is preserved (`1250` at scale 2 is `12.50`).
pub(crate) fn decimal_text(unscaled_be: &[u8], scale: i32) -> Option<String> {
    let scale = usize::try_from(scale).ok()?;
    if unscaled_be.is_empty() {
        return None;
    }
    let negative = unscaled_be[0] & 0x80 != 0;
    // Magnitude bytes: two's complement negate when negative.
    let mut magnitude: Vec<u8> = unscaled_be.to_vec();
    if negative {
        for b in &mut magnitude {
            *b = !*b;
        }
        for b in magnitude.iter_mut().rev() {
            let (v, carry) = b.overflowing_add(1);
            *b = v;
            if !carry {
                break;
            }
        }
    }
    let digits = to_decimal_digits(&magnitude);
    let (int_part, frac_part) = if scale == 0 {
        (digits, String::new())
    } else {
        let padded = format!("{digits:0>width$}", width = scale + 1);
        let split = padded.len() - scale;
        (padded[..split].to_owned(), padded[split..].to_owned())
    };
    let sign = if negative && digits_nonzero(&int_part, &frac_part) {
        "-"
    } else {
        ""
    };
    Some(if frac_part.is_empty() {
        format!("{sign}{int_part}")
    } else {
        format!("{sign}{int_part}.{frac_part}")
    })
}

fn digits_nonzero(int_part: &str, frac_part: &str) -> bool {
    int_part.bytes().chain(frac_part.bytes()).any(|b| b != b'0')
}

/// Base-10 digits of an unsigned big-endian integer (no leading zeros; `0` for zero).
fn to_decimal_digits(be: &[u8]) -> String {
    // Little-endian base 2^32 limbs, repeatedly divided by 10^9.
    let mut limbs: Vec<u32> = Vec::new();
    for chunk in be.rchunks(4) {
        let mut v = 0u32;
        for &b in chunk {
            v = (v << 8) | u32::from(b);
        }
        limbs.push(v);
    }
    while limbs.last() == Some(&0) {
        limbs.pop();
    }
    if limbs.is_empty() {
        return "0".to_owned();
    }
    let mut groups: Vec<u32> = Vec::new();
    while !limbs.is_empty() {
        let mut rem = 0u64;
        for limb in limbs.iter_mut().rev() {
            let cur = (rem << 32) | u64::from(*limb);
            *limb = (cur / 1_000_000_000) as u32;
            rem = cur % 1_000_000_000;
        }
        groups.push(rem as u32);
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
    }
    let mut out = format!("{}", groups.pop().unwrap_or(0));
    for g in groups.iter().rev() {
        out.push_str(&format!("{g:09}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_around_the_epoch_and_leap_days() {
        assert_eq!(date_text(0).as_deref(), Some("1970-01-01"));
        assert_eq!(date_text(-1).as_deref(), Some("1969-12-31"));
        assert_eq!(date_text(19_782).as_deref(), Some("2024-02-29"));
        assert_eq!(date_text(-719_528).as_deref(), Some("0000-01-01"));
        assert_eq!(date_text(2_932_896).as_deref(), Some("9999-12-31"));
        assert_eq!(date_text(2_932_897), None);
        assert_eq!(date_text(-719_529), None);
    }

    #[test]
    fn timestamps_have_fixed_width_fractions() {
        assert_eq!(
            timestamp_text(1_541_186_529_153, Unit::Millis, true).as_deref(),
            Some("2018-11-02T19:22:09.153Z")
        );
        assert_eq!(
            timestamp_text(1_541_186_529_153_123, Unit::Micros, false).as_deref(),
            Some("2018-11-02T19:22:09.153123")
        );
        assert_eq!(
            timestamp_text(1_541_186_529_000_000_007, Unit::Nanos, true).as_deref(),
            Some("2018-11-02T19:22:09.000000007Z")
        );
        // Negative instants floor correctly: 1 ms before the epoch.
        assert_eq!(
            timestamp_text(-1, Unit::Millis, true).as_deref(),
            Some("1969-12-31T23:59:59.999Z")
        );
        assert_eq!(
            timestamp_text(0, Unit::Micros, true).as_deref(),
            Some("1970-01-01T00:00:00.000000Z")
        );
    }

    #[test]
    fn times_of_day() {
        assert_eq!(time_text(0, Unit::Millis).as_deref(), Some("00:00:00.000"));
        assert_eq!(
            time_text(86_399_999, Unit::Millis).as_deref(),
            Some("23:59:59.999")
        );
        assert_eq!(time_text(86_400_000, Unit::Millis), None);
        assert_eq!(time_text(-1, Unit::Millis), None);
        assert_eq!(excel_time_text(0.5).as_deref(), Some("12:00:00.000"));
        assert_eq!(excel_time_text(1.0), None);
    }

    #[test]
    fn iso_cells() {
        assert_eq!(
            iso_datetime_text("2024-02-29").as_deref(),
            Some("2024-02-29")
        );
        assert_eq!(
            iso_datetime_text("2024-02-29T01:02:03").as_deref(),
            Some("2024-02-29T01:02:03.000")
        );
        assert_eq!(
            iso_datetime_text("2024-02-29T01:02:03.5").as_deref(),
            Some("2024-02-29T01:02:03.500")
        );
        for bad in [
            "2023-02-29",
            "2024-13-01",
            "2024-01-01T24:00:00",
            "2024-01-01T00:00:00Z",
            "2024-01-01T00:00:00+01:00",
            "2024-01-01T00:00:00.1234",
            "2024-01-01 00:00:00",
            "24-01-01",
            "",
        ] {
            assert_eq!(iso_datetime_text(bad), None, "{bad}");
        }
    }

    #[test]
    fn excel_civil_parts() {
        assert_eq!(excel_text((2025, 10, 13, 0, 0, 0, 0)), "2025-10-13");
        assert_eq!(
            excel_text((2025, 10, 13, 12, 59, 2, 400)),
            "2025-10-13T12:59:02.400"
        );
    }

    #[test]
    fn decimals_are_exact_and_scale_preserving() {
        let be = |v: i128| v.to_be_bytes().to_vec();
        assert_eq!(decimal_text(&be(1250), 2).as_deref(), Some("12.50"));
        assert_eq!(decimal_text(&be(-1250), 2).as_deref(), Some("-12.50"));
        assert_eq!(decimal_text(&be(3), 3).as_deref(), Some("0.003"));
        assert_eq!(decimal_text(&be(-3), 3).as_deref(), Some("-0.003"));
        assert_eq!(decimal_text(&be(0), 2).as_deref(), Some("0.00"));
        assert_eq!(decimal_text(&be(42), 0).as_deref(), Some("42"));
        assert_eq!(decimal_text(&be(-42), 0).as_deref(), Some("-42"));
        // Short encodings (Parquet FLBA / int32-backed) sign-extend.
        assert_eq!(decimal_text(&[0xFF], 0).as_deref(), Some("-1"));
        assert_eq!(decimal_text(&[0x80], 0).as_deref(), Some("-128"));
        assert_eq!(decimal_text(&[0x04, 0xD2], 2).as_deref(), Some("12.34"));
        // 38 digits, beyond i64, and a 32-byte value beyond i128.
        let big = 99_999_999_999_999_999_999_999_999_999_999_999_999i128;
        assert_eq!(
            decimal_text(&be(big), 10).as_deref(),
            Some("9999999999999999999999999999.9999999999")
        );
        let mut huge = vec![0u8; 16];
        huge.extend(be(big));
        assert_eq!(
            decimal_text(&huge, 0).as_deref(),
            Some("99999999999999999999999999999999999999")
        );
        assert_eq!(decimal_text(&be(1), -1), None);
        assert_eq!(decimal_text(&[], 0), None);
    }
}
