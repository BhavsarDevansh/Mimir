use super::*;

use std::fs;
use std::path::Path;

use chrono::{DateTime, NaiveDateTime};

use crate::photos::cursor::{
    Change, FileSig, PhotosCursor, file_signature, is_image, relative_key,
};
use crate::photos::exif::read_exif;
use crate::photos::scan::stage_file;

pub(super) fn fixture(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures");
    p.push(name);
    p
}

// -- cursor --
#[test]
fn cursor_round_trips_json() {
    let mut cursor = PhotosCursor::default();
    cursor.upsert(
        "a/b.jpg".to_string(),
        FileSig {
            inode: 42,
            mtime_ms: 1_700_000_000_000,
            size: 1024,
        },
    );
    let json = cursor.to_json();
    let back = PhotosCursor::from_json(Some(&json)).unwrap();
    assert_eq!(cursor, back);
}

#[test]
fn cursor_classifies_new_changed_unchanged() {
    let mut cursor = PhotosCursor::default();
    let sig = FileSig {
        inode: 1,
        mtime_ms: 100,
        size: 10,
    };
    cursor.upsert("x.jpg".to_string(), sig);

    assert_eq!(cursor.classify("x.jpg", sig), Change::Unchanged);
    assert_eq!(
        cursor.classify(
            "x.jpg",
            FileSig {
                mtime_ms: 200,
                ..sig
            }
        ),
        Change::NewOrChanged
    );
    assert_eq!(cursor.classify("y.jpg", sig), Change::NewOrChanged);
}

#[test]
fn cursor_prunes_missing() {
    let mut cursor = PhotosCursor::default();
    cursor.upsert(
        "gone.jpg".to_string(),
        FileSig {
            inode: 1,
            mtime_ms: 1,
            size: 1,
        },
    );
    cursor.upsert(
        "kept.jpg".to_string(),
        FileSig {
            inode: 2,
            mtime_ms: 2,
            size: 2,
        },
    );
    let live = [("kept.jpg".to_string(), ())].into_iter().collect();
    cursor.prune_missing(&live);
    assert!(cursor.files.contains_key("kept.jpg"));
    assert!(!cursor.files.contains_key("gone.jpg"));
}

#[test]
fn cursor_none_yields_empty() {
    assert!(PhotosCursor::from_json(None).unwrap().is_empty());
    assert!(PhotosCursor::from_json(Some("")).unwrap().is_empty());
}

// -- path helpers --
#[test]
fn relative_key_normalises_to_forward_slash() {
    let root = Path::new("/tmp/photos");
    let key = relative_key(root, Path::new("/tmp/photos/sub/IMG_001.jpg")).unwrap();
    assert_eq!(key, "sub/IMG_001.jpg");
}

#[test]
fn is_image_matches_default_extensions_case_insensitively() {
    let exts: Vec<String> = DEFAULT_EXTENSIONS.iter().map(|e| e.to_string()).collect();
    assert!(is_image(Path::new("photo.JPG"), &exts));
    assert!(is_image(Path::new("photo.tiff"), &exts));
    assert!(!is_image(Path::new("photo.txt"), &exts));
    assert!(!is_image(Path::new("noext"), &exts));
}

// -- EXIF parsing against committed fixtures --
#[test]
fn parses_jpeg_exif_gps_and_datetime() {
    let fields = read_exif(&fixture("exif.jpg")).unwrap();
    let datetime = fields.datetime.expect("datetime");
    assert_eq!(
        datetime.format("%Y:%m:%d %H:%M:%S").to_string(),
        "2024:05:15 14:30:00"
    );
    // No OffsetTime in the fixture → interpreted as UTC.
    assert_eq!(
        datetime,
        DateTime::<Utc>::from_naive_utc_and_offset(
            NaiveDateTime::parse_from_str("2024:05:15 14:30:00", "%Y:%m:%d %H:%M:%S").unwrap(),
            Utc,
        )
    );
    let lat = fields.latitude.expect("latitude");
    let lon = fields.longitude.expect("longitude");
    assert!((lat - 46.5).abs() < 1e-6, "latitude {lat}");
    assert!((lon - 7.5).abs() < 1e-6, "longitude {lon}");
}

#[test]
fn parses_tiff_exif_gps_and_datetime() {
    let fields = read_exif(&fixture("exif.tif")).unwrap();
    assert!(fields.datetime.is_some());
    assert!((fields.latitude.unwrap() - 46.5).abs() < 1e-6);
    assert!((fields.longitude.unwrap() - 7.5).abs() < 1e-6);
}

#[test]
fn no_gps_yields_no_location() {
    let fields = read_exif(&fixture("no_gps.jpg")).unwrap();
    assert!(fields.datetime.is_some());
    assert!(fields.latitude.is_none());
    assert!(fields.longitude.is_none());
}

#[test]
fn no_exif_yields_empty_fields() {
    let fields = read_exif(&fixture("no_exif.jpg")).unwrap();
    assert!(fields.datetime.is_none());
    assert!(fields.latitude.is_none());
    assert!(fields.longitude.is_none());
}

// -- config --
#[test]
fn config_requires_existing_watch_dir() {
    let config = serde_json::json!({ "watch_dir": "/definitely/not/here/xyz" });
    assert!(PhotosConnector::from_config(config).is_err());
}

#[test]
fn config_loads_seeded_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let mut cursor = PhotosCursor::default();
    cursor.upsert(
        "seen.jpg".to_string(),
        FileSig {
            inode: 9,
            mtime_ms: 5,
            size: 5,
        },
    );
    let cursor_json = cursor.to_json();
    let config = serde_json::json!({
        "watch_dir": dir.path().to_string_lossy(),
        "__cursor": cursor_json,
    });
    let connector = PhotosConnector::from_config(config).unwrap();
    assert_eq!(connector.cursor.try_lock().unwrap().len(), 1);
}

#[test]
fn config_uses_slug_when_owner_absent() {
    let dir = tempfile::tempdir().unwrap();
    let config = serde_json::json!({
        "watch_dir": dir.path().to_string_lossy(),
        "__slug": "my-photos",
    });
    let connector = PhotosConnector::from_config(config).unwrap();
    assert_eq!(connector.owner_name, "my-photos");
    assert_eq!(connector.id(), "my-photos");
}

// -- signature --
#[test]
fn file_signature_reads_inode_mtime_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.jpg");
    fs::write(&path, b"hello").unwrap();
    let sig = file_signature(&path).unwrap();
    assert_eq!(sig.size, 5);
    assert!(sig.mtime_ms > 0);
    // On Unix the inode is non-zero; elsewhere it is 0.
    #[cfg(unix)]
    assert_ne!(sig.inode, 0);
}

#[test]
fn stage_file_falls_back_to_mtime_without_exif() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bare.jpg");
    fs::write(&path, b"not really a jpeg").unwrap();
    let sig = file_signature(&path).expect("signature");
    let raw = stage_file(&path, "bare.jpg", sig).unwrap();
    assert!(raw.latitude.is_none());
    assert!(raw.longitude.is_none());
    // taken_at falls back to the file mtime carried in the signature.
    let expected = DateTime::<Utc>::from_timestamp_millis(sig.mtime_ms).unwrap();
    assert_eq!(raw.taken_at, expected);
}
