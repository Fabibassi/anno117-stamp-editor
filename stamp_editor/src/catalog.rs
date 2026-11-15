use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct IoAmount {
    pub product: u32,
    pub amount: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EffectBonus {
    pub attr: String,
    pub value: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BuildingDef {
    pub guid: u32,
    pub name: String,
    #[serde(default)]
    pub short_name: Option<String>,
    pub template: String,
    pub category: String,
    pub size: [f32; 2],
    /// Marsh-terrain footprint (Albion). When set, prefer over `size` on region 6626.
    #[serde(default)]
    pub size_marsh: Option<[f32; 2]>,
    /// BuildBlocker min corner `(xf, zf)` relative to pipette `Pos` (mesh pivot).
    /// When absent, the footprint is assumed centered: `(-w/2, -h/2)`.
    /// Aqueduct cisterns are *not* centered — see [`blocker_min_for`].
    #[serde(default)]
    pub blocker_min: Option<[f32; 2]>,
    pub outputs: Vec<IoAmount>,
    pub inputs: Vec<IoAmount>,
    pub cycle_ms: Option<f32>,
    pub influence_radius: Option<f32>,
    pub needed_area: Option<f32>,
    /// `ModuleOwner/ModuleLimits/Main/Limit` — tile area (PlantFarm) or module slots (AnimalFarm).
    #[serde(default)]
    pub module_limit: Option<u32>,
    /// `PlantFarm` | `AnimalFarm` | …
    #[serde(default)]
    pub farm_type: Option<String>,
    /// Circular area effect (EffectSource.RadiusDistance / EffectScope Radius).
    #[serde(default)]
    pub radius_distance: Option<f32>,
    /// Road-network reach (EffectSource.StreetDistance / EffectScope StreetDistance).
    #[serde(default)]
    pub street_distance: Option<f32>,
    /// `StreetDistance` | `Radius` | …
    #[serde(default)]
    pub effect_scope: Option<String>,
    #[serde(default)]
    pub effect_bonuses: Vec<EffectBonus>,
    /// fire / riot / illness / public / production / institution
    #[serde(default)]
    pub service_kind: Option<String>,
    pub road_kind: Option<String>,
    #[serde(default)]
    pub needs_fuel: bool,
    #[serde(default)]
    pub logistic_speed_factor: Option<f32>,
    #[serde(default)]
    pub tileset_folder: Option<String>,
    #[serde(default)]
    pub constructable: Option<bool>,
    /// Game AssociatedRegions / name hints: Celtic, Roman, Egyptian, …
    #[serde(default)]
    pub regions: Vec<String>,
}

/// Latium (19753) / Celtic (29526) Aqueduct Cistern — `AqueductDistributor`.
///
/// In-game footprint is always **3×3**. Pipette `Pos` is the mesh pivot, which sits
/// **1 tile left of the geometric center** (BuildBlocker ≈ `[-0.5, 2.5] × [-1.5, 1.5]`).
/// Writing centered `Pos = TL + 1.5` therefore lands the game footprint at `TL+1`
/// (one grass tile gap to a road at `TL-1`), while a size-from-AABB catalog of 4–5
/// makes the same bug look identical. Both size and pivot are forced here so a
/// wrong/`None` catalog entry cannot silently keep the +1 offset.
pub const AQUEDUCT_CISTERN_GUIDS: &[u32] = &[19753, 29526];

/// Canonical tile size for aqueduct cisterns (ignore catalog AABB guesses).
pub const AQUEDUCT_CISTERN_SIZE: [f32; 2] = [3.0, 3.0];

pub fn is_aqueduct_cistern(guid: u32) -> bool {
    AQUEDUCT_CISTERN_GUIDS.contains(&guid)
}

/// BuildBlocker min corner relative to pipette Pos for known decentered assets.
///
/// For cisterns the offset is `mx = 1 - w/2` (= −0.5 at w=3): one tile left of center.
pub fn known_blocker_min(guid: u32, w: f32, h: f32) -> Option<(f32, f32)> {
    let _ = (w, h);
    if is_aqueduct_cistern(guid) {
        Some((
            1.0 - AQUEDUCT_CISTERN_SIZE[0] * 0.5,
            -AQUEDUCT_CISTERN_SIZE[1] * 0.5,
        ))
    } else {
        None
    }
}

/// Resolve BuildBlocker min corner `(mx, my)` for footprint ↔ Pos conversion.
///
/// Aqueduct cistern GUIDs always use [`known_blocker_min`] — catalog `blocker_min`
/// must not override (a centered `[-1.5,-1.5]` from a bad extract would re-break
/// the +1 gap).
pub fn blocker_min_for(
    guid: u32,
    w: f32,
    h: f32,
    def: Option<&BuildingDef>,
    variation: Option<u32>,
) -> (f32, f32) {
    if let Some(m) = known_blocker_min(guid, w, h) {
        return m;
    }
    if let Some(d) = def {
        if let Some(m) = d.blocker_min {
            return (m[0], m[1]);
        }
        let s = d.size_for_variation(variation);
        return (-s[0] * 0.5, -s[1] * 0.5);
    }
    (-w * 0.5, -h * 0.5)
}

/// Footprint size for placement / coverage. Cisterns are forced to 3×3.
pub fn size_for_guid(
    guid: u32,
    w: f32,
    h: f32,
    def: Option<&BuildingDef>,
    variation: Option<u32>,
) -> (f32, f32) {
    if is_aqueduct_cistern(guid) {
        return (AQUEDUCT_CISTERN_SIZE[0], AQUEDUCT_CISTERN_SIZE[1]);
    }
    if let Some(d) = def {
        let s = d.size_for_variation(variation);
        return (s[0].max(1.0), s[1].max(1.0));
    }
    (w.max(1.0), h.max(1.0))
}

/// Rotate BuildBlocker min + size for pipette Dir (0/2/4/6). Diagonals keep catalog order.
pub fn oriented_blocker(
    mx: f32,
    my: f32,
    w: f32,
    h: f32,
    direction: i32,
) -> (f32, f32, f32, f32) {
    match direction.rem_euclid(8) {
        2 => (-(my + h), mx, h, w),
        4 => (-(mx + w), -(my + h), w, h),
        6 => (my, -(mx + w), h, w),
        _ => (mx, my, w, h),
    }
}

#[cfg(test)]
mod blocker_offset_tests {
    use super::*;

    #[test]
    fn cistern_blocker_min_is_decentered_on_x() {
        let (mx, my) = known_blocker_min(19753, 3.0, 3.0).expect("latium");
        assert!((mx - (-0.5)).abs() < 1e-5);
        assert!((my - (-1.5)).abs() < 1e-5);
        assert!(known_blocker_min(3310, 6.0, 6.0).is_none());
    }

    #[test]
    fn place_pos_from_flush_tl_is_not_geometric_center() {
        let (mx, my) = blocker_min_for(19753, 3.0, 3.0, None, None);
        let pos_x = 1.0_f32 - mx; // TL x=1 (road at 0)
        let pos_y = 0.0_f32 - my;
        assert!(
            (pos_x - 1.5).abs() < 1e-5,
            "want Pos.x=1.5 (not centered 2.5), got {pos_x}"
        );
        assert!((pos_y - 1.5).abs() < 1e-5);
    }

    #[test]
    fn cistern_ignores_catalog_centered_blocker_min_and_wrong_size() {
        // Bad extract: centered mins + AABB size 5 must not reintroduce the +1 gap.
        let bad = BuildingDef {
            guid: 19753,
            name: "Aqueduct Cistern".into(),
            short_name: None,
            template: "AqueductDistributor".into(),
            category: "other".into(),
            size: [5.0, 5.0],
            size_marsh: None,
            blocker_min: Some([-2.5, -2.5]),
            outputs: vec![],
            inputs: vec![],
            cycle_ms: None,
            influence_radius: None,
            needed_area: None,
            module_limit: None,
            farm_type: None,
            radius_distance: None,
            street_distance: None,
            effect_scope: None,
            effect_bonuses: vec![],
            service_kind: None,
            road_kind: None,
            needs_fuel: false,
            logistic_speed_factor: None,
            tileset_folder: None,
            constructable: Some(true),
            regions: vec![],
        };
        let (mx, my) = blocker_min_for(19753, 5.0, 5.0, Some(&bad), None);
        assert!((mx - (-0.5)).abs() < 1e-5, "mx={mx}");
        assert!((my - (-1.5)).abs() < 1e-5, "my={my}");
        let (w, h) = size_for_guid(19753, 5.0, 5.0, Some(&bad), None);
        assert!((w - 3.0).abs() < 1e-5 && (h - 3.0).abs() < 1e-5);
        let pos_x = 1.0 - mx;
        assert!(
            (pos_x - 1.5).abs() < 1e-5,
            "flush TL=1 must encode Pos.x=1.5, got {pos_x}"
        );
    }
}

impl BuildingDef {
    /// Footprint for a pipette Variation index.
    /// For Warehouse Roman Celtic (7055+), Variation `1` selects the marsh CFG (5×5);
    /// missing/`0` keeps the land CFG (6×6). Region alone must not force marsh —
    /// Albion land stamps (e.g. Drinking Horns x4) share GUID 7055 without Variation.
    pub fn size_for_variation(&self, variation: Option<u32>) -> [f32; 2] {
        if matches!(variation, Some(v) if v >= 1) {
            if let Some(m) = self.size_marsh {
                return m;
            }
        }
        self.size
    }

    /// Deprecated region heuristic — kept for call-site migration; ignores region.
    pub fn size_for_region(&self, _region_guid: u32) -> [f32; 2] {
        self.size
    }
}

/// Palette region filter. Albion≈Celtic, Latium≈Roman, Delta≈Egyptian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegionFilter {
    #[default]
    All,
    Albion,
    Celtic,
    Latium,
    Delta,
}

impl RegionFilter {
    pub const ALL: [RegionFilter; 5] = [
        RegionFilter::All,
        RegionFilter::Albion,
        RegionFilter::Celtic,
        RegionFilter::Latium,
        RegionFilter::Delta,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RegionFilter::All => "Alle",
            RegionFilter::Albion => "Albion",
            RegionFilter::Celtic => "Celtic",
            RegionFilter::Latium => "Latium",
            RegionFilter::Delta => "Delta (Ägypten)",
        }
    }

    /// Stamp RegionGUID → filter (6626 Celtic/Albion, 3225 Roman/Latium, 149678 Delta).
    pub fn from_region_guid(guid: u32) -> Self {
        match guid {
            6626 => RegionFilter::Albion,
            3225 => RegionFilter::Latium,
            149678 => RegionFilter::Delta,
            _ => RegionFilter::All,
        }
    }

    /// RegionGUID written into new stamp packs for the active filter.
    pub fn stamp_region_guid(self) -> u32 {
        match self {
            RegionFilter::Albion | RegionFilter::Celtic | RegionFilter::All => 6626,
            RegionFilter::Latium => 3225,
            RegionFilter::Delta => 149678,
        }
    }

    /// Whether a stamp (by RegionGUID / folder hint) belongs to this filter.
    pub fn matches_stamp(self, region_guid: u32, folder_name: &str) -> bool {
        if self == RegionFilter::All {
            return true;
        }
        let folder = folder_name.to_ascii_lowercase();
        match self {
            RegionFilter::Albion | RegionFilter::Celtic => {
                region_guid == 6626
                    || folder.contains("albion")
                    || folder.contains("celtic")
            }
            RegionFilter::Latium => {
                region_guid == 3225
                    || folder.contains("latium")
                    || folder.contains("roman")
            }
            RegionFilter::Delta => {
                region_guid == 149678
                    || folder.contains("delta")
                    || folder.contains("egypt")
                    || folder.contains("egyptian")
            }
            RegionFilter::All => true,
        }
    }

    fn matches_asset_region(self, region: &str) -> bool {
        let r = region.to_ascii_lowercase();
        match self {
            RegionFilter::All => true,
            RegionFilter::Albion | RegionFilter::Celtic => r == "celtic",
            RegionFilter::Latium => r == "roman",
            RegionFilter::Delta => r == "egyptian" || r == "delta",
        }
    }

    pub fn matches_building(self, b: &BuildingDef) -> bool {
        if self == RegionFilter::All {
            return true;
        }
        if b.regions.iter().any(|r| self.matches_asset_region(r)) {
            return true;
        }
        // Streets / universal assets without region: keep visible in every filter
        // only if they look region-neutral; otherwise hide.
        if b.regions.is_empty() {
            let n = b.name.to_ascii_lowercase();
            return match self {
                RegionFilter::Albion | RegionFilter::Celtic => {
                    n.contains("celtic") || n.contains("albion") || n.contains("marsh")
                }
                RegionFilter::Latium => n.contains("roman") || n.contains("latium"),
                RegionFilter::Delta => {
                    n.contains("egyptian") || n.contains("egypt") || n.contains("delta")
                }
                RegionFilter::All => true,
            };
        }
        false
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProductDef {
    pub guid: u32,
    pub name: String,
    #[serde(default)]
    pub template: String,
}

#[derive(Debug, Clone, Deserialize)]
struct CatalogFile {
    buildings: HashMap<String, BuildingDef>,
    products: HashMap<String, ProductDef>,
}

#[derive(Debug, Clone, Default)]
pub struct Catalog {
    pub buildings: HashMap<u32, BuildingDef>,
    pub products: HashMap<u32, ProductDef>,
}

impl Catalog {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let raw: CatalogFile = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let mut buildings = HashMap::new();
        for (k, v) in raw.buildings {
            let guid = k.parse::<u32>().unwrap_or(v.guid);
            buildings.insert(guid, v);
        }
        let mut products = HashMap::new();
        for (k, v) in raw.products {
            let guid = k.parse::<u32>().unwrap_or(v.guid);
            products.insert(guid, v);
        }
        Ok(Self {
            buildings,
            products,
        })
    }

    pub fn name(&self, guid: u32) -> String {
        if let Some(b) = self.buildings.get(&guid) {
            return b.name.clone();
        }
        if let Some(p) = self.products.get(&guid) {
            return p.name.clone();
        }
        format!("GUID {guid}")
    }

    pub fn label(&self, guid: u32) -> String {
        if let Some(b) = self.buildings.get(&guid) {
            if let Some(s) = b.short_name.as_ref().filter(|s| !s.is_empty()) {
                return s.clone();
            }
            return short_label(&b.name);
        }
        self.name(guid)
    }

    pub fn building(&self, guid: u32) -> Option<&BuildingDef> {
        self.buildings.get(&guid)
    }

    pub fn buildings_iter(&self) -> impl Iterator<Item = &BuildingDef> {
        self.buildings.values()
    }

    pub fn palette(&self, filter: &str, region: RegionFilter) -> Vec<&BuildingDef> {
        let f = filter.to_lowercase();
        let mut items: Vec<&BuildingDef> = self
            .buildings
            .values()
            .filter(|b| {
                matches!(
                    b.category.as_str(),
                    "production" | "warehouse" | "module" | "institution" | "street"
                )
            })
            .filter(|b| !is_palette_noise(b))
            .filter(|b| region.matches_building(b))
            .filter(|b| {
                f.is_empty()
                    || b.name.to_lowercase().contains(&f)
                    || b.short_name
                        .as_ref()
                        .map(|s| s.to_lowercase().contains(&f))
                        .unwrap_or(false)
                    || b.guid.to_string().contains(&f)
            })
            .collect();
        items.sort_by(|a, b| {
            let la = a.short_name.as_deref().unwrap_or(a.name.as_str());
            let lb = b.short_name.as_deref().unwrap_or(b.name.as_str());
            la.cmp(lb)
        });
        items
    }
}

fn is_palette_noise(b: &BuildingDef) -> bool {
    let n = b.name.to_lowercase();
    let t = b.template.to_lowercase();
    let noise = [
        "buff ",
        "asset pool",
        "audiopool",
        "pamsy",
        "bubble:",
        "old_",
        "effect ",
        "blockproduction",
        "keep state checker",
        "governordecision",
        "supplied pamsy",
        "movein ",
        "fire audiopool",
        "riot audiopool",
        "illness audiopool",
        "nostreetconnection",
        "onboarding",
        "shipmodule",
        "settings streets",
        "pavement_set",
        "targets",
    ];
    if noise.iter().any(|k| n.contains(k) || t.contains(k)) {
        return true;
    }
    if t.contains("ship") {
        return true;
    }
    // Prefer constructable production when flag present; keep streets/modules.
    if matches!(b.category.as_str(), "production" | "warehouse" | "institution")
        && b.constructable == Some(false)
        && b.outputs.is_empty()
    {
        return true;
    }
    false
}

/// Strip common Anno asset-name prefixes for canvas labels.
pub fn short_label(name: &str) -> String {
    let mut s = name.trim().to_string();
    let prefixes = [
        "Production Construction ",
        "Production Mountain ",
        "Production Marsh ",
        "Production Field ",
        "Production ",
        "Module Marsh Pasture ",
        "Module Marsh Field ",
        "Module Marsh ",
        "Module Polygon ",
        "Module ",
        "Warehouse ",
        "Street ",
        "Institution ",
        "Ornament ",
        "HoF Ornament ",
    ];
    for p in prefixes {
        if let Some(rest) = s.strip_prefix(p) {
            s = rest.to_string();
            break;
        }
    }
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

