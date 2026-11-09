"""Find radial menu / build tool defaults in assets.xml."""
import re
from pathlib import Path

xml = Path(__file__).resolve().parent.parent.joinpath("_gamedata", "assets.xml").read_text(
    encoding="utf-8", errors="replace"
)

# Dump RadialMenuMaxEntriesConfig
m = re.search(r"<RadialMenuMaxEntriesConfig>(.*?)</RadialMenuMaxEntriesConfig>", xml, re.S)
print("=== RadialMenuMaxEntriesConfig ===")
print(re.sub(r"\s+", " ", m.group(1)) if m else None)

# Find assets mentioning RadialMenu with Name
print("\n=== Assets with Radial in Name ===")
for chunk in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
    c = chunk.group(1)
    n = re.search(r"<Name>([^<]*Radial[^<]*)</Name>", c, re.I)
    if not n:
        continue
    g = re.search(r"<GUID>(\d+)</GUID>", c)
    t = re.search(r"<Template>([^<]+)</Template>", c)
    print(f"{g.group(1) if g else '?'} {n.group(1)} tmpl={t.group(1) if t else None}")

print("\n=== Build tools / Demolish / Copy / Upgrade names ===")
keys = (
    "Demolish",
    "Copy Tool",
    "CopyTool",
    "Relocate",
    "Upgrade Tool",
    "Build Tool",
    "Street Tool",
    "Road Tool",
    "Pipette",
    "Cosmetic",
    "Paint Brush",
    "Quick Selection",
    "Radial Default",
    "Construction Tool",
)
for chunk in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
    c = chunk.group(1)
    n = re.search(r"<Name>([^<]+)</Name>", c)
    if not n:
        continue
    name = n.group(1)
    if not any(k.lower() in name.lower() for k in keys):
        continue
    if any(x in name for x in ("Buff", "Effect ", "Audio", "Incident", "Icon Text")):
        continue
    g = re.search(r"<GUID>(\d+)</GUID>", c)
    t = re.search(r"<Template>([^<]+)</Template>", c)
    # icon?
    icon = re.search(r"<IconFilename>([^<]+)</IconFilename>", c)
    print(
        f"{g.group(1) if g else '?':>8} {name[:60]:60} "
        f"tmpl={t.group(1) if t else None} icon={icon.group(1) if icon else None}"
    )

