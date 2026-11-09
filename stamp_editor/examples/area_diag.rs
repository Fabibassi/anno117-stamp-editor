fn main() {
    use stamp_editor::area_efficiency::analyze_area_efficiency;
    use stamp_editor::catalog::Catalog;
    use stamp_editor::coverage::build_road_map_region;
    use stamp_editor::pack::open_named_stamp;
    use std::path::PathBuf;

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cat = Catalog::load(&root.join("../_gamedata/editor_catalog.json")).unwrap();
    for (pack, name) in [
        ("testdata/lumber x4.json", "Lumber x4"),
        ("testdata/woodcutter.json", "Woodcutter"),
    ] {
        let p = root.join(pack);
        if !p.exists() {
            // list stamp names
            println!("missing {pack}");
            continue;
        }
        let open = match open_named_stamp(&p, name) {
            Ok(o) => o,
            Err(e) => {
                // try first stamp
                println!("{pack} open {name:?} err {e}; listing...");
                continue;
            }
        };
        let roads = build_road_map_region(&open.stamp.buildings, &cat, Some(&open.stamp.document), open.region_guid);
        let areas = analyze_area_efficiency(&open.stamp.buildings, &cat, Some(&open.stamp.document), open.region_guid);
        println!("=== {} buildings={} roads={} areas={} ===", name, open.stamp.buildings.len(), roads.len(), areas.len());
        for a in &areas {
            println!("  [{}] {} have={:.0} need={:.0} eff={:.0}% r={:.0}", a.building_index, a.label, a.have, a.need, a.efficiency*100.0, a.radius);
        }
    }
}
