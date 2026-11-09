#!/usr/bin/env python3
"""Extract ConstructionMenu hierarchy from assets.xml for the stamp editor."""

from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
ASSETS = ROOT / "_gamedata" / "assets.xml"
OUT = ROOT / "_gamedata" / "construction_menu.json"

# From ConstructionMenu LinearBuildingsMenu (GUID 41248).
REGION_ROOTS = {
    "Celtic": {
        "need_categories": [41329, 41330, 41331, 41328, 41327],
        "infrastructure": 55978,
        "materials": 41334,
    },
    "Roman": {
        "need_categories": [41362, 41363, 41364, 41365],
        "infrastructure": 41252,
        "materials": 41253,
    },
    # Delta / Ägypten (DLC03) — LinearBuildingsMenu Egyptian.
    "Egyptian": {
        "need_categories": [149688, 152078, 152079, 152081, 152083],
        "infrastructure": 149687,
        "materials": 149686,
    },
}


def short_cat_name(name: str) -> str:
    n = name
    for prefix in (
        "Construction Menu Greco-Roman ",
        "Construction Menu Egyptian ",
        "Construction Menu Celtic ",
        "Construction Menu Roman ",
        "Construction Menu RC ",
        "Celtic Pins Middle ",
        "Roman Pins Middle ",
        "Egyptian Pins Middle - ",
        "Egyptian Pins Middle ",
        "Celtic ",
        "Roman ",
        "Egyptian ",
    ):
        if n.startswith(prefix):
            n = n[len(prefix) :]
    for suffix in (" List",):
        if n.endswith(suffix):
            n = n[: -len(suffix)]
    n = n.lstrip("- ").strip()
    return n.strip() or name


def short_chain_name(name: str) -> str:
    n = name
    for prefix in (
        "Production Chain Egyptian ",
        "Production Chain Celtic ",
        "Production Chain Roman Celtic ",
        "Production Chain Roman ",
        "Production Chain ",
    ):
        if n.startswith(prefix):
            n = n[len(prefix) :]
            break
    return n.strip() or name


def entry_is_usable(entry: dict) -> bool:
    """Drop unresolved stubs (GUID-only / no icon) that make empty UI buttons."""
    name = entry.get("name") or ""
    if name.startswith("GUID "):
        return False
    if entry.get("kind") == "building" and not entry.get("icon"):
        return False
    if entry.get("kind") == "category":
        kids = [e for e in entry.get("entries", []) if entry_is_usable(e)]
        entry["entries"] = kids
        return bool(kids) or bool(entry.get("icon"))
    if entry.get("kind") == "chain":
        return bool(entry.get("buildings"))
    return True


def parse_assets(text: str):
    templates: dict[int, str] = {}
    names: dict[int, str] = {}
    icons: dict[int, str | None] = {}
    categories: dict[int, list[int]] = {}
    chains: dict[int, list[int]] = {}
    category_icons: dict[int, int] = {}
    secondary_icons: dict[int, int] = {}

    for m in re.finditer(r"<Asset>(.*?)</Asset>", text, re.S):
        chunk = m.group(1)
        tmpl_m = re.search(r"<Template>([^<]+)</Template>", chunk)
        guid_m = re.search(r"<Standard>\s*<GUID>(\d+)</GUID>", chunk, re.S)
        if not tmpl_m or not guid_m:
            continue
        guid = int(guid_m.group(1))
        tmpl = tmpl_m.group(1)
        templates[guid] = tmpl
        name_m = re.search(r"<Standard>\s*<GUID>\d+</GUID>\s*<Name>([^<]+)</Name>", chunk, re.S)
        if name_m:
            names[guid] = name_m.group(1)
        icon_m = re.search(r"<IconFilename>([^<]+)</IconFilename>", chunk)
        icons[guid] = icon_m.group(1) if icon_m else None

        if tmpl == "ConstructionCategory":
            cc = re.search(r"<ConstructionCategory>(.*?)</ConstructionCategory>", chunk, re.S)
            if cc:
                categories[guid] = [int(x) for x in re.findall(r"<Building>(\d+)</Building>", cc.group(1))]
                cat_m = re.search(r"<CategoryIcon>(\d+)</CategoryIcon>", cc.group(1))
                if cat_m:
                    category_icons[guid] = int(cat_m.group(1))
                sec_m = re.search(r"<SecondaryIcon>(\d+)</SecondaryIcon>", cc.group(1))
                if sec_m:
                    secondary_icons[guid] = int(sec_m.group(1))
        elif tmpl == "ProductionChain":
            pc = re.search(r"<ProductionChain>(.*?)</ProductionChain>", chunk, re.S)
            if pc:
                # Preserve order, unique.
                seen = set()
                ordered = []
                for b in re.findall(r"<Building>(\d+)</Building>", pc.group(1)):
                    bi = int(b)
                    if bi not in seen:
                        seen.add(bi)
                        ordered.append(bi)
                chains[guid] = ordered

    return templates, names, icons, categories, chains, category_icons, secondary_icons


def resolve_entry(
    guid: int,
    templates: dict[int, str],
    names: dict[int, str],
    icons: dict[int, str | None],
    categories: dict[int, list[int]],
    chains: dict[int, list[int]],
    depth: int = 0,
) -> dict | None:
    if depth > 6:
        return None
    tmpl = templates.get(guid, "")
    name = names.get(guid, f"GUID {guid}")

    if guid in chains or tmpl == "ProductionChain":
        blds = chains.get(guid, [])
        return {
            "kind": "chain",
            "guid": guid,
            "name": name,
            "short_name": short_chain_name(name),
            "icon": icons.get(guid),
            "buildings": blds,
        }

    if guid in categories or tmpl == "ConstructionCategory":
        kids = []
        for child in categories.get(guid, []):
            e = resolve_entry(child, templates, names, icons, categories, chains, depth + 1)
            if e:
                kids.append(e)
        return {
            "kind": "category",
            "guid": guid,
            "name": name,
            "short_name": short_cat_name(name),
            "icon": icons.get(guid),
            "entries": kids,
        }

    # Leaf building / other placeable.
    return {
        "kind": "building",
        "guid": guid,
        "name": name,
        "short_name": short_cat_name(name) if name.startswith("Construction") else name.split()[-1] if False else None,
        "icon": icons.get(guid),
    }


def fix_building_short(entry: dict, names: dict[int, str]) -> dict:
    if entry["kind"] == "building":
        name = names.get(entry["guid"], entry["name"])
        entry["name"] = name
        # Prefer readable short label later in Rust via catalog.
        entry["short_name"] = None
    elif entry["kind"] == "category":
        entry["entries"] = [fix_building_short(e, names) for e in entry.get("entries", [])]
    return entry


def resolve_icon_guid(guid: int | None, icons: dict[int, str | None]) -> str | None:
    if guid is None:
        return None
    return icons.get(guid)


def build_region_tabs(
    region: str,
    cfg: dict,
    templates,
    names,
    icons,
    categories,
    chains,
    category_icons,
    secondary_icons,
) -> list[dict]:
    tabs = []
    for cid in cfg["need_categories"]:
        e = resolve_entry(cid, templates, names, icons, categories, chains)
        if e and e["kind"] == "category":
            cat_guid = category_icons.get(cid)
            sec_guid = secondary_icons.get(cid)
            entries = [
                fix_building_short(x, names)
                for x in e.get("entries", [])
            ]
            entries = [x for x in entries if entry_is_usable(x)]
            # Prefer Standard IconFilename as portrait when CategoryIcon resolves poorly.
            cat_icon = resolve_icon_guid(cat_guid, icons) or e.get("icon")
            tabs.append(
                {
                    "id": f"need-{cid}",
                    "guid": cid,
                    "label": e["short_name"],
                    "role": "need",
                    "category_icon": cat_icon,
                    "secondary_icon": resolve_icon_guid(sec_guid, icons),
                    "entries": entries,
                }
            )
    for role, key in (("infrastructure", "infrastructure"), ("materials", "materials")):
        cid = cfg[key]
        e = resolve_entry(cid, templates, names, icons, categories, chains)
        if e and e["kind"] == "category":
            cat_guid = category_icons.get(cid)
            entries = [
                fix_building_short(x, names)
                for x in e.get("entries", [])
            ]
            entries = [x for x in entries if entry_is_usable(x)]
            tabs.append(
                {
                    "id": f"{role}-{cid}",
                    "guid": cid,
                    "label": e["short_name"] or role.title(),
                    "role": role,
                    "category_icon": resolve_icon_guid(cat_guid, icons) or e.get("icon"),
                    "secondary_icon": None,
                    "entries": entries,
                }
            )
    return tabs


def main() -> None:
    text = ASSETS.read_text(encoding="utf-8", errors="ignore")
    templates, names, icons, categories, chains, category_icons, secondary_icons = parse_assets(
        text
    )
    regions = {}
    for region, cfg in REGION_ROOTS.items():
        regions[region] = {
            "tabs": build_region_tabs(
                region,
                cfg,
                templates,
                names,
                icons,
                categories,
                chains,
                category_icons,
                secondary_icons,
            )
        }

    # Flat GUID→icon map for chain building tiles.
    building_icons = {
        str(g): path
        for g, path in icons.items()
        if path and templates.get(g) not in ("ProductionChain", "ConstructionCategory")
    }

    # Module Field / Pasture assets often share generic farm_fields.png — remap to the
    # matching Production Field crop icon so canvas SubTileGrid cells show olives/oats/…
    crop_icon_by_token = {}
    for g, path in building_icons.items():
        if not path or "farm_fields" in path or "animal_compound" in path:
            continue
        name = (names.get(int(g), "") if g.isdigit() else "") or ""
        if not name.startswith("Production Field") and not name.startswith("Production Pasture"):
            continue
        low = name.lower()
        for tok in (
            "olives", "olive", "oats", "wheat", "flax", "hemp", "grape", "lavender",
            "barley", "herb", "sheep", "wool", "pig", "horse", "reed",
        ):
            if tok in low:
                crop_icon_by_token.setdefault(tok, path)
    for g, path in list(building_icons.items()):
        if not path or ("farm_fields" not in path and "animal_compound" not in path):
            continue
        name = (names.get(int(g), "") if g.isdigit() else "") or ""
        if "Module Field" not in name and "Module Pasture" not in name:
            continue
        low = name.lower()
        for tok, crop_path in crop_icon_by_token.items():
            if tok in low:
                building_icons[g] = crop_path
                break

    out = {
        "source": "assets.xml ConstructionMenu LinearBuildingsMenu",
        "regions": regions,
        "building_icons": building_icons,
        "chains": {
            str(g): {
                "guid": g,
                "name": names.get(g, ""),
                "short_name": short_chain_name(names.get(g, "")),
                "icon": icons.get(g),
                "buildings": blds,
            }
            for g, blds in chains.items()
        },
    }
    OUT.write_text(json.dumps(out, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    tab_n = sum(len(r["tabs"]) for r in regions.values())
    entry_n = sum(len(t["entries"]) for r in regions.values() for t in r["tabs"])
    print(f"wrote {OUT} regions={len(regions)} tabs={tab_n} top_entries={entry_n} chains={len(chains)}")
    for region, data in regions.items():
        print(region, [t["label"] for t in data["tabs"]])


if __name__ == "__main__":
    main()

