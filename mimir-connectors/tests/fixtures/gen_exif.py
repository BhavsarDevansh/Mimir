#!/usr/bin/env python3
# Generates tiny EXIF-bearing test fixtures for the Photos connector (C1 / #195).
#
# Pure-stdlib (no PIL/piexif) so the fixtures can be regenerated anywhere. Each
# fixture is a minimal, structurally-valid container carrying a known EXIF
# payload (DateTimeOriginal + GPS latitude/longitude) that kamadak-exif parses
# via `Reader::read_from_container`. The images are 1x1 placeholder bodies; only
# the EXIF metadata matters for the connector tests.
#
# Known values (asserted by the Rust tests):
#   DateTimeOriginal = "2024:05:15 14:30:00"
#   GPS              = 46 deg 30 min 0.000 sec N, 7 deg 30 min 0.000 sec E
#                      -> 46.5 N, 7.5 E
import struct, sys, os

# TIFF/EXIF type codes
ASCII, SHORT, LONG, RATIONAL = 2, 3, 4, 5

def rational(num, den=1):
    return struct.pack("<II", num, den)

def build_tiff(exif_dt: str | None, gps: tuple | None) -> bytes:
    """Build a little-endian Exif TIFF stream (the APP1 payload body for JPEG,
    or the whole file for TIFF)."""
    # Layout plan (offsets are from the start of this TIFF stream):
    #   0..8  : header ("II", 0x002A, ifd0_offset=8)
    #   8..   : IFD0 (count + entries + next=0)
    #   then  : Exif IFD, GPS IFD, data blobs
    entries = []  # (tag, type, count, value_or_offset, extra_bytes)
    # We'll fill sub-IFD offsets after laying out IFD0.
    # First, decide which IFD0 entries exist.
    ifd0_entries = []
    if exif_dt is not None:
        ifd0_entries.append(("exif", 0x8769, LONG, 1, None))  # ExifIFD pointer
    if gps is not None:
        ifd0_entries.append(("gps", 0x8825, LONG, 1, None))  # GPSInfo pointer

    def ifd_size(n):
        return 2 + n * 12 + 4

    ifd0_offset = 8
    ifd0_end = ifd0_offset + ifd_size(len(ifd0_entries))

    # Sub-IFDs follow IFD0.
    cur = ifd0_end
    sub_offsets = {}
    sub_layout = []  # (key, entries-spec)
    # Build Exif IFD entries
    exif_entries_spec = []
    if exif_dt is not None:
        exif_entries_spec.append(("dt", 0x9003, ASCII, len(exif_dt) + 1, exif_dt + "\x00"))
    gps_entries_spec = []
    if gps is not None:
        lat_deg, lat_min, lat_sec, lat_ref, lon_deg, lon_min, lon_sec, lon_ref = gps
        gps_entries_spec.append(("latref", 0x0001, ASCII, 2, lat_ref + "\x00"))
        gps_entries_spec.append(("lat", 0x0002, RATIONAL, 3,
                                 rational(lat_deg) + rational(lat_min) + rational(int(lat_sec * 10000), 10000)))
        gps_entries_spec.append(("lonref", 0x0003, ASCII, 2, lon_ref + "\x00"))
        gps_entries_spec.append(("lon", 0x0004, RATIONAL, 3,
                                 rational(lon_deg) + rational(lon_min) + rational(int(lon_sec * 10000), 10000)))

    sub_starts = {}
    if exif_dt is not None:
        sub_starts["exif"] = cur
        cur += ifd_size(len(exif_entries_spec))
    if gps is not None:
        sub_starts["gps"] = cur
        cur += ifd_size(len(gps_entries_spec))
    data_start = cur

    # Now lay out data blobs and resolve offsets.
    data_blobs = []  # (offset, bytes)
    def alloc(blob: bytes) -> int:
        nonlocal cur
        off = cur
        data_blobs.append((off, blob))
        cur += len(blob)
        return off

    # Build each IFD's packed entries with resolved offsets.
    def pack_ifd(offset, spec_entries):
        out = bytearray()
        out += struct.pack("<H", len(spec_entries))
        for key, tag, typ, count, payload in spec_entries:
            if typ == ASCII and count > 4:
                val = alloc(payload.encode("ascii"))
            elif typ == RATIONAL:
                val = alloc(payload)
            elif typ == ASCII:  # count <= 4, inline
                b = payload.encode("ascii")
                b = b + b"\x00" * (4 - len(b))
                val = struct.unpack("<I", b)[0]
            elif typ == LONG:
                val = payload if isinstance(payload, int) else 0
            else:
                val = payload if isinstance(payload, int) else 0
            out += struct.pack("<HHII", tag, typ, count, val)
        out += struct.pack("<I", 0)  # next IFD = 0
        assert len(out) == ifd_size(len(spec_entries)), (len(out), ifd_size(len(spec_entries)))
        return bytes(out)

    # Exif IFD spec uses resolved offsets for dt (alloc happens inside pack_ifd).
    # But sub-IFD offsets referenced by IFD0 must be set BEFORE packing IFD0,
    # and data offsets are resolved during pack_ifd. To keep ordering simple,
    # pack sub-IFDs first (allocating their data), then pack IFD0 referencing
    # the now-known sub-IFD starts.
    sub_packed = {}
    if exif_dt is not None:
        sub_packed["exif"] = pack_ifd(sub_starts["exif"], exif_entries_spec)
    if gps is not None:
        sub_packed["gps"] = pack_ifd(sub_starts["gps"], gps_entries_spec)

    # IFD0 entries: ExifIFD/GPS pointers use sub_starts offsets.
    ifd0_spec = []
    for key, tag, typ, count, _ in ifd0_entries:
        ifd0_spec.append((key, tag, typ, count, sub_starts[key]))
    ifd0_packed = pack_ifd(ifd0_offset, ifd0_spec)

    # Assemble
    out = bytearray()
    out += b"II"
    out += struct.pack("<H", 0x002A)
    out += struct.pack("<I", ifd0_offset)
    out += ifd0_packed
    if exif_dt is not None:
        # The sub-IFD was packed at sub_starts["exif"]; ensure contiguous layout.
        out += sub_packed["exif"]
    if gps is not None:
        out += sub_packed["gps"]
    for off, blob in data_blobs:
        # Pad to reach offset if needed (should already be contiguous).
        if len(out) < off:
            out += b"\x00" * (off - len(out))
        out += blob
    return bytes(out)

def write_jpeg(path, tiff_body: bytes):
    # SOI + APP1(Exif) + EOI
    app1 = b"Exif\x00\x00" + tiff_body
    seg = struct.pack(">H", 2 + len(app1))
    with open(path, "wb") as f:
        f.write(b"\xff\xd8")          # SOI
        f.write(b"\xff\xe1")          # APP1 marker
        f.write(seg)
        f.write(app1)
        f.write(b"\xff\xd9")          # EOI

def write_tiff(path, tiff_body: bytes):
    with open(path, "wb") as f:
        f.write(tiff_body)

def main():
    here = os.path.dirname(os.path.abspath(__file__))
    # Full EXIF: datetime + GPS
    gps = (46, 30, 0.0, "N", 7, 30, 0.0, "E")
    body = build_tiff("2024:05:15 14:30:00", gps)
    write_jpeg(os.path.join(here, "exif.jpg"), body)
    write_tiff(os.path.join(here, "exif.tif"), body)
    # DateTime only, no GPS
    body_nogps = build_tiff("2024:05:15 14:30:00", None)
    write_jpeg(os.path.join(here, "no_gps.jpg"), body_nogps)
    # No EXIF at all: bare JPEG
    with open(os.path.join(here, "no_exif.jpg"), "wb") as f:
        f.write(b"\xff\xd8\xff\xd9")
    print("fixtures written:", sorted(os.listdir(here)))

if __name__ == "__main__":
    main()
