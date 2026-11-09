//! Catalog wiring for Förster / Marsh / Embankment / Meadow area buildings.

use stamp_editor::area_efficiency::{
    analyze_area_efficiency, is_resource_area_building, resource_need, resource_radius,
};
use stamp_editor::catalog::Catalog;
use stamp_editor::modules::{needed_modules, uses_area_progress};
use stamp_editor::stamp::Building;
use std::path::PathBuf;

fn catalog() -> Catalog {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Catalog::load(&root.join("../_gamedata/editor_catalog.json")).expect("catalog")
}

#[test]
fn catalog_covers_forest_marsh_meadow_embankment() {
    let cat = catalog();
    let cases: &[(u32, &str, f32)] = &[
        (2878, "Production Area", 80.0),                  // Roman Wood
        (2880, "Production Area", 80.0),                  // Roman Charcoal
        (2795, "Production Area", 80.0),                  // Roman Honeycombs
        (5851, "Production Marsh Area", 180.0),           // Birds
        (152000, "ProductionEmbankmentArea", 100.0),      // Hippos
    ];
    for &(guid, tmpl, need) in cases {
        let def = cat.building(guid).unwrap_or_else(|| panic!("missing {guid}"));
        assert_eq!(def.template, tmpl, "guid {guid}");
        assert!(is_resource_area_building(def), "guid {guid}");
        assert_eq!(resource_need(def), Some(need));
        assert!(resource_radius(def).unwrap() > 0.0);
        assert_eq!(needed_modules(Some(def)), 0);
        assert!(!uses_area_progress(Some(def)));
    }
}

#[test]
fn resin_without_catalog_radius_still_gets_fallback() {
    let cat = catalog();
    let def = cat.building(31752).expect("roman resin");
    assert!(is_resource_area_building(def));
    assert!(def.influence_radius.is_none());
    assert_eq!(resource_need(def), Some(45.0));
    assert!(resource_radius(def).unwrap() > 0.0);
}

#[test]
fn woodcutter_chain_scales_with_packing() {
    let cat = catalog();
    let lone = vec![Building {
        guid: 2878,
        x: 40.0,
        y: 40.0,
        direction: 0,
        variation: None,
        complex_owner_id: None,
    }];
    let lone_eff = analyze_area_efficiency(&lone, &cat, None, 0)[0].efficiency;
    assert!((lone_eff - 1.0).abs() < 1e-3);

    let mut packed = Vec::new();
    for iy in 0..4 {
        for ix in 0..4 {
            packed.push(Building {
                guid: 2878,
                x: 20.0 + ix as f32 * 2.0,
                y: 20.0 + iy as f32 * 2.0,
                direction: 0,
                variation: None,
                complex_owner_id: None,
            });
        }
    }
    let areas = analyze_area_efficiency(&packed, &cat, None, 0);
    assert_eq!(areas.len(), 16);
    let avg: f32 = areas.iter().map(|a| a.efficiency).sum::<f32>() / areas.len() as f32;
    let lone_have = analyze_area_efficiency(&lone, &cat, None, 0)[0].have;
    let packed_have_avg: f32 = areas.iter().map(|a| a.have).sum::<f32>() / areas.len() as f32;
    // Same-class overlap competes (1/N) — dense 4×4 packing must tank efficiency hard.
    assert!(
        packed_have_avg < lone_have * 0.55,
        "packed avg have={packed_have_avg} lone={lone_have} avg_eff={avg}"
    );
    assert!(
        avg < 0.85,
        "dense packing should leave most woodcutters under 85% (avg={avg})"
    );
    assert!(areas.iter().all(|a| a.contested_tiles > 0.0));
}
