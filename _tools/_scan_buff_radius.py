"""Find radius values on buffs / templates for effects without EffectSource."""
import re
from pathlib import Path

xml = Path(__file__).resolve().parent.parent.joinpath("_gamedata", "assets.xml").read_text(
    encoding="utf-8", errors="replace"
)
by_guid = {}
for m in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
    chunk = m.group(1)
    g = re.search(r"<GUID>(\d+)</GUID>", chunk)
    if not g:
        continue
    guid = int(g.group(1))
    if guid not in by_guid:
        by_guid[guid] = chunk


def dump(guid):
    chunk = by_guid.get(guid)
    if not chunk:
        print(guid, "MISSING")
        return
    name = re.search(r"<Name>([^<]+)</Name>", chunk)
    print("=" * 60, guid, name.group(1) if name else "?")
    print(re.sub(r"\s+", " ", chunk)[:2000])


for g in (32340, 32332, 32360, 32321, 68484, 68477):
    dump(g)

# Template CityInstitutionBuilding / PublicServiceBuilding defaults?
print("\n=== Template assets ===")
for m in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
    chunk = m.group(1)
    name = re.search(r"<Name>([^<]+)</Name>", chunk)
    if not name:
        continue
    n = name.group(1)
    if n in (
        "CityInstitutionBuilding",
        "PublicServiceBuilding",
        "Production",
        "MiniInstitutionBuilding",
    ):
        print("---", n)
        es = re.search(r"<EffectSource>(.*?)</EffectSource>", chunk, re.S)
        print("EffectSource", re.sub(r"\s+", " ", es.group(0)) if es else None)
        dists = re.findall(
            r"<(RadiusDistance|StreetDistance)>([^<]+)</\1>",
            chunk,
        )
        print("dists", dists)

# Correlation: buildings with FunctionalEffect Radius-scope but no EffectSource —
# do they share a common default radius somewhere in Building node?
print("\n=== Sample Radius-scope buildings without EffectSource distances ===")
count = 0
for guid, chunk in by_guid.items():
    name_m = re.search(r"<Name>([^<]+)</Name>", chunk)
    if not name_m:
        continue
    name = name_m.group(1)
    if not name.startswith("Production "):
        continue
    funcs = re.findall(r"<FunctionalEffect>(\d+)</FunctionalEffect>", chunk)
    if not funcs:
        continue
    es = re.search(r"<EffectSource>(.*?)</EffectSource>", chunk, re.S)
    has_dist = bool(es and ("RadiusDistance" in es.group(1) or "StreetDistance" in es.group(1)))
    # check first effect scope
    ech = by_guid.get(int(funcs[0]))
    if not ech:
        continue
    scope = re.search(r"<EffectScope>([^<]+)</EffectScope>", ech)
    if not scope or scope.group(1) != "Radius":
        continue
    if has_dist:
        continue
    # look for any number that might be radius near Building
    nums = re.findall(
        r"<(MaxTransporterRange|InfluenceRadius|Range|Radius)>([^<]+)</\1>",
        chunk,
    )
    print(f"{guid} {name[:50]} funcs={funcs} nums={nums}")
    count += 1
    if count >= 25:
        break

