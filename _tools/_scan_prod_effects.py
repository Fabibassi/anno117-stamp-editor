"""Find how production buildings expose area bonuses."""
import re
from pathlib import Path

xml = Path(__file__).resolve().parent.parent.joinpath("_gamedata", "assets.xml").read_text(
    encoding="utf-8", errors="replace"
)
pat = re.compile(r"<Asset>(.*?)</Asset>", re.S)
keys = ("Sandal", "Garum", "Bread", "Bakery", "Porridge", "Soap", "Toga", "Wine")
for m in pat.finditer(xml):
    chunk = m.group(1)
    nm = re.search(r"<Name>([^<]+)</Name>", chunk)
    if not nm:
        continue
    name = nm.group(1)
    if not name.startswith("Production "):
        continue
    if not any(k.lower() in name.lower() for k in keys):
        continue
    if any(x in name for x in ("Buff", "Effect ", "Icon", "Module", "Audio")):
        continue
    guid = re.search(r"<GUID>(\d+)</GUID>", chunk)
    tmpl = re.search(r"<Template>([^<]+)</Template>", chunk)
    es = re.search(r"<EffectSource>(.*?)</EffectSource>", chunk, re.S)
    rd = sd = None
    if es:
        rdm = re.search(r"<RadiusDistance>([^<]+)</RadiusDistance>", es.group(1))
        sdm = re.search(r"<StreetDistance>([^<]+)</StreetDistance>", es.group(1))
        rd = float(rdm.group(1)) if rdm else None
        sd = float(sdm.group(1)) if sdm else None
    # ItemEffect / FactoryArea / ResidenceBonus etc.
    interesting = []
    for tag in (
        "ItemEffect",
        "FactoryAreaEffect",
        "AreaEffect",
        "ResidenceUpgrade",
        "PublicService",
        "AttributeProvider",
        "EffectTarget",
        "Distance",
        "Radius",
        "Street",
        "Buff",
        "Influence",
    ):
        if f"<{tag}" in chunk:
            interesting.append(tag)
    # Pull a slice around AttributeProvider if any
    ap = re.search(r"<AttributeProvider>(.*?)</AttributeProvider>", chunk, re.S)
    ap_snip = ap.group(1).strip()[:200] if ap else None
    # Distance fields anywhere
    dists = re.findall(
        r"<(RadiusDistance|StreetDistance|InfluenceRadius|MaxDistance|Range)>([^<]+)</\1>",
        chunk,
    )
    print(f"{guid.group(1) if guid else '?'} {name}")
    print(f"  tmpl={tmpl.group(1) if tmpl else None} EffectSource R={rd} S={sd}")
    print(f"  tags={interesting} dists={dists}")
    if ap_snip:
        print(f"  attr={ap_snip!r}")
    print()

