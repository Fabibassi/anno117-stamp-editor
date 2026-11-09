"""Extract building tile footprints from IFO BuildBlocker (Anno 117).

CFG Mesh-AABB is NOT the build size. Tile size lives in sibling .ifo files:
  BuildBlocker Position xf/zf corners → width/height in tiles (1 unit = 1 tile).
"""
from __future__ import annotations

import json
import mmap
import re
import struct
import zlib
from pathlib import Path

MAINDATA = Path(r"G:\Program Files (x86)\Steam\steamapps\common\Anno 117 - Pax Romana\maindata")
OUT = Path(__file__).resolve().parent.parent / "_gamedata" / "cfg_build_sizes.json"
ENTRY = 560

RDAS = [
    "shared_configs.rda",
    "graphics_celtic.rda",
    "graphics_roman_celtic.rda",
    "graphics_roman.rda",  # mmap — do not read whole file into RAM
    "graphics_library.rda",
]


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


def normalize_key(path: str) -> str:
    key = path.replace("\\", "/").lower().lstrip("/")
    for pref in ("shared/", "data/"):
        if key.startswith(pref):
            key = key[len(pref) :]
    return key


def size_from_ifo(text: str) -> list[float] | None:
    m = re.search(r"<BuildBlocker>(.*?)</BuildBlocker>", text, re.S)
    if not m:
        return None
    xs = [float(x) for x in re.findall(r"<xf>([^<]+)</xf>", m.group(1))]
    zs = [float(z) for z in re.findall(r"<zf>([^<]+)</zf>", m.group(1))]
    if len(xs) < 2 or len(zs) < 2:
        return None
    tw = max(1.0, float(round(max(xs) - min(xs))))
    th = max(1.0, float(round(max(zs) - min(zs))))
    if tw > 64 or th > 64:
        return None
    return [tw, th]


def main() -> None:
    sizes: dict[str, list[float]] = {}
    scanned = 0
    for name in RDAS:
        path = MAINDATA / name
        if not path.exists():
            print(f"skip missing {name}")
            continue
        print(f"scan {name} ...")
        n = 0
        with path.open("rb") as fh:
            mm = mmap.mmap(fh.fileno(), 0, access=mmap.ACCESS_READ)
            try:
                for fpath, ptr, cs, _us in iter_rda_files(mm):
                    if not fpath.lower().endswith(".ifo"):
                        continue
                    if "/buildings/" not in fpath.lower() and "\\buildings\\" not in fpath.lower():
                        continue
                    scanned += 1
                    if not (0 < ptr < len(mm) and 0 < cs < 50_000_000):
                        continue
                    try:
                        blob = bytes(mm[ptr : ptr + cs])
                        if blob[:2] in (b"\x78\x9c", b"\x78\xda", b"\x78\x01"):
                            blob = zlib.decompress(blob)
                        text = blob.decode("utf-8", "replace")
                    except Exception:
                        continue
                    sz = size_from_ifo(text)
                    if not sz:
                        continue
                    key = normalize_key(fpath)
                    # Map .ifo → .cfg key so catalog Filename matching still works.
                    cfg_key = re.sub(r"\.ifo$", ".cfg", key)
                    sizes[cfg_key] = sz
                    sizes["#" + Path(cfg_key).name] = sz
                    sizes[key] = sz
                    sizes["#" + Path(key).name] = sz
                    n += 1
            finally:
                mm.close()
        print(f"  ifo sizes from {name}: {n}")

    OUT.write_text(json.dumps(sizes, indent=0), encoding="utf-8")
    print(f"wrote {OUT} entries={len(sizes)} scanned_ifo={scanned}")
    for probe in (
        "#craft_chassis.cfg",
        "#culture_chariots.cfg",
        "#construction_granite.cfg",
        "#residence_tier01_estate01.cfg",
        "#institution_01.cfg",
    ):
        print(probe, sizes.get(probe))


if __name__ == "__main__":
    main()

