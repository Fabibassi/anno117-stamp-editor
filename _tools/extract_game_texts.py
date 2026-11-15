"""Extract Anno 117 GUI texts (EN/DE) from config.rda into _gamedata JSON.

Uses the linked game install from stamp_editor_config.json (or --game).
Output is local-only (copyright) — same policy as other _gamedata files.
"""
from __future__ import annotations

import argparse
import json
import re
import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT_DIR = ROOT / "_gamedata"
CONFIG_JSON = ROOT / "stamp_editor_config.json"
ENTRY = 560


def normalize_key(path: str) -> str:
    return path.replace("\\", "/").lstrip("/").lower()


def read_rda_file(rda_path: Path, key: str) -> bytes:
    key = normalize_key(key)
    with rda_path.open("rb") as f:
        f.seek(784)
        block = struct.unpack_from("<Q", f.read(8))[0]
        file_len = rda_path.stat().st_size
        seen: set[int] = set()
        while block and block not in seen and block + 32 <= file_len:
            seen.add(block)
            f.seek(block)
            hdr = f.read(32)
            count, csize, usize, nxt = struct.unpack_from("<IQQQ", hdr, 4)
            if count and csize and block >= csize:
                f.seek(block - csize)
                raw = f.read(csize)
                if csize != usize:
                    try:
                        raw = zlib.decompress(raw)
                    except zlib.error:
                        pass
                n = min(count, len(raw) // ENTRY)
                for i in range(n):
                    e = raw[i * ENTRY : (i + 1) * ENTRY]
                    p = (
                        e[0:520]
                        .decode("utf-16-le", "ignore")
                        .split("\x00", 1)[0]
                    )
                    if normalize_key(p) != key:
                        continue
                    ptr, cs, us = struct.unpack_from("<QQQ", e, 520)
                    f.seek(ptr)
                    data = f.read(cs)
                    if cs != us:
                        data = zlib.decompress(data)
                    return data
            if not nxt or nxt >= file_len:
                break
            block = nxt
    raise FileNotFoundError(f"{key} not in {rda_path}")


def parse_texts_xml(xml: str) -> dict[str, str]:
    out: dict[str, str] = {}
    for m in re.finditer(
        r"<LineId>(-?\d+)</LineId>\s*<Text>(.*?)</Text>",
        xml,
        re.S,
    ):
        line_id, text = m.group(1), m.group(2)
        # Unescape common XML entities; keep raw otherwise.
        text = (
            text.replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", '"')
            .replace("&apos;", "'")
        )
        out[line_id] = text
    return out


def resolve_game_install(cli: str | None) -> Path:
    if cli:
        return Path(cli)
    if CONFIG_JSON.is_file():
        cfg = json.loads(CONFIG_JSON.read_text(encoding="utf-8"))
        gi = cfg.get("game_install")
        if gi:
            return Path(gi)
    raise SystemExit(
        "No game install — pass --game or set game_install in stamp_editor_config.json"
    )


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--game",
        help="Anno 117 install folder (contains maindata/config.rda)",
    )
    args = ap.parse_args()
    game = resolve_game_install(args.game)
    config_rda = game / "maindata" / "config.rda"
    if not config_rda.is_file():
        raise SystemExit(f"Missing {config_rda}")

    OUT_DIR.mkdir(parents=True, exist_ok=True)
    for lang in ("english", "german"):
        key = f"data/base/config/gui/texts_{lang}.xml"
        xml = read_rda_file(config_rda, key).decode("utf-8", errors="replace")
        table = parse_texts_xml(xml)
        out = OUT_DIR / f"texts_{lang}.json"
        out.write_text(
            json.dumps(table, ensure_ascii=False, separators=(",", ":")),
            encoding="utf-8",
        )
        print(f"wrote {out} entries={len(table)}")

    # Spot-check: Good Leather OasisId
    leather = "-6906930495692423618"
    en = json.loads((OUT_DIR / "texts_english.json").read_text(encoding="utf-8"))
    de = json.loads((OUT_DIR / "texts_german.json").read_text(encoding="utf-8"))
    print(f"spot-check Leather: EN={en.get(leather)!r} DE={de.get(leather)!r}")


if __name__ == "__main__":
    main()
