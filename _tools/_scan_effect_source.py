"""One-shot: sample EffectSource Street/Radius distances from assets.xml."""
import re
from pathlib import Path

xml = Path(__file__).resolve().parent.parent.joinpath("_gamedata", "assets.xml").read_text(
    encoding="utf-8", errors="replace"
)
pat = re.compile(r"<Asset>(.*?)</Asset>", re.S)
want = re.compile(
    r"(Institution|Public |Production Roman (Sandal|Garum|Bread|Bakery|Porridge)|"
    r"Market|Tavern|Vigiles|Medici|Custod|CityInstitution)",
    re.I,
)
samples = []
for m in pat.finditer(xml):
    chunk = m.group(1)
    nm = re.search(r"<Name>([^<]+)</Name>", chunk)
    if not nm:
        continue
    name = nm.group(1)
    if not want.search(name):
        continue
    if any(x in name for x in ("Buff", "Effect ", "Icon", "Image ", "Audio", "Incident")):
        continue
    es = re.search(r"<EffectSource>(.*?)</EffectSource>", chunk, re.S)
    if not es:
        continue
    rd = re.search(r"<RadiusDistance>([^<]+)</RadiusDistance>", es.group(1))
    sd = re.search(r"<StreetDistance>([^<]+)</StreetDistance>", es.group(1))
    if not (rd or sd):
        continue
    tmpl = re.search(r"<Template>([^<]+)</Template>", chunk)
    guid = re.search(r"<GUID>(\d+)</GUID>", chunk)
    attrs = re.findall(
        r"<(FireSafety|Health|Happiness|Workforce|Population|Income|Faith|Knowledge|Prestige|"
        r"ResidenceProsperity|Attractiveness)>([^<]+)</\1>",
        chunk,
    )
    infl = re.search(r"<InfluenceRadius>([^<]+)</InfluenceRadius>", chunk)
    samples.append(
        {
            "guid": int(guid.group(1)) if guid else None,
            "name": name,
            "template": tmpl.group(1) if tmpl else None,
            "radius_distance": float(rd.group(1)) if rd else None,
            "street_distance": float(sd.group(1)) if sd else None,
            "influence_radius": float(infl.group(1)) if infl else None,
            "attrs": attrs[:10],
        }
    )

# Prefer constructable-looking / shorter names first
samples.sort(key=lambda s: (s["name"], s["guid"] or 0))
for s in samples[:80]:
    print(
        f"{s['guid']!s:>8} {s['name'][:55]:55} "
        f"R={s['radius_distance']!s:>5} S={s['street_distance']!s:>5} "
        f"I={s['influence_radius']!s:>4} attrs={s['attrs']}"
    )
print("total", len(samples))

# CityInstitutions distance modifiers
mod = re.search(r"<CityInstitutions>\s*<DistanceModifiers>(.*?)</DistanceModifiers>", xml, re.S)
if mod:
    print("\n--- CityInstitutions DistanceModifiers ---")
    print(mod.group(1)[:800])

