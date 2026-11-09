import re
import struct
import zlib
from pathlib import Path

RDA = Path(
    r"G:\Program Files (x86)\Steam\steamapps\common\Anno 117 - Pax Romana\maindata\shared_configs.rda"
)
data = RDA.read_bytes()
first = struct.unpack_from("<Q", data, 784)[0]
ENTRY = 560
b = first
seen: set[int] = set()
want = ("craft_chassis.cfg", "culture_chariots.cfg")
while b and b not in seen and b + 32 <= len(data):
    seen.add(b)
    _flags, count = struct.unpack_from("<II", data, b)
    csize, usize, nxt = struct.unpack_from("<QQQ", data, b + 8)
    if count > 0 and 0 < csize and b >= csize:
        raw = data[b - csize : b]
        if csize != usize:
            try:
                raw = zlib.decompress(raw)
            except Exception:
                b = nxt
                continue
        count = min(count, len(raw) // ENTRY)
        for i in range(count):
            e = raw[i * ENTRY : (i + 1) * ENTRY]
            path = e[0:520].decode("utf-16le", "replace").split("\x00", 1)[0].replace("\\", "/")
            pl = path.lower()
            if not any(w in pl for w in want) or not pl.endswith(".cfg"):
                continue
            ptr, cs, _us, _mt, _res = struct.unpack_from("<QQQQQ", e, 520)
            blob = data[ptr : ptr + cs]
            if blob[:2] in (b"\x78\x9c", b"\x78\xda", b"\x78\x01"):
                blob = zlib.decompress(blob)
            text = blob.decode("utf-8", "replace")
            print("====", path, "chars", len(text))
            for tag in ("BuildBlocker", "DummyBuildBlocker", "Blocking", "Mesh", "Config"):
                print(" tag", tag, text.count(tag))
            bb = re.search(r"<BuildBlocker>(.*?)</BuildBlocker>", text, re.S)
            if bb:
                print(bb.group(0)[:1000])
                xs = [float(x) for x in re.findall(r"<xf>([^<]+)</xf>", bb.group(1))]
                zs = [float(z) for z in re.findall(r"<zf>([^<]+)</zf>", bb.group(1))]
                print("xs", xs, "zs", zs)
                if xs and zs:
                    print("raw dw/dh", max(xs) - min(xs), max(zs) - min(zs))
            lines = re.findall(r"<Line>([01]+)</Line>", text)
            if lines:
                print("bitmap", len(lines), "x", len(lines[0]))
                for line in lines:
                    print(" ", line)
            aabbs = []
            for am in re.finditer(
                r"<AABBMin><x>([^<]+)</x>.*?<z>([^<]+)</z>.*?<AABBMax><x>([^<]+)</x>.*?<z>([^<]+)</z>",
                text,
                re.S,
            ):
                x0, z0, x1, z1 = map(float, am.groups())
                aabbs.append((abs(x1 - x0), abs(z1 - z0)))
            print("AABBs", sorted(aabbs, reverse=True)[:8])
            # show any float pairs near 'blocker' / 'extent'
            print(text[:400].replace("\n", " | "))
    if nxt == 0 or nxt >= len(data):
        break
    b = nxt

