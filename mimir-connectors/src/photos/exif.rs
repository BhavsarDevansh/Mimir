use std::path::Path;

use chrono::{DateTime, FixedOffset, NaiveDateTime, Utc};

use std::fs::File;
use std::io::BufReader;

use crate::connector::ConnectorError;

// ---------------------------------------------------------------------------
// EXIF extraction
// ---------------------------------------------------------------------------

/// Parsed EXIF fields for one image file. Missing fields are `None`; the
/// connector falls back to the file mtime for the temporal bound and emits no
/// location overlay when GPS is absent.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ExifFields {
    pub(super) datetime: Option<DateTime<Utc>>,
    pub(super) latitude: Option<f64>,
    pub(super) longitude: Option<f64>,
}

impl ExifFields {
    fn empty() -> Self {
        Self {
            datetime: None,
            latitude: None,
            longitude: None,
        }
    }
}

/// Read and parse EXIF metadata from an image file.
///
/// I/O failures (the file vanished or is unreadable) propagate as
/// [`ConnectorError::Io`]. Missing or malformed EXIF yields [`ExifFields`]
/// with all fields `None` — the caller still emits a fact using the file
/// mtime fallback and no location overlay.
pub(super) fn read_exif(path: &Path) -> Result<ExifFields, ConnectorError> {
    let mut file = File::open(path)?;
    let mut reader = BufReader::new(&mut file);
    let exif = match exif::Reader::new().read_from_container(&mut reader) {
        Ok(exif) => exif,
        Err(_) => return Ok(ExifFields::empty()),
    };
    Ok(ExifFields {
        datetime: parse_exif_datetime(&exif),
        latitude: parse_exif_latitude(&exif),
        longitude: parse_exif_longitude(&exif),
    })
}

/// Parse `DateTimeOriginal` (falling back to `DateTimeDigitized` then
/// `DateTime`), applying `OffsetTimeOriginal`/`OffsetTimeDigitized`/`OffsetTime`
/// when present; otherwise the naive timestamp is interpreted as UTC.
pub(super) fn parse_exif_datetime(exif: &exif::Exif) -> Option<DateTime<Utc>> {
    let field = [
        exif::Tag::DateTimeOriginal,
        exif::Tag::DateTimeDigitized,
        exif::Tag::DateTime,
    ]
    .into_iter()
    .find_map(|tag| exif.get_field(tag, exif::In::PRIMARY))?;
    let raw = ascii_value(&field.value)?;
    let naive =
        NaiveDateTime::parse_from_str(raw.trim_end_matches('\0'), "%Y:%m:%d %H:%M:%S").ok()?;
    match parse_offset(exif) {
        Some(offset) => Some(
            naive
                .and_local_timezone(offset)
                .single()?
                .with_timezone(&Utc),
        ),
        None => Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc)),
    }
}

pub(super) fn parse_exif_latitude(exif: &exif::Exif) -> Option<f64> {
    let dms = rational3(exif, exif::Tag::GPSLatitude)?;
    let decimal = dms_to_decimal(dms);
    let negative = matches!(
        ascii_first_byte(exif, exif::Tag::GPSLatitudeRef),
        Some(b'S')
    );
    let signed = if negative { -decimal } else { decimal };
    // Reject malformed EXIF (zero-denominator rationals → `NaN`, or a corrupt
    // DMS triple outside the valid range) so garbage never reaches the
    // location overlay / proximity queries. The `took_photo` fact is still
    // emitted; only the location is dropped.
    (signed.is_finite() && (-90.0..=90.0).contains(&signed)).then_some(signed)
}

pub(super) fn parse_exif_longitude(exif: &exif::Exif) -> Option<f64> {
    let dms = rational3(exif, exif::Tag::GPSLongitude)?;
    let decimal = dms_to_decimal(dms);
    let negative = matches!(
        ascii_first_byte(exif, exif::Tag::GPSLongitudeRef),
        Some(b'W')
    );
    let signed = if negative { -decimal } else { decimal };
    (signed.is_finite() && (-180.0..=180.0).contains(&signed)).then_some(signed)
}

/// Extract the first ASCII value of a field as a borrowed `&str` (NUL-trimmed
/// by the caller).
fn ascii_value(value: &exif::Value) -> Option<&str> {
    if let exif::Value::Ascii(vec) = value {
        vec.first()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
    } else {
        None
    }
}

/// First byte of an ASCII-tag field (e.g. `GPSLatitudeRef` = `b'N'`/`b'S'`).
fn ascii_first_byte(exif: &exif::Exif, tag: exif::Tag) -> Option<u8> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    if let exif::Value::Ascii(vec) = &field.value {
        return vec.first().and_then(|bytes| bytes.first()).copied();
    }
    None
}

/// Three rationals (degrees, minutes, seconds) → `[f64; 3]`.
fn rational3(exif: &exif::Exif, tag: exif::Tag) -> Option<[f64; 3]> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    if let exif::Value::Rational(vec) = &field.value {
        if vec.len() == 3 {
            return Some([vec[0].to_f64(), vec[1].to_f64(), vec[2].to_f64()]);
        }
    }
    None
}

fn dms_to_decimal([deg, min, sec]: [f64; 3]) -> f64 {
    deg + min / 60.0 + sec / 3600.0
}

/// Parse an EXIF `OffsetTime*` ASCII string ("±HH:MM") into a [`FixedOffset`].
pub(super) fn parse_offset(exif: &exif::Exif) -> Option<FixedOffset> {
    let field = [
        exif::Tag::OffsetTimeOriginal,
        exif::Tag::OffsetTimeDigitized,
        exif::Tag::OffsetTime,
    ]
    .into_iter()
    .find_map(|tag| exif.get_field(tag, exif::In::PRIMARY))?;
    let raw = ascii_value(&field.value)?;
    let raw = raw.trim_end_matches('\0');
    let (sign, rest) = match raw.as_bytes() {
        [b'+', ..] => (1i32, &raw[1..]),
        [b'-', ..] => (-1i32, &raw[1..]),
        _ => return None,
    };
    let (hh, mm) = rest.split_once(':')?;
    let hours: i32 = hh.parse().ok()?;
    let minutes: i32 = mm.parse().ok()?;
    FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))
}
