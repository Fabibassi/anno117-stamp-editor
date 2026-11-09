"""Deep dump Sandals / Tavern / Buff linkage."""
import re
from pathlib import Path

xml = Path(__file__).resolve().parent.parent.joinpath("_gamedata", "assets.xml").read_text(
    encoding="utf-8", errors="replace"
)


def dump_guid(guid: int, label: str):
    for m in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
        chunk = m.group(1)
        if f"<GUID>{guid}</GUID>" not in chunk:
            continue
        name = re.search(r"<Name>([^<]+)</Name>", chunk)
        print("=" * 70)
        print(label, guid, name.group(1) if name else "?")
        # key blocks
        for tag in (
            "EffectSource",
            "AttributeProvider",
            "ItemEffect",
            "BuffList",
            "Buff",
            "FactoryBase",
            "PublicService",
            "Maintenance",
        ):
            mm = re.search(rf"<{tag}\b[^>]*>(.*?)</{tag}>", chunk, re.S)
            if mm:
                body = re.sub(r"\s+", " ", mm.group(0))[:500]
                print(f"  [{tag}] {body}")
            elif f"<{tag}" in chunk:
                print(f"  [{tag}] (empty/self-closing present)")
        # all numeric distance-ish
        dists = re.findall(
            r"<(RadiusDistance|StreetDistance|InfluenceRadius|ResolverRange|"
            r"MaxStreetDistance|Range)>([^<]+)</\1>",
            chunk,
        )
        print("  dists", dists)
        # referenced GUIDs in Buffs
        refs = re.findall(r"<GUID>(\d+)</GUID>", chunk)
        print("  nested GUIDs", refs[:20])
        return
    print("NOT FOUND", guid, label)


dump_guid(3190, "Sandals")
dump_guid(3174, "Bread")
dump_guid(3185, "Garum")
dump_guid(3177, "Wine")
# find Tavern
for m in re.finditer(r"<Asset>(.*?)</Asset>", xml, re.S):
    chunk = m.group(1)
    nm = re.search(r"<Name>([^<]+)</Name>", chunk)
    if not nm:
        continue
    if "Tavern" in nm.group(1) and nm.group(1).startswith("Public"):
        g = re.search(r"<GUID>(\d+)</GUID>", chunk)
        dump_guid(int(g.group(1)), nm.group(1))
        break

