"""Extract building footprints from CFG files inside shared_configs.rda."""
from __future__ import annotations

import json
import re
import struct
import zlib
from pathlib import Path

RDA = Path(
    r"G:\Program Files (x86)\Steam\steamapps\common\Anno 117 - Pax Romana\maindata\shared_configs.rda"
)
OUT = Path(__file__).resolve().parent.parent / "_gamedata" / "cfg_build_sizes.json"
ENTRY = 560


def main() -> None:
    data = RDA.read_bytes()
    first = struct.unpack_from("<Q", data, 784)[0]
    blocks: list[tuple[int, int, int, int, int]] = []
    b = first
    seen: set[int] = set()
    while b and b not in seen and b + 32 <= len(data):
        seen.add(b)
        flags, count = struct.unpack_from("<II", data, b)
        csize, usize, nxt = struct.unpack_from("<QQQ", data, b + 8)
        blocks.append((b, count, csize, usize, nxt))
        if nxt == 0 or nxt >= len(data):
            break
        b = nxt

    sizes: dict[str, list[float]] = {}
    for block, count, csize, usize, _nxt in blocks:
        if count <= 0 or csize <= 0 or block < csize:
            continue
        raw = data[block - csize : block]
        if csize != usize:
            try:
                raw = zlib.decompress(raw)
            except zlib.error:
                continue
        count = min(count, len(raw) // ENTRY)
        for i in range(count):
            e = raw[i * ENTRY : (i + 1) * ENTRY]
            path = e[0:520].decode("utf-16le", "replace").split("\x00", 1)[0].replace("\\", "/")
            if not path.lower().endswith(".cfg") or "/buildings/" not in path.lower():
                continue
            ptr, cs, us, _mt, _res = struct.unpack_from("<QQQQQ", e, 520)
            if not (0 < ptr < len(data) and 0 < cs < 20_000_000):
                continue
            try:
                blob = data[ptr : ptr + cs]
                if blob[:2] in (b"\x78\x9c", b"\x78\xda", b"\x78\x01"):
                    blob = zlib.decompress(blob)
                text = blob.decode("utf-8", "replace")
            except Exception:
                continue

            tw = th = None
            m = re.search(r"<BuildBlocker>(.*?)</BuildBlocker>", text, re.S)
            if m:
                xs = [float(x) for x in re.findall(r"<xf>([^<]+)</xf>", m.group(1))]
                zs = [float(z) for z in re.findall(r"<zf>([^<]+)</zf>", m.group(1))]
                if xs and zs:
                    tw = max(1.0, float(round(max(xs) - min(xs))))
                    th = max(1.0, float(round(max(zs) - min(zs))))
            if tw is None:
                lines = re.findall(r"<Line>([01]+)</Line>", text)
                if lines and 1 <= len(lines) <= 24 and 1 <= len(lines[0]) <= 24:
                    tw = float(len(lines[0]))
                    th = float(len(lines))
            if tw is None:
                best = 0.0
                bw = bh = None
                for am in re.finditer(
                    r"<AABBMin><x>([^<]+)</x>.*?<z>([^<]+)</z>.*?<AABBMax><x>([^<]+)</x>.*?<z>([^<]+)</z>",
                    text,
                    re.S,
                ):
                    x0, z0, x1, z1 = map(float, am.groups())
                    w, h = abs(x1 - x0), abs(z1 - z0)
                    if w * h > best and 0.5 < w < 30 and 0.5 < h < 30:
                        best = w * h
                        bw, bh = w, h
                if bw is not None and bh is not None:
                    tw = max(1.0, float(round(bw)))
                    th = max(1.0, float(round(bh)))
            if tw is None:
                continue

            key = path.lower().lstrip("/")
            for pref in ("shared/", "data/"):
                if key.startswith(pref):
                    key = key[len(pref) :]
            sizes[key] = [tw, th]
            sizes["#" + Path(path).name.lower()] = [tw, th]

    OUT.write_text(json.dumps(sizes), encoding="utf-8")
    print(f"wrote {OUT} entries={len(sizes)}")


if __name__ == "__main__":
    main()

