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
    info = size_and_min_from_ifo(text)
    return None if info is None else info[0]


def size_and_min_from_ifo(text: str) -> tuple[list[float], list[float]] | None:
    """Return (size [w,h], blocker_min [xf,zf]) from the first BuildBlocker rect.

    Pipette `Pos` is the mesh pivot, not necessarily the footprint center. Decentered
    blockers (e.g. aqueduct cistern ~[-0.5,2.5]×[-1.5,1.5]) must keep `blocker_min`
    so the editor writes the correct Pos for a chosen top-left tile.
    """
    m = re.search(r"<BuildBlocker>(.*?)</BuildBlocker>", text, re.S)
    if not m:
        return None
    xs = [float(x) for x in re.findall(r"<xf>([^<]+)</xf>", m.group(1))]
    zs = [float(z) for z in re.findall(r"<zf>([^<]+)</zf>", m.group(1))]
    if len(xs) < 2 or len(zs) < 2:
        return None
    # Prefer the *larger* rectangle when a second DummyBuildBlocker exists
    # (aqueduct cisterns may ship a small connector rect first).
    if len(xs) >= 8 and len(zs) >= 8:
        xs0, zs0 = xs[:4], zs[:4]
        xs1, zs1 = xs[4:8], zs[4:8]
        d0 = (max(xs0) - min(xs0)) ** 2 + (max(zs0) - min(zs0)) ** 2
        d1 = (max(xs1) - min(xs1)) ** 2 + (max(zs1) - min(zs1)) ** 2
        if d1 > d0 + 0.01:
            xs, zs = xs1, zs1
        else:
            xs, zs = xs0, zs0
    tw = max(1.0, float(round(max(xs) - min(xs))))
    th = max(1.0, float(round(max(zs) - min(zs))))
    if tw > 64 or th > 64:
        return None
    return [tw, th], [float(min(xs)), float(min(zs))]


MINS_OUT = OUT.with_name("cfg_blocker_mins.json")


def main() -> None:
    sizes: dict[str, list[float]] = {}
    mins: dict[str, list[float]] = {}
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
                    info = size_and_min_from_ifo(text)
                    if not info:
                        continue
                    sz, bmin = info
                    key = normalize_key(fpath)
                    # Map .ifo → .cfg key so catalog Filename matching still works.
                    cfg_key = re.sub(r"\.ifo$", ".cfg", key)
                    for k in (cfg_key, "#" + Path(cfg_key).name, key, "#" + Path(key).name):
                        sizes[k] = sz
                        # Only store mins that differ from a centered blocker (ε = 0.05).
                        if abs(bmin[0] + sz[0] * 0.5) > 0.05 or abs(bmin[1] + sz[1] * 0.5) > 0.05:
                            mins[k] = [round(bmin[0], 4), round(bmin[1], 4)]
                    n += 1
            finally:
                mm.close()
        print(f"  ifo sizes from {name}: {n}")

    OUT.write_text(json.dumps(sizes, indent=0), encoding="utf-8")
    MINS_OUT.write_text(json.dumps(mins, indent=0), encoding="utf-8")
    print(f"wrote {OUT} entries={len(sizes)} scanned_ifo={scanned}")
    print(f"wrote {MINS_OUT} decentered entries={len(mins)}")
    for probe in (
        "#craft_chassis.cfg",
        "#culture_chariots.cfg",
        "#construction_granite.cfg",
        "#residence_tier01_estate01.cfg",
        "#institution_01.cfg",
    ):
        print(probe, sizes.get(probe), "min", mins.get(probe))


if __name__ == "__main__":
    main()

