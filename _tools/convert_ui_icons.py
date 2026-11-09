#!/usr/bin/env python3
"""Convert extracted Anno UI DDS icons to PNG for the stamp editor."""

from __future__ import annotations

from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1] / "_gamedata"
ICON_ROOT = ROOT / "ui_icons/data/ui/4k/base/icon_content"
DS_ICONS = ROOT / "ui_icons/data/ui/4k/base/ds/icons"
OUT = ROOT / "ui_png"


def convert_dir(src: Path, dst: Path, pattern: str, size: tuple[int, int] | None) -> int:
    if not src.is_dir():
        print("missing", src)
        return 0
    dst.mkdir(parents=True, exist_ok=True)
    n = 0
    for p in src.glob(pattern):
        try:
            im = Image.open(p).convert("RGBA")
            if size:
                im = im.resize(size, Image.Resampling.LANCZOS)
            name = p.stem[:-2] if p.stem.endswith("_0") else p.stem
            im.save(dst / f"{name}.png")
            n += 1
        except Exception as e:
            print("fail", p.name, e)
    return n


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    counts = {}
    for folder, size in (
        ("production_goods", (96, 96)),
        ("building", (96, 96)),
        ("infrastructure", (96, 96)),
        ("tools", (96, 96)),
        ("generic", (96, 96)),
        ("ornaments", (96, 96)),
        ("need_categories", (96, 96)),
        ("construction_groups", (96, 96)),
        ("construction_tools", (96, 96)),
        ("workforce", (96, 96)),
        ("colosseum", (96, 96)),
        ("portraits", (96, 128)),
    ):
        pattern = "*constructionmenu*.dds" if folder == "portraits" else "*.dds"
        counts[folder] = convert_dir(ICON_ROOT / folder, OUT / folder, pattern, size)
    counts["numeric"] = convert_dir(DS_ICONS, OUT / "numeric", "icon_2d_numeric_*.dds", (32, 32))
    print(f"wrote {counts} -> {OUT}")


if __name__ == "__main__":
    main()

