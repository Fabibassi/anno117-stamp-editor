//! Production chain balance: resources in / out per minute.

use crate::area_efficiency::{analyze_area_efficiency_with_fields, efficiency_by_index};
use crate::catalog::Catalog;
use crate::filedb::DocumentOrdered;
use crate::i18n::Language;
use crate::stamp::{Building, PolygonCell};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct ProductFlow {
    pub product: u32,
    pub name: String,
    pub produced_per_min: f32,
    pub consumed_per_min: f32,
}

impl ProductFlow {
    pub fn net(&self) -> f32 {
        self.produced_per_min - self.consumed_per_min
    }

    pub fn balanced(&self) -> bool {
        // Allow small float noise; deficit is unbalanced.
        self.net() >= -0.001
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChainReport {
    pub flows: Vec<ProductFlow>,
    pub producers: usize,
    pub all_balanced: bool,
}

/// CycleTime in assets is often seconds (e.g. 60). Treat values <= 1000 as seconds,
/// larger values as milliseconds.
fn cycles_per_minute(cycle: f32) -> f32 {
    if cycle <= 0.0 {
        return 0.0;
    }
    let seconds = if cycle > 1000.0 { cycle / 1000.0 } else { cycle };
    60.0 / seconds
}

pub fn analyze_chain(buildings: &[Building], catalog: &Catalog) -> ChainReport {
    analyze_chain_region(buildings, catalog, None, 0, &[], Language::De)
}

/// Like [`analyze_chain`], but scales Production-Area I/O by dense-resource efficiency.
pub fn analyze_chain_region(
    buildings: &[Building],
    catalog: &Catalog,
    document: Option<&DocumentOrdered>,
    region_guid: u32,
    polygon_cells: &[PolygonCell],
    lang: Language,
) -> ChainReport {
    let areas = analyze_area_efficiency_with_fields(
        buildings,
        catalog,
        document,
        region_guid,
        polygon_cells,
        lang,
    );
    let eff_map = efficiency_by_index(&areas);

    let mut produced: BTreeMap<u32, f32> = BTreeMap::new();
    let mut consumed: BTreeMap<u32, f32> = BTreeMap::new();
    let mut producers = 0usize;

    for (i, b) in buildings.iter().enumerate() {
        let Some(def) = catalog.building(b.guid) else {
            continue;
        };
        if def.outputs.is_empty() && def.inputs.is_empty() {
            continue;
        }
        producers += 1;
        let cpm = def.cycle_ms.map(cycles_per_minute).unwrap_or(1.0); // fallback: 1 cycle / min
        let eff = eff_map.get(&i).copied().unwrap_or(1.0);
        for o in &def.outputs {
            *produced.entry(o.product).or_default() += o.amount * cpm * eff;
        }
        for inp in &def.inputs {
            *consumed.entry(inp.product).or_default() += inp.amount * cpm * eff;
        }
        // NeedsFuelInput buildings consume charcoal/fuel each cycle (GUID 2085).
        if def.needs_fuel {
            const FUEL_PRODUCT: u32 = 2085;
            *consumed.entry(FUEL_PRODUCT).or_default() += 1.0 * cpm * eff;
        }
    }

    let mut keys: BTreeMap<u32, ()> = BTreeMap::new();
    for k in produced.keys().chain(consumed.keys()) {
        keys.insert(*k, ());
    }

    let mut flows = Vec::new();
    let mut all_balanced = true;
    for product in keys.keys() {
        let flow = ProductFlow {
            product: *product,
            name: catalog.label(*product, lang),
            produced_per_min: *produced.get(product).unwrap_or(&0.0),
            consumed_per_min: *consumed.get(product).unwrap_or(&0.0),
        };
        // Only require balance for products that are consumed somewhere.
        if flow.consumed_per_min > 0.0 && !flow.balanced() {
            all_balanced = false;
        }
        flows.push(flow);
    }
    flows.sort_by(|a, b| a.name.cmp(&b.name));

    ChainReport {
        flows,
        producers,
        all_balanced,
    }
}
