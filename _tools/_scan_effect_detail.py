"""Dump Effect + Buff structure for coverage modeling."""
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


def show(guid: int):
    chunk = by_guid.get(guid)
    if not chunk:
        print(guid, "MISSING")
        return
    name = re.search(r"<Name>([^<]+)</Name>", chunk)
    print("=" * 60, guid, name.group(1) if name else "?")
    # Effect block
    eff = re.search(r"<Effect>(.*?)</Effect>", chunk, re.S)
    if eff:
        print("Effect:", re.sub(r"\s+", " ", eff.group(1))[:800])
    # Buff values / Attribute
    for tag in ("Buff", "Values", "ItemEffect", "AttributeProvider"):
        pass
    # attrs anywhere
    attrs = re.findall(
        r"<(FireSafety|Health|Happiness|Workforce|Income|Faith|Knowledge|"
        r"Prestige|Attractiveness|ResidenceProsperity|Population)>\s*"
        r"(?:<Value>)?([^<\n]+)(?:</Value>)?",
        chunk,
        re.S,
    )
    print("attrs raw", attrs[:15])
    # EffectScope / distances
    print(
        "scope",
        re.findall(
            r"<(EffectScope|RadiusDistance|StreetDistance|SourceCategory)>([^<]+)</\1>",
            chunk,
        ),
    )
    buffs = re.findall(r"<Buffs>.*?<GUID>(\d+)</GUID>", chunk, re.S)
    targets = re.findall(r"<Targets>.*?<GUID>(\d+)</GUID>", chunk, re.S)
    print("buffs", buffs, "targets", targets[:5])


for g in (98994, 32321, 43097, 99315, 99026, 99025, 99027, 3177):
    show(g)

# How many Effects have StreetDistance scope + linked from production/public
print("\n=== EffectScope distribution for Effects referenced by buildings ===")
scopes = {}
for m in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
    chunk = m.group(1)
    if "<Template>Effect</Template>" not in chunk and ">Effect<" not in chunk[:200]:
        if "<Template>Effect</Template>" not in chunk:
            continue
    scope = re.search(r"<EffectScope>([^<]+)</EffectScope>", chunk)
    if scope:
        scopes[scope.group(1)] = scopes.get(scope.group(1), 0) + 1
print(scopes)

# Default street distance when building has FunctionalEffect but no EffectSource?
# Check Need/PublicService global defaults
for key in ("DefaultStreetDistance", "DefaultRadiusDistance", "PublicServiceRange"):
    hits = list(re.finditer(rf"<{key}>([^<]+)</{key}>", xml))
    print(key, [h.group(1) for h in hits[:10]], "count", len(hits))

