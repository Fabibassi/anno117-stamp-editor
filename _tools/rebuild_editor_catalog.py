"""Rebuild editor_catalog.json with richer production + street metadata."""
from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "_gamedata" / "assets.xml"
OUT = ROOT / "_gamedata" / "editor_catalog.json"
CFG_SIZES = ROOT / "_gamedata" / "cfg_build_sizes.json"
CFG_BLOCKER_MINS = ROOT / "_gamedata" / "cfg_blocker_mins.json"
TEXTS_EN = ROOT / "_gamedata" / "texts_english.json"
TEXTS_DE = ROOT / "_gamedata" / "texts_german.json"

PREFIX_RES = [
    re.compile(r"^Production Construction\s+", re.I),
    re.compile(r"^Production Mountain\s+", re.I),
    re.compile(r"^Production Marsh\s+", re.I),
    re.compile(r"^Production Field\s+", re.I),
    re.compile(r"^Production\s+", re.I),
    re.compile(r"^Module Marsh Pasture\s+", re.I),
    re.compile(r"^Module Marsh Field\s+", re.I),
    re.compile(r"^Module Marsh\s+", re.I),
    re.compile(r"^Module Polygon\s+", re.I),
    re.compile(r"^Module\s+", re.I),
    re.compile(r"^Warehouse\s+", re.I),
    re.compile(r"^Street\s+", re.I),
    re.compile(r"^Institution\s+", re.I),
    re.compile(r"^Ornament\s+", re.I),
    re.compile(r"^HoF Ornament\s+", re.I),
]


def short_name(name: str) -> str:
    s = name.strip()
    # Keep a useful category word for warehouses / institutions.
    keep_wh = name.lower().startswith("warehouse ")
    keep_inst = name.lower().startswith("institution ")
    for rx in PREFIX_RES:
        s2 = rx.sub("", s)
        if s2 != s:
            s = s2
            break
    if keep_wh and not s.lower().startswith("warehouse"):
        s = f"Warehouse {s}"
    if keep_inst and not s.lower().startswith("institution"):
        # "Institution Roman Celtic Illness" → "Infirmary Celtic" style
        s = re.sub(r"^(Roman|Celtic)\s+", "", s, flags=re.I)
        s = f"Institution {s}"
    s = re.sub(r"\s+", " ", s).strip()
    s = re.sub(r"\bPamsy\b", "", s, flags=re.I)
    s = re.sub(r"\bAudioPool\b", "", s, flags=re.I)
    s = re.sub(r"\s+\d+\s*$", "", s).strip()
    s = re.sub(r"\s+", " ", s).strip()
    return s or name


def classify_road(name: str, template: str, speed: float | None, tileset: str | None) -> str | None:
    n = name.lower()
    t = (tileset or "").lower()
    if "AqueductConnector" in template or ("aqueduct" in n and "minground" in n):
        return "aqueduct"
    is_street = template == "Street" or "street" in n or "road" in n or "quay street" in n
    if not is_street:
        return None
    if speed is not None and speed >= 1.4:
        return "stone"
    if any(k in t for k in ("street_02", "street_03", "paved", "cobble", "quay_street_02", "quay_street_03")):
        return "stone"
    if any(k in n for k in ("paved", "stone", "cobble", "marble")):
        return "stone"
    if any(k in n for k in ("earthen", "dirt", "earth", "wood")) or "street_01" in t or "quay_street_01" in t:
        return "dirt"
    return "dirt"


def size_from_name(name: str) -> list[float] | None:
    m = re.search(r"(?<!\d)(\d+)\s*[xX×]\s*(\d+)(?!\d)", name)
    if not m:
        return None
    w, h = float(m.group(1)), float(m.group(2))
    if w <= 0 or h <= 0 or w > 32 or h > 32:
        return None
    return [w, h]


def normalize_cfg_key(path: str) -> str:
    key = path.replace("\\", "/").lower().lstrip("/")
    for pref in ("data/", "shared/"):
        if key.startswith(pref):
            key = key[len(pref) :]
    return key


def load_cfg_sizes() -> dict[str, list[float]]:
    if not CFG_SIZES.exists():
        return {}
    raw = json.loads(CFG_SIZES.read_text(encoding="utf-8"))
    out: dict[str, list[float]] = {}
    for k, v in raw.items():
        if not isinstance(v, list) or len(v) < 2:
            continue
        out[normalize_cfg_key(k)] = [float(v[0]), float(v[1])]
    return out


def load_cfg_blocker_mins() -> dict[str, list[float]]:
    if not CFG_BLOCKER_MINS.exists():
        return {}
    raw = json.loads(CFG_BLOCKER_MINS.read_text(encoding="utf-8"))
    out: dict[str, list[float]] = {}
    for k, v in raw.items():
        if not isinstance(v, list) or len(v) < 2:
            continue
        out[normalize_cfg_key(k)] = [float(v[0]), float(v[1])]
    return out


def blocker_min_from_cfg(
    chunk: str, cfg_mins: dict[str, list[float]]
) -> list[float] | None:
    for m in re.finditer(r"<Filename>([^<]+\.cfg)</Filename>", chunk, re.I):
        if is_marsh_variant(m.group(1)):
            continue
        key = normalize_cfg_key(m.group(1))
        bmin = cfg_mins.get(key) or cfg_mins.get("#" + key.split("/")[-1])
        if bmin:
            return bmin
    return None


def is_marsh_variant(path: str) -> bool:
    p = path.replace("\\", "/").lower()
    name = p.rsplit("/", 1)[-1]
    if "no_marsh" in name:
        return False
    return "_marsh" in name or "/marsh/" in p


def sizes_from_cfg(chunk: str, cfg_sizes: dict[str, list[float]]) -> tuple[list[float] | None, list[float] | None]:
    """Return (default_size, marsh_size) from Filename variations."""
    default = None
    marsh = None
    for m in re.finditer(r"<Filename>([^<]+\.cfg)</Filename>", chunk, re.I):
        key = normalize_cfg_key(m.group(1))
        sz = cfg_sizes.get(key) or cfg_sizes.get("#" + key.split("/")[-1])
        if not sz:
            continue
        if is_marsh_variant(m.group(1)):
            if marsh is None:
                marsh = sz
        elif default is None:
            default = sz
    return default, marsh


def size_from_cfg(chunk: str, cfg_sizes: dict[str, list[float]]) -> list[float] | None:
    default, marsh = sizes_from_cfg(chunk, cfg_sizes)
    return default or marsh


def guess_size(
    name: str,
    template: str,
    category: str,
    chunk: str,
    cfg_sizes: dict[str, list[float]],
) -> list[float]:
    named = size_from_name(name)
    if named:
        return named

    from_cfg = size_from_cfg(chunk, cfg_sizes)
    if from_cfg:
        return from_cfg

    t = template.lower()
    n = name.lower()

    if category == "street" or template == "Street":
        return [1.0, 1.0]
    if "AqueductConnector" in template or (
        "Aqueduct" in name and "MinGround" in name
    ):
        return [1.0, 1.0]
    if "Aqueduct" in template or name.startswith("Aqueduct "):
        return [3.0, 3.0]
    if category == "warehouse" or "warehouse" in t:
        return [4.0, 4.0]
    if category == "ornament":
        return [1.0, 1.0]

    # Pasture / field modules are multi-tile compounds, not 1×1 markers.
    if any(
        k in t
        for k in (
            "modulemarshfield",
            "modulepolygonfield",
            "modulefield",
            "productionmodule",
            "pasture",
        )
    ) or ("module" in n and ("pasture" in n or "field" in n)):
        return [3.0, 3.0]

    if category == "module":
        if "silo" in t or "silo" in n:
            return [2.0, 2.0]
        return [2.0, 2.0]

    if category == "residence":
        return [3.0, 3.0]

    # Default production / institution / public footprint in Anno 117.
    if category in ("production", "institution", "public"):
        return [3.0, 3.0]

    return [3.0, 3.0]


def parse_regions(chunk: str, name: str) -> list[str]:
    """AssociatedRegions + name heuristics → Celtic / Roman / Egyptian."""
    found: list[str] = []
    assoc = re.search(r"<AssociatedRegions>([^<]+)</AssociatedRegions>", chunk)
    if assoc:
        for part in re.split(r"[;,/|\s]+", assoc.group(1)):
            p = part.strip()
            if not p:
                continue
            key = p[:1].upper() + p[1:].lower() if p.lower() != "egyptian" else "Egyptian"
            # normalize casing
            low = p.lower()
            if low == "celtic":
                key = "Celtic"
            elif low == "roman":
                key = "Roman"
            elif low in ("egyptian", "egypt", "delta"):
                key = "Egyptian"
            else:
                key = p
            if key not in found:
                found.append(key)
    n = name.lower()
    if "celtic" in n or "albion" in n:
        if "Celtic" not in found:
            found.append("Celtic")
    if "roman" in n or "latium" in n:
        if "Roman" not in found:
            found.append("Roman")
    if "egyptian" in n or "egypt" in n or "delta" in n or "nil" in n:
        if "Egyptian" not in found:
            found.append("Egyptian")
    return found


ATTR_TAGS = (
    "FireSafety",
    "Health",
    "Happiness",
    "Workforce",
    "Income",
    "Money",
    "Faith",
    "Knowledge",
    "Prestige",
    "Attractiveness",
    "ResidenceProsperity",
    "Population",
)


def parse_effect_source(chunk: str) -> tuple[float | None, float | None]:
    es = re.search(r"<EffectSource>(.*?)</EffectSource>", chunk, re.S)
    if not es or not es.group(1).strip():
        return None, None
    body = es.group(1)
    rd = re.search(r"<RadiusDistance>([^<]+)</RadiusDistance>", body)
    sd = re.search(r"<StreetDistance>([^<]+)</StreetDistance>", body)
    return (
        float(rd.group(1)) if rd else None,
        float(sd.group(1)) if sd else None,
    )


def parse_buff_bonuses(chunk: str) -> list[dict]:
    """AdditionalAttributes / NeedAttributes style bonuses."""
    out: list[dict] = []
    for attr in ATTR_TAGS:
        for m in re.finditer(
            rf"<{attr}>\s*(?:<AmountOrPercent>\s*)?(?:<Value>)?([^<\s]+)(?:</Value>)?",
            chunk,
            re.S,
        ):
            raw = m.group(1).strip()
            try:
                val = float(raw)
            except ValueError:
                continue
            # Skip population-tier GUID-like values (> 1000 and integer-ish)
            if attr == "Population" and abs(val) >= 100:
                continue
            out.append({"attr": attr, "value": val})
    # ProvidedNeed (public service)
    for m in re.finditer(r"<ProvidedNeed>(\d+)</ProvidedNeed>", chunk):
        out.append({"attr": "ProvidedNeed", "value": float(m.group(1))})
    return out


def index_effect_assets(text: str) -> dict[int, dict]:
    """GUID → {scope, bonuses, name} for Effect + BuildingBuff assets."""
    effects: dict[int, dict] = {}
    buffs: dict[int, list[dict]] = {}
    for chunk in text.split("<Asset>")[1:]:
        g = re.search(r"<GUID>(\d+)</GUID>", chunk)
        n = re.search(r"<Name>([^<]+)</Name>", chunk)
        if not g or not n:
            continue
        guid = int(g.group(1))
        name = n.group(1)
        tmpl_m = re.search(r"<Template>([^<]+)</Template>", chunk)
        tmpl = tmpl_m.group(1) if tmpl_m else ""
        if tmpl == "BuildingBuff" or name.startswith("Buff "):
            buffs[guid] = parse_buff_bonuses(chunk)
        if tmpl == "Effect" or name.startswith("Effect "):
            scope_m = re.search(r"<EffectScope>([^<]+)</EffectScope>", chunk)
            buff_ids = [int(x) for x in re.findall(r"<Buffs>.*?<GUID>(\d+)</GUID>", chunk, re.S)]
            # also flat Item GUID under Buffs
            if not buff_ids:
                buff_block = re.search(r"<Buffs>(.*?)</Buffs>", chunk, re.S)
                if buff_block:
                    buff_ids = [int(x) for x in re.findall(r"<GUID>(\d+)</GUID>", buff_block.group(1))]
            bonuses: list[dict] = []
            for bid in buff_ids:
                bonuses.extend(buffs.get(bid, []))
            effects[guid] = {
                "scope": scope_m.group(1) if scope_m else None,
                "bonuses": bonuses,
                "name": name,
                "buff_ids": buff_ids,
            }
    # Second pass: buffs may appear after effects in file — refill bonuses
    for eff in effects.values():
        if eff["bonuses"]:
            continue
        bonuses = []
        for bid in eff.get("buff_ids") or []:
            bonuses.extend(buffs.get(bid, []))
        eff["bonuses"] = bonuses
    return effects


def resolve_building_effects(
    chunk: str, effects: dict[int, dict]
) -> tuple[str | None, list[dict], list[int]]:
    """Return (effect_scope, bonuses, effect_guids)."""
    ids: list[int] = []
    pse = re.search(r"<PublicServiceEffect>(\d+)</PublicServiceEffect>", chunk)
    if pse:
        ids.append(int(pse.group(1)))
    ids.extend(int(x) for x in re.findall(r"<FunctionalEffect>(\d+)</FunctionalEffect>", chunk))
    scope = None
    bonuses: list[dict] = []
    for eid in ids:
        eff = effects.get(eid)
        if not eff:
            continue
        if scope is None and eff.get("scope"):
            scope = eff["scope"]
        for b in eff.get("bonuses") or []:
            if b not in bonuses:
                bonuses.append(b)
    # Local AttributeProvider values (often pollution) also radiate with EffectSource.
    local = parse_buff_bonuses(chunk)
    for b in local:
        if b["attr"] == "ProvidedNeed":
            continue
        if b not in bonuses:
            bonuses.append(b)
    return scope, bonuses, ids


def service_kind_from_name(name: str, category: str) -> str | None:
    n = name.lower()
    if "institution" in n and "fire" in n:
        return "fire"
    if "institution" in n and "riot" in n:
        return "riot"
    if "institution" in n and "illness" in n:
        return "illness"
    if category == "public":
        return "public"
    if category == "production":
        return "production"
    if category == "institution":
        return "institution"
    return None


def is_palette_noise(name: str, template: str) -> bool:
    n = name.lower()
    t = template.lower()
    noise = (
        "buff ",
        "buff_",
        "asset pool",
        "audiopool",
        "pamsy",
        "bubble:",
        "old_",
        "effect ",
        "reason_",
        "warreason_",
        "blockproduction",
        "keep state checker",
        "governordecision",
        "welcome all needs",
        "no street connection",
        "enemy troops",
        "fertility missing",
        "modules missing",
        "free area missing",
        "workforce low",
        "water missing",
        "fuel missing",
        "input goods missing",
        "full output storage",
        "production paused",
        "movein ",
        "idle pamsy",
        "supplied pamsy",
        "riot ",
        "illness ",
        "fire audiopool",
    )
    if any(k in n for k in noise):
        return True
    if "pool" in t and "production" not in t:
        return True
    return False


def load_text_table(path: Path) -> dict[str, str]:
    if not path.is_file():
        print(f"warning: missing {path} — run extract_game_texts.py first")
        return {}
    return json.loads(path.read_text(encoding="utf-8"))


def loca_names(
    oasis_id: str | None, texts_en: dict[str, str], texts_de: dict[str, str]
) -> tuple[str | None, str | None]:
    if not oasis_id:
        return None, None
    return texts_en.get(oasis_id), texts_de.get(oasis_id)


def classify_need_kind(need_name: str, product_name: str | None) -> str:
    """service = public coverage need; goods = food/fashion/etc. consumption."""
    pn = (product_name or "").lower()
    nn = need_name.lower()
    if pn.startswith("service ") or " public " in nn or nn.startswith("need ") and "public" in nn:
        return "service"
    if pn.startswith("good ") or "food" in nn or "fashion" in nn or "tech need" in nn:
        return "goods"
    if "service" in pn:
        return "service"
    return "goods"


def parse_residence_needs(chunk: str) -> list[dict]:
    out: list[dict] = []
    m = re.search(r"<NeedsList>(.*?)</NeedsList>", chunk, re.S)
    if not m:
        return out
    for item in re.finditer(r"<Item>(.*?)</Item>", m.group(1), re.S):
        body = item.group(1)
        nm = re.search(r"<Need>(\d+)</Need>", body)
        if not nm:
            continue
        rate_m = re.search(r"<NeedConsumptionRate>([^<]+)</NeedConsumptionRate>", body)
        buff_only = "<IsOnlyAvailableThroughBuff>1</IsOnlyAvailableThroughBuff>" in body
        entry: dict = {"need_guid": int(nm.group(1))}
        if rate_m:
            try:
                entry["consumption_rate"] = float(rate_m.group(1))
            except ValueError:
                pass
        if buff_only:
            entry["buff_only"] = True
        out.append(entry)
    return out


def parse_upgrade_guids(chunk: str) -> list[int]:
    m = re.search(r"<PossibleUpgrades>(.*?)</PossibleUpgrades>", chunk, re.S)
    if not m:
        return []
    return [int(x) for x in re.findall(r"<UpgradeGUID>(\d+)</UpgradeGUID>", m.group(1))]


def main() -> None:
    text = ASSETS.read_text(encoding="utf-8", errors="replace")
    buildings: dict[str, dict] = {}
    products: dict[str, dict] = {}
    needs: dict[str, dict] = {}
    cfg_sizes = load_cfg_sizes()
    cfg_mins = load_cfg_blocker_mins()
    texts_en = load_text_table(TEXTS_EN)
    texts_de = load_text_table(TEXTS_DE)
    print(f"cfg size entries: {len(cfg_sizes)}")
    print(f"cfg blocker_min entries: {len(cfg_mins)}")
    print(f"loca EN={len(texts_en)} DE={len(texts_de)}")
    print("indexing effects/buffs…")
    effects = index_effect_assets(text)
    print(f"effect assets: {len(effects)}")

    for chunk in text.split("<Asset>")[1:]:
        tmpl_m = re.search(r"<Template>([^<]+)</Template>", chunk)
        tmpl = tmpl_m.group(1) if tmpl_m else ""
        g = re.search(r"<Standard>\s*<GUID>(\d+)</GUID>\s*<Name>([^<]+)</Name>", chunk, re.S)
        if not g:
            g2 = re.search(r"<GUID>(\d+)</GUID>", chunk)
            n2 = re.search(r"<Name>([^<]+)</Name>", chunk)
            if not g2 or not n2:
                continue
            guid, name = int(g2.group(1)), n2.group(1)
        else:
            guid, name = int(g.group(1)), g.group(2)

        oasis = re.search(r"<Text>\s*<OasisId>([^<]+)</OasisId>", chunk, re.S)
        oasis_id = oasis.group(1).strip() if oasis else None
        icon_m = re.search(r"<IconFilename>([^<]+)</IconFilename>", chunk)
        icon = icon_m.group(1).strip() if icon_m else None

        if tmpl in ("Product", "ProductItem") or tmpl.endswith("Product"):
            name_en, name_de = loca_names(oasis_id, texts_en, texts_de)
            products[str(guid)] = {
                "guid": guid,
                "name": name,
                "template": tmpl,
                "oasis_id": oasis_id,
                "name_en": name_en,
                "name_de": name_de,
                "icon": icon,
            }

        if tmpl == "Need":
            np_m = re.search(r"<NeedProduct>(\d+)</NeedProduct>", chunk)
            need_product = int(np_m.group(1)) if np_m else None
            name_en, name_de = loca_names(oasis_id, texts_en, texts_de)
            needs[str(guid)] = {
                "guid": guid,
                "name": name,
                "need_product": need_product,
                "icon": icon,
                "oasis_id": oasis_id,
                "name_en": name_en,
                "name_de": name_de,
                # kind filled after products pass (second pass below)
                "kind": "goods",
            }

        factory = re.search(r"<FactoryBase>(.*?)</FactoryBase>", chunk, re.S)
        outputs = []
        inputs = []
        cycle = None
        needs_fuel = False
        if factory:
            fb = factory.group(1)
            needs_fuel = "<NeedsFuelInput>1</NeedsFuelInput>" in fb or "<NeedsFuelInput>true</NeedsFuelInput>" in fb.lower()
            cm = re.search(r"<CycleTime>([^<]+)</CycleTime>", fb)
            if cm:
                cycle = float(cm.group(1))
            for m in re.finditer(r"<FactoryOutputs>(.*?)</FactoryOutputs>", fb, re.S):
                for item in re.finditer(r"<Item>(.*?)</Item>", m.group(1), re.S):
                    p = re.search(r"<Product>(\d+)</Product>", item.group(1))
                    a = re.search(r"<Amount>([^<]+)</Amount>", item.group(1))
                    if p:
                        outputs.append(
                            {"product": int(p.group(1)), "amount": float(a.group(1)) if a else 1.0}
                        )
            for m in re.finditer(r"<FactoryInputs>(.*?)</FactoryInputs>", fb, re.S):
                for item in re.finditer(r"<Item>(.*?)</Item>", m.group(1), re.S):
                    p = re.search(r"<Product>(\d+)</Product>", item.group(1))
                    a = re.search(r"<Amount>([^<]+)</Amount>", item.group(1))
                    if p:
                        inputs.append(
                            {"product": int(p.group(1)), "amount": float(a.group(1)) if a else 1.0}
                        )

        if cycle is None:
            cm = re.search(r"<CycleTime>([^<]+)</CycleTime>", chunk)
            if cm:
                cycle = float(cm.group(1))

        infl = re.search(r"<InfluenceRadius>([^<]+)</InfluenceRadius>", chunk)
        needed = re.search(r"<NeededArea>([^<]+)</NeededArea>", chunk)
        # Plant farms use ModuleLimits/Main/Limit as tile-area need (e.g. oats=80).
        # Animal farms use the same for module-slot need (e.g. sheep=3).
        mod_lim = re.search(
            r"<ModuleLimits>\s*<Main>\s*<Limit>(\d+)</Limit>", chunk, re.S
        )
        farm_type_m = re.search(r"<FarmType>([^<]+)</FarmType>", chunk)
        speed_m = re.search(r"<LogicisticSpeedFactor>([^<]+)</LogicisticSpeedFactor>", chunk)
        speed = float(speed_m.group(1)) if speed_m else None
        tileset_m = re.search(r"<TileSetCfgFolder>([^<]+)</TileSetCfgFolder>", chunk)
        tileset = tileset_m.group(1) if tileset_m else None
        has_street_node = bool(re.search(r"<Street\b", chunk))
        constructable = "<Constructable" in chunk
        regions = parse_regions(chunk, name)
        radius_distance, street_distance = parse_effect_source(chunk)
        effect_scope, effect_bonuses, effect_guids = resolve_building_effects(chunk, effects)

        category = "other"
        if name.startswith("Production "):
            category = "production"
        elif "Warehouse" in name:
            category = "warehouse"
        elif "Module " in name or "Module" in tmpl or "Farmfield" in tmpl or "Pasture" in tmpl:
            category = "module"
        elif tmpl == "Street" or has_street_node or "Street " in name or name.startswith("Street"):
            category = "street"
        elif "Institution" in name or tmpl in (
            "CityInstitutionBuilding",
            "MiniInstitutionBuilding",
        ):
            category = "institution"
        elif (
            name.startswith("Public ")
            or "PublicService" in tmpl
            or tmpl == "PublicServiceBuilding"
        ):
            # Market, Tavern, baths, … — must not fall through as "other" (dropped).
            category = "public"
        elif "Ornament" in name:
            category = "ornament"
        elif "Residence" in name or "House" in tmpl:
            category = "residence"
        elif "AqueductConnector" in tmpl or (
            "Aqueduct" in name and ("MinGround" in name or "MaxGround" in name)
        ):
            # MeshGraph brush (like roads) — MinGround ground span, MaxGround raised.
            category = "street"
        elif "Aqueduct" in tmpl or name.startswith("Aqueduct "):
            # Basin / Cistern buildings.
            category = "public"

        road_kind = classify_road(name, tmpl, speed, tileset)
        if category == "street" and (
            "AqueductConnector" in tmpl
            or ("Aqueduct" in name and ("MinGround" in name or "MaxGround" in name))
        ):
            road_kind = "aqueduct"
        if category != "street" and road_kind:
            category = "street"

        # Defaults when EffectSource empty but effect exists (Anno often omits them).
        if effect_scope == "StreetDistance" and street_distance is None:
            if category in ("public", "institution"):
                street_distance = 28.0
                if radius_distance is None:
                    radius_distance = 24.0
        if effect_scope == "Radius" and radius_distance is None and effect_guids:
            radius_distance = 24.0

        svc = service_kind_from_name(name, category)

        keep = (
            category != "other"
            or outputs
            or inputs
            or infl
            or road_kind
            or has_street_node
            or street_distance is not None
            or radius_distance is not None
            or bool(effect_guids)
        )
        if not keep:
            continue

        if is_palette_noise(name, tmpl) and not outputs and category not in ("street",):
            # Keep streets; drop noise buff/pool entries from catalog clutter.
            if category not in ("production", "warehouse", "module", "institution", "public"):
                continue
            if not constructable and not outputs and not effect_guids:
                continue

        size = guess_size(name, tmpl, category, chunk, cfg_sizes)
        if category == "street" and "MaxGround" in name:
            # Brush cell is 1×1 like MinGround; asset AABB is often 3×3.
            size = [1.0, 1.0]
        _default_sz, marsh_sz = sizes_from_cfg(chunk, cfg_sizes)
        # If only marsh exists, size already is marsh; don't duplicate.
        if marsh_sz == size:
            marsh_sz = None
        blocker_min = blocker_min_from_cfg(chunk, cfg_mins)

        if category == "production" and outputs and cycle is None:
            cycle = 30.0

        name_en, name_de = loca_names(oasis_id, texts_en, texts_de)
        pop_m = re.search(r"<PopulationLevel>(\d+)</PopulationLevel>", chunk)
        residence_needs = parse_residence_needs(chunk) if category == "residence" else []
        upgrade_guids = parse_upgrade_guids(chunk) if category == "residence" else []
        buildings[str(guid)] = {
            "guid": guid,
            "name": name,
            "short_name": short_name(name),
            "name_en": name_en,
            "name_de": name_de,
            "template": tmpl,
            "category": category,
            "size": size,
            "size_marsh": marsh_sz,
            "blocker_min": blocker_min,
            "outputs": outputs,
            "inputs": inputs,
            "cycle_ms": cycle,
            "influence_radius": float(infl.group(1)) if infl else None,
            "needed_area": float(needed.group(1)) if needed else None,
            "module_limit": int(mod_lim.group(1)) if mod_lim else None,
            "farm_type": farm_type_m.group(1) if farm_type_m else None,
            "radius_distance": radius_distance,
            "street_distance": street_distance,
            "effect_scope": effect_scope,
            "effect_bonuses": effect_bonuses,
            "service_kind": svc,
            "road_kind": road_kind,
            "needs_fuel": needs_fuel,
            "logistic_speed_factor": speed,
            "tileset_folder": tileset,
            "oasis_id": oasis_id,
            "constructable": constructable,
            "regions": regions,
            "icon": icon,
            "population_level": int(pop_m.group(1)) if pop_m else None,
            "residence_needs": residence_needs,
            "upgrade_guids": upgrade_guids,
        }

    # PopulationLevel portraits → residence.population_icon (buildings share one house icon).
    pop_icons: dict[int, str] = {}
    for m in re.finditer(
        r"<Template>PopulationLevel</Template>\s*<Values>(.*?)</Values>\s*</Asset>",
        text,
        re.S,
    ):
        chunk = m.group(1)
        g = re.search(r"<GUID>(\d+)</GUID>", chunk)
        icon_m = re.search(r"<IconFilename>([^<]+)</IconFilename>", chunk)
        if g and icon_m:
            pop_icons[int(g.group(1))] = icon_m.group(1).replace("\\", "/")
    for b in buildings.values():
        pl = b.get("population_level")
        if pl and int(pl) in pop_icons:
            b["population_icon"] = pop_icons[int(pl)]

    # Classify need kinds using product names; enrich residence_needs entries.
    for nd in needs.values():
        prod = products.get(str(nd.get("need_product") or ""))
        pname = prod["name"] if prod else None
        nd["kind"] = classify_need_kind(nd["name"], pname)

    for b in buildings.values():
        enriched = []
        for item in b.get("residence_needs") or []:
            ng = item["need_guid"]
            nd = needs.get(str(ng), {})
            enriched.append(
                {
                    "need_guid": ng,
                    "kind": nd.get("kind", "goods"),
                    "need_product": nd.get("need_product"),
                    "consumption_rate": item.get("consumption_rate"),
                    "buff_only": item.get("buff_only", False),
                    "icon": nd.get("icon"),
                    "name": nd.get("name"),
                    "name_en": nd.get("name_en"),
                    "name_de": nd.get("name_de"),
                }
            )
        b["residence_needs"] = enriched

    # Reverse upgrade edges → base_guid (who upgrades into this).
    upgrade_into: dict[int, list[int]] = {}
    for b in buildings.values():
        if b.get("category") != "residence":
            continue
        for ug in b.get("upgrade_guids") or []:
            upgrade_into.setdefault(int(ug), []).append(int(b["guid"]))
    for b in buildings.values():
        if b.get("category") != "residence":
            continue
        parents = upgrade_into.get(int(b["guid"]), [])
        b["base_guids"] = parents
        # Walk upgrades forward for full forward chain from this node.
        chain = [int(b["guid"])]
        seen = {chain[0]}
        frontier = list(b.get("upgrade_guids") or [])
        while frontier:
            nxt = int(frontier.pop(0))
            if nxt in seen:
                continue
            seen.add(nxt)
            chain.append(nxt)
            child = buildings.get(str(nxt))
            if child:
                frontier.extend(child.get("upgrade_guids") or [])
        b["upgrade_chain"] = chain

    OUT.write_text(
        json.dumps(
            {"buildings": buildings, "products": products, "needs": needs},
            ensure_ascii=False,
        ),
        encoding="utf-8",
    )
    from collections import Counter

    cats = Counter(b["category"] for b in buildings.values())
    roads = Counter(b.get("road_kind") for b in buildings.values() if b.get("road_kind"))
    region_hits = Counter()
    for b in buildings.values():
        for r in b.get("regions") or []:
            region_hits[r] += 1
    with_io = sum(1 for b in buildings.values() if b["inputs"] or b["outputs"])
    fuel = sum(1 for b in buildings.values() if b.get("needs_fuel"))
    with_street = sum(1 for b in buildings.values() if b.get("street_distance"))
    with_radius = sum(1 for b in buildings.values() if b.get("radius_distance"))
    with_bonus = sum(1 for b in buildings.values() if b.get("effect_bonuses"))
    loca_b = sum(1 for b in buildings.values() if b.get("name_de") or b.get("name_en"))
    loca_p = sum(1 for p in products.values() if p.get("name_de") or p.get("name_en"))
    residences = sum(1 for b in buildings.values() if b.get("category") == "residence")
    with_needs = sum(1 for b in buildings.values() if b.get("residence_needs"))
    print(
        f"wrote {OUT} buildings={len(buildings)} products={len(products)} needs={len(needs)} "
        f"residences={residences} with_needs={with_needs} "
        f"with_io={with_io} fuel={fuel} street={with_street} radius={with_radius} "
        f"bonuses={with_bonus} loca_b={loca_b} loca_p={loca_p} "
        f"cats={dict(cats)} roads={dict(roads)} "
        f"regions={dict(region_hits)}"
    )
    r3087 = buildings.get("3087")
    if r3087:
        print(
            "sample residence 3087",
            r3087.get("short_name"),
            "upgrades",
            r3087.get("upgrade_guids"),
            "chain",
            r3087.get("upgrade_chain"),
            "needs",
            [(n.get("need_guid"), n.get("kind"), n.get("name")) for n in r3087.get("residence_needs") or []],
        )
    for sample in ("30754", "3190", "3528", "3527", "3177"):
        b = buildings.get(sample)
        if b:
            print(
                "sample",
                sample,
                b.get("short_name"),
                "R",
                b.get("radius_distance"),
                "S",
                b.get("street_distance"),
                "scope",
                b.get("effect_scope"),
                "kind",
                b.get("service_kind"),
                "bonuses",
                b.get("effect_bonuses"),
            )
    g = buildings.get("41811")
    m = buildings.get("5861")
    w = buildings.get("7055")
    print("granite", g and (g["short_name"], g["size"], g.get("regions")))
    print("warehouse", w and (w["short_name"], w["size"], w.get("size_marsh")))
    print("aurochs module", m and (m["short_name"], m["size"], m.get("regions")))
    cfg_hits = sum(
        1
        for b in buildings.values()
        if b["size"] != [3.0, 3.0] and b["size"] != [1.0, 1.0] and b["size"] != [4.0, 4.0]
    )
    print("non-default-ish sizes", cfg_hits)


if __name__ == "__main__":
    main()

