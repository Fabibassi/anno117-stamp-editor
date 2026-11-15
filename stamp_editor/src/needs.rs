//! Residence service-need matching against coverage `ProvidedNeed` hits.

use crate::catalog::{BuildingDef, Catalog, ResidenceNeed};
use crate::coverage::{CoverageMap, EffectHit};
use crate::i18n::Language;
use crate::stamp::Building;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct NeedStatus {
    pub need: ResidenceNeed,
    pub met: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ResidenceNeedReport {
    pub services: Vec<NeedStatus>,
    pub goods: Vec<ResidenceNeed>,
}

impl ResidenceNeedReport {
    pub fn met_count(&self) -> usize {
        self.services.iter().filter(|s| s.met).count()
    }

    pub fn service_count(&self) -> usize {
        self.services.len()
    }

    pub fn summary_badge(&self) -> String {
        format!("{}/{}", self.met_count(), self.service_count())
    }
}

/// Collect Need GUIDs provided by effect hits (`ProvidedNeed` values).
pub fn provided_need_guids(hits: &[EffectHit]) -> HashSet<u32> {
    let mut out = HashSet::new();
    for h in hits {
        for b in &h.bonuses {
            if b.attr == "ProvidedNeed" {
                let g = b.value.round() as u32;
                if g > 0 {
                    out.insert(g);
                }
            }
        }
    }
    out
}

pub fn report_for_def(def: &BuildingDef, provided: &HashSet<u32>) -> ResidenceNeedReport {
    let mut services = Vec::new();
    let mut goods = Vec::new();
    for n in &def.residence_needs {
        if n.buff_only {
            continue;
        }
        if n.is_service() {
            services.push(NeedStatus {
                met: provided.contains(&n.need_guid),
                need: n.clone(),
            });
        } else {
            goods.push(n.clone());
        }
    }
    ResidenceNeedReport { services, goods }
}

pub fn report_for_building(
    catalog: &Catalog,
    buildings: &[Building],
    index: usize,
    coverage: &CoverageMap,
) -> Option<ResidenceNeedReport> {
    let b = buildings.get(index)?;
    let def = catalog.building(b.guid)?;
    if !def.is_residence() {
        return None;
    }
    let provided = provided_need_guids(&coverage.hits);
    Some(report_for_def(def, &provided))
}

/// City-wide goods/food consumption estimate: sum of rates × house count per need.
#[derive(Debug, Clone)]
pub struct ConsumptionRow {
    pub need_guid: u32,
    pub label: String,
    pub houses: usize,
    pub rate_sum: f32,
    pub icon: Option<String>,
}

/// How many residences have each service need unmet.
#[derive(Debug, Clone)]
pub struct ServiceCoverageRow {
    pub need_guid: u32,
    pub label: String,
    pub houses_needing: usize,
    pub houses_met: usize,
    pub icon: Option<String>,
}

pub fn analyse_city_needs(
    catalog: &Catalog,
    buildings: &[Building],
    // building_index → incoming coverage (only residences need real data)
    incoming_by_index: &HashMap<usize, CoverageMap>,
    lang: Language,
) -> (Vec<ConsumptionRow>, Vec<ServiceCoverageRow>) {
    let mut goods_acc: HashMap<u32, (usize, f32, Option<String>, String)> = HashMap::new();
    let mut svc_acc: HashMap<u32, (usize, usize, Option<String>, String)> = HashMap::new();

    for (idx, b) in buildings.iter().enumerate() {
        let Some(def) = catalog.building(b.guid) else {
            continue;
        };
        if !def.is_residence() {
            continue;
        }
        let provided = incoming_by_index
            .get(&idx)
            .map(|c| provided_need_guids(&c.hits))
            .unwrap_or_default();

        for n in &def.residence_needs {
            if n.buff_only {
                continue;
            }
            let label = n.label(lang);
            if n.is_service() {
                let e = svc_acc.entry(n.need_guid).or_insert((
                    0,
                    0,
                    n.icon.clone(),
                    label.clone(),
                ));
                e.0 += 1;
                if provided.contains(&n.need_guid) {
                    e.1 += 1;
                }
            } else {
                let rate = n.consumption_rate.unwrap_or(0.0);
                let e = goods_acc.entry(n.need_guid).or_insert((
                    0,
                    0.0,
                    n.icon.clone(),
                    label.clone(),
                ));
                e.0 += 1;
                e.1 += rate;
            }
        }
    }

    let mut consumption: Vec<ConsumptionRow> = goods_acc
        .into_iter()
        .map(|(need_guid, (houses, rate_sum, icon, label))| ConsumptionRow {
            need_guid,
            label,
            houses,
            rate_sum,
            icon,
        })
        .collect();
    consumption.sort_by(|a, b| a.label.cmp(&b.label));

    let mut services: Vec<ServiceCoverageRow> = svc_acc
        .into_iter()
        .map(|(need_guid, (houses_needing, houses_met, icon, label))| ServiceCoverageRow {
            need_guid,
            label,
            houses_needing,
            houses_met,
            icon,
        })
        .collect();
    services.sort_by(|a, b| a.label.cmp(&b.label));

    (consumption, services)
}
