"""Resolve PublicServiceEffect / FunctionalEffect GUIDs to ranges + attrs."""
import re
from pathlib import Path

xml = Path(__file__).resolve().parent.parent.joinpath("_gamedata", "assets.xml").read_text(
    encoding="utf-8", errors="replace"
)

# Index assets by GUID (first Name wins)
by_guid = {}
for m in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
    chunk = m.group(1)
    g = re.search(r"<GUID>(\d+)</GUID>", chunk)
    if not g:
        continue
    guid = int(g.group(1))
    if guid in by_guid:
        continue
    by_guid[guid] = chunk


def summarize(guid: int):
    chunk = by_guid.get(guid)
    if not chunk:
        return f"{guid}: MISSING"
    name = re.search(r"<Name>([^<]+)</Name>", chunk)
    tmpl = re.search(r"<Template>([^<]+)</Template>", chunk)
    base = re.search(r"<BaseAssetGUID>(\d+)</BaseAssetGUID>", chunk)
    es = re.search(r"<EffectSource>(.*?)</EffectSource>", chunk, re.S)
    rd = sd = None
    if es:
        rdm = re.search(r"<RadiusDistance>([^<]+)</RadiusDistance>", es.group(1))
        sdm = re.search(r"<StreetDistance>([^<]+)</StreetDistance>", es.group(1))
        rd = float(rdm.group(1)) if rdm else None
        sd = float(sdm.group(1)) if sdm else None
    attrs = re.findall(
        r"<(FireSafety|Health|Happiness|Workforce|Income|Faith|Knowledge|"
        r"Prestige|Attractiveness|ResidenceProsperity)>\s*(?:<Value>)?([^<]+)(?:</Value>)?\s*</\1>",
        chunk,
        re.S,
    )
    # flatten
    attrs = [(a, v.strip()) for a, v in attrs if v.strip() and not v.strip().startswith("<")]
    return {
        "guid": guid,
        "name": name.group(1) if name else "?",
        "template": tmpl.group(1) if tmpl else None,
        "base": int(base.group(1)) if base else None,
        "radius_distance": rd,
        "street_distance": sd,
        "attrs": attrs[:12],
    }


for gid in (98994, 99315, 3527, 30754, 3528):
    print(summarize(gid))

print("\n=== Walk bases for 98994 ===")
g = 98994
seen = set()
while g and g not in seen:
    seen.add(g)
    s = summarize(g)
    print(s)
    g = s.get("base") if isinstance(s, dict) else None

print("\n=== Walk bases for 99315 (Tavern effect) ===")
g = 99315
seen = set()
while g and g not in seen:
    seen.add(g)
    s = summarize(g)
    print(s)
    g = s.get("base") if isinstance(s, dict) else None

# Collect all constructable buildings with effect refs
print("\n=== Buildings with PublicServiceEffect or FunctionalEffects ===")
count = 0
rows = []
for guid, chunk in by_guid.items():
    name_m = re.search(r"<Name>([^<]+)</Name>", chunk)
    if not name_m:
        continue
    name = name_m.group(1)
    if not (
        name.startswith("Public ")
        or name.startswith("Institution ")
        or name.startswith("Production ")
    ):
        continue
    if any(x in name for x in ("Buff", "Effect ", "Icon", "Audio", "Chain ")):
        continue
    pse = re.search(r"<PublicServiceEffect>(\d+)</PublicServiceEffect>", chunk)
    funcs = re.findall(r"<FunctionalEffect>(\d+)</FunctionalEffect>", chunk)
    es = re.search(r"<EffectSource>(.*?)</EffectSource>", chunk, re.S)
    rd = sd = None
    if es and "<RadiusDistance>" in es.group(1):
        rdm = re.search(r"<RadiusDistance>([^<]+)</RadiusDistance>", es.group(1))
        sdm = re.search(r"<StreetDistance>([^<]+)</StreetDistance>", es.group(1))
        rd = float(rdm.group(1)) if rdm else None
        sd = float(sdm.group(1)) if sdm else None
    if not pse and not funcs and rd is None and sd is None:
        continue
    effect_ids = []
    if pse:
        effect_ids.append(int(pse.group(1)))
    effect_ids.extend(int(x) for x in funcs)
    # resolve first effect with distances
    er = esd = None
    eattrs = []
    ename = None
    for eid in effect_ids:
        s = summarize(eid)
        if not isinstance(s, dict):
            continue
        # walk base chain for distances
        cur = s
        chain = [cur]
        bg = cur.get("base")
        hops = 0
        while bg and hops < 5:
            hops += 1
            ns = summarize(bg)
            if not isinstance(ns, dict):
                break
            chain.append(ns)
            bg = ns.get("base")
        for c in chain:
            if er is None and c.get("radius_distance") is not None:
                er = c["radius_distance"]
            if esd is None and c.get("street_distance") is not None:
                esd = c["street_distance"]
            eattrs.extend(c.get("attrs") or [])
        ename = s["name"]
        break
    rows.append((name, guid, rd, sd, effect_ids, er, esd, ename, eattrs[:6]))
    count += 1

rows.sort(key=lambda r: r[0])
for r in rows[:70]:
    print(
        f"{r[1]:>8} {r[0][:45]:45} selfR={r[2]} selfS={r[3]} "
        f"eff={r[4]} -> {r[7]} R={r[5]} S={r[6]} attrs={r[8]}"
    )
print("total buildings with effects", len(rows))

