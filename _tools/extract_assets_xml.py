"""Extract data/config/export/assets.xml from config.rda into _gamedata."""
from __future__ import annotations

import mmap
import struct
import zlib
from pathlib import Path

MAINDATA = Path(r"G:\Program Files (x86)\Steam\steamapps\common\Anno 117 - Pax Romana\maindata")
OUT_DIR = Path(__file__).resolve().parent.parent / "_gamedata"
ENTRY = 560


def iter_rda_files(data: mmap.mmap | bytes):
    if len(data) < 800:
        return
    first = struct.unpack_from("<Q", data, 784)[0]
    b = first
    seen: set[int] = set()
    while b and b not in seen and b + 32 <= len(data):
        seen.add(b)
        _flags, count = struct.unpack_from("<II", data, b)
        csize, usize, nxt = struct.unpack_from("<QQQ", data, b + 8)
        if count > 0 and 0 < csize <= b:
            raw = bytes(data[b - csize : b])
            if csize != usize:
                try:
                    raw = zlib.decompress(raw)
                except zlib.error:
                    b = nxt
                    continue
            count = min(count, len(raw) // ENTRY)
            for i in range(count):
                e = raw[i * ENTRY : (i + 1) * ENTRY]
                path = e[0:520].decode("utf-16le", "replace").split("\x00", 1)[0].replace("\\", "/")
                ptr, cs, us, _mt, _res = struct.unpack_from("<QQQQQ", e, 520)
                yield path, ptr, cs, us
        if nxt == 0 or nxt >= len(data):
            break
        b = nxt


def main() -> None:
    rda = MAINDATA / "config.rda"
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    want_suffixes = (
        "assets.xml",
        "assets.xml.checksum",
    )
    found = []
    with rda.open("rb") as fh:
        mm = mmap.mmap(fh.fileno(), 0, access=mmap.ACCESS_READ)
        try:
            for fpath, ptr, cs, us in iter_rda_files(mm):
                low = fpath.replace("\\", "/").lower()
                if not any(low.endswith(s) for s in want_suffixes):
                    continue
                if not (0 < ptr < len(mm) and 0 < cs < 500_000_000):
                    print("bad ptr", fpath, ptr, cs)
                    continue
                blob = bytes(mm[ptr : ptr + cs])
                if blob[:2] in (b"\x78\x9c", b"\x78\xda", b"\x78\x01") or cs != us:
                    try:
                        blob = zlib.decompress(blob)
                    except zlib.error:
                        pass
                name = Path(fpath.replace("\\", "/")).name
                out = OUT_DIR / name
                out.write_bytes(blob)
                found.append((fpath, out, len(blob)))
                print(f"wrote {out} ({len(blob)} bytes) from {fpath}")
        finally:
            mm.close()
    if not found:
        raise SystemExit("assets.xml not found in config.rda")
    print("done", len(found))


if __name__ == "__main__":
    main()
