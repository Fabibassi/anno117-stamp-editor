"""Trace FunctionalEffect / BaseAssetGUID for Institution Roman Fire."""
import re
from pathlib import Path

xml = Path(__file__).resolve().parent.parent.joinpath("_gamedata", "assets.xml").read_text(
    encoding="utf-8", errors="replace"
)


def assets_with(substr: str):
    out = []
    for m in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
        chunk = m.group(1)
        if substr not in chunk:
            continue
        name = re.search(r"<Name>([^<]+)</Name>", chunk)
        guid = re.search(r"<GUID>(\d+)</GUID>", chunk)
        tmpl = re.search(r"<Template>([^<]+)</Template>", chunk)
        base = re.search(r"<BaseAssetGUID>(\d+)</BaseAssetGUID>", chunk)
        out.append(
            (
                int(guid.group(1)) if guid else None,
                name.group(1) if name else "?",
                tmpl.group(1) if tmpl else None,
                int(base.group(1)) if base else None,
                chunk,
            )
        )
    return out


print("=== assets mentioning Institution Roman Fire name ===")
for guid, name, tmpl, base, chunk in assets_with("Institution Roman Fire"):
    if "Buff" in name or "Icon" in name or "Image" in name or "Audio" in name:
        continue
    es = re.search(r"<EffectSource>(.*?)</EffectSource>", chunk, re.S)
    rd = sd = None
    if es:
        rdm = re.search(r"<RadiusDistance>([^<]+)</RadiusDistance>", es.group(1))
        sdm = re.search(r"<StreetDistance>([^<]+)</StreetDistance>", es.group(1))
        rd = rdm.group(1) if rdm else None
        sd = sdm.group(1) if sdm else None
    funcs = re.findall(r"<FunctionalEffect>(\d+)</FunctionalEffect>", chunk)
    print(f"  {guid} {name} tmpl={tmpl} base={base} R={rd} S={sd} funcs={funcs}")

print("\n=== FunctionalEffect asset 98994 ===")
for guid, name, tmpl, base, chunk in assets_with("<GUID>98994</GUID>"):
    if guid != 98994:
        continue
    print(name, tmpl, base)
    # print condensed
    body = re.sub(r"\s+", " ", chunk)
    print(body[:2500])

print("\n=== any EffectSource near FireSafety positive ===")
count = 0
for m in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
    chunk = m.group(1)
    if "<EffectSource>" not in chunk:
        continue
    if "FireSafety" not in chunk and "Health" not in chunk and "Happiness" not in chunk:
        continue
    name = re.search(r"<Name>([^<]+)</Name>", chunk)
    guid = re.search(r"<GUID>(\d+)</GUID>", chunk)
    if not name:
        continue
    n = name.group(1)
    if any(x in n for x in ("Buff", "Effect ", "Icon", "Incident", "Audio")):
        continue
    attrs = re.findall(
        r"<(FireSafety|Health|Happiness)>\s*<Value>([^<]+)</Value>\s*</\1>",
        chunk,
        re.S,
    )
    if not attrs:
        # try flat
        attrs = re.findall(r"<(FireSafety|Health|Happiness)>([^<]+)</\1>", chunk)
    es = re.search(r"<EffectSource>(.*?)</EffectSource>", chunk, re.S)
    rd = re.search(r"<RadiusDistance>([^<]+)</RadiusDistance>", es.group(1)) if es else None
    sd = re.search(r"<StreetDistance>([^<]+)</StreetDistance>", es.group(1)) if es else None
    if not attrs:
        continue
    print(
        f"{guid.group(1)} {n[:50]} R={rd.group(1) if rd else None} S={sd.group(1) if sd else None} {attrs[:6]}"
    )
    count += 1
    if count >= 40:
        break
print("count", count)

