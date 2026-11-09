"""Find Adjacency / EffectSource defaults and Tavern distance source."""
import re
from pathlib import Path

xml = Path(__file__).resolve().parent.parent.joinpath("_gamedata", "assets.xml").read_text(
    encoding="utf-8", errors="replace"
)

# Context around <Adjacency>
for m in re.finditer(r".{200}<Adjacency>.{0,800}", xml, re.S):
    snip = re.sub(r"\s+", " ", m.group(0))
    print("ADJ:", snip[:500])
    print("---")

# PublicService buildings: do they get distance from ProvidedNeed or another node?
by = {}
for m in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
    c = m.group(1)
    g = re.search(r"<GUID>(\d+)</GUID>", c)
    if g and int(g.group(1)) not in by:
        by[int(g.group(1))] = c

# Market has EffectSource; Tavern doesn't — compare full Values tags present
for guid in (3527, 3528, 3618, 3615):
    c = by[guid]
    name = re.search(r"<Name>([^<]+)</Name>", c).group(1)
    tags = sorted(set(re.findall(r"<([A-Za-z0-9]+)[\s>]", c)))
    es = re.search(r"<EffectSource>(.*?)</EffectSource>", c, re.S)
    print(guid, name)
    print("  EffectSource", re.sub(r"\s+", " ", es.group(0)) if es else None)
    print("  has PublicService", "<PublicService>" in c)
    pse = re.search(r"<PublicServiceEffect>(\d+)</PublicServiceEffect>", c)
    print("  PSE", pse.group(1) if pse else None)

# Maybe Need assets have range?
for need_guid in (2751,):  # tavern ProvidedNeed
    c = by.get(need_guid)
    if not c:
        print("need", need_guid, "missing")
        continue
    name = re.search(r"<Name>([^<]+)</Name>", c)
    print("NEED", need_guid, name.group(1) if name else "?")
    print(re.sub(r"\s+", " ", c)[:1500])

# Heuristic: median RadiusDistance among production with EffectSource
rds = []
sds = []
for c in by.values():
    nm = re.search(r"<Name>([^<]+)</Name>", c)
    if not nm or not nm.group(1).startswith("Production "):
        continue
    es = re.search(r"<EffectSource>(.*?)</EffectSource>", c, re.S)
    if not es:
        continue
    rd = re.search(r"<RadiusDistance>([^<]+)</RadiusDistance>", es.group(1))
    sd = re.search(r"<StreetDistance>([^<]+)</StreetDistance>", es.group(1))
    if rd:
        rds.append(float(rd.group(1)))
    if sd:
        sds.append(float(sd.group(1)))
rds.sort()
sds.sort()
print("prod RadiusDistance count", len(rds), "median", rds[len(rds) // 2] if rds else None, "min", rds[0] if rds else None, "max", rds[-1] if rds else None)
print("prod StreetDistance count", len(sds), "median", sds[len(sds) // 2] if sds else None)

# Empty EffectSource self-closing — check if VectorElement InheritedIndex pattern nearby in same Values
c = by[3190]
print("\nSandals EffectSource context:")
idx = c.find("EffectSource")
print(repr(c[idx - 100 : idx + 200]))

