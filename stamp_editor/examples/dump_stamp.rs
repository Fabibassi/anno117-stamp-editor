use stamp_editor::catalog::Catalog;
use stamp_editor::coverage::{compute_roads_only, footprint_cells, RoadKind};
use stamp_editor::meshgraph::{mesh_road_cells, mesh_road_segments};
use stamp_editor::pack::load_pack;
use stamp_editor::stamp::decode_pipette_b64;
use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = env::args().collect();
    let pack = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\Albion\Albion Stamp Pack.json")
        });
    let want = args.get(2).map(|s| s.as_str()).unwrap_or("Chariots x4");
    let catalog_path = PathBuf::from(
        r"C:\Users\fabi_\Projects\anno117-stamp-editor\_gamedata\editor_catalog.json",
    );
    let catalog = Catalog::load(&catalog_path).expect("catalog");
    let pack_data = load_pack(&pack).expect("pack");
    let entry = pack_data
        .stamps
        .iter()
        .find(|s| s.name.eq_ignore_ascii_case(want))
        .unwrap_or_else(|| panic!("stamp not found: {want}"));
    let stamp = decode_pipette_b64(&entry.pipette_data).expect("decode");
    println!(
        "stamp={} buildings={} polygon_modules={} polygon_cells={}",
        entry.name,
        stamp.buildings.len(),
        stamp.polygon_modules.len(),
        stamp.polygon_cells.len()
    );
    let mut all_buildings = stamp.buildings.clone();
    all_buildings.extend(stamp.polygon_modules.iter().cloned());

    let cov = compute_roads_only(&stamp.buildings, &catalog, Some(&stamp.document));
    println!(
        "roads={} dirt={} stone={}",
        cov.roads.len(),
        cov.roads
            .iter()
            .filter(|(_, k)| matches!(k, RoadKind::Dirt))
            .count(),
        cov.roads
            .iter()
            .filter(|(_, k)| matches!(k, RoadKind::Stone))
            .count()
    );

    let mut size_counts: BTreeMap<(i32, i32), usize> = BTreeMap::new();
    for b in &all_buildings {
        let def = catalog.building(b.guid);
        let (w, h) = def
            .map(|d| (d.size[0], d.size[1]))
            .unwrap_or((2.0, 2.0));
        *size_counts.entry((w as i32, h as i32)).or_default() += 1;
        let label = catalog.label(b.guid, stamp_editor::i18n::Language::En);
        let cells = footprint_cells(b, def);
        let minx = cells.iter().map(|c| c.x).min().unwrap_or(0);
        let maxx = cells.iter().map(|c| c.x).max().unwrap_or(0);
        let miny = cells.iter().map(|c| c.y).min().unwrap_or(0);
        let maxy = cells.iter().map(|c| c.y).max().unwrap_or(0);
        println!(
            "guid={:<6} pos=({:>7.2},{:>7.2}) dir={} size={:.0}x{:.0} cells={}..{} x {}..{}  {}",
            b.guid, b.x, b.y, b.direction, w, h, minx, maxx, miny, maxy, label
        );
    }
    println!("size histogram: {size_counts:?}");

    let cells = mesh_road_cells(&stamp.document);
    if !cells.is_empty() {
        let xs: Vec<_> = cells.iter().map(|c| c.0).collect();
        let ys: Vec<_> = cells.iter().map(|c| c.1).collect();
        println!(
            "mesh cells bbox x={}..{} y={}..{} n={}",
            xs.iter().min().unwrap(),
            xs.iter().max().unwrap(),
            ys.iter().min().unwrap(),
            ys.iter().max().unwrap(),
            cells.len()
        );
    }
    for (x0, y0, x1, y1, g) in mesh_road_segments(&stamp.document).into_iter().take(20) {
        println!("seg guid={g} ({x0:.2},{y0:.2})->({x1:.2},{y1:.2})");
    }

    // Occupancy grid: building footprints vs roads vs empty inside AABB
    let mut occupied = std::collections::BTreeSet::new();
    for b in &all_buildings {
        for c in footprint_cells(b, catalog.building(b.guid)) {
            occupied.insert((c.x, c.y));
        }
    }
    for c in &stamp.polygon_cells {
        occupied.insert((c.x, c.y));
    }
    let road_set: std::collections::BTreeSet<_> =
        cov.roads.iter().map(|(c, _)| (c.x, c.y)).collect();
    let all_x = occupied
        .iter()
        .chain(road_set.iter())
        .map(|c| c.0)
        .collect::<Vec<_>>();
    let all_y = occupied
        .iter()
        .chain(road_set.iter())
        .map(|c| c.1)
        .collect::<Vec<_>>();
    if all_x.is_empty() {
        return;
    }
    let minx = *all_x.iter().min().unwrap();
    let maxx = *all_x.iter().max().unwrap();
    let miny = *all_y.iter().min().unwrap();
    let maxy = *all_y.iter().max().unwrap();
    let mut empty = 0;
    let mut both = 0;
    for y in miny..=maxy {
        let mut row = String::new();
        for x in minx..=maxx {
            let o = occupied.contains(&(x, y));
            let r = road_set.contains(&(x, y));
            let ch = match (o, r) {
                (true, true) => {
                    both += 1;
                    'X'
                }
                (true, false) => '#',
                (false, true) => '=',
                (false, false) => {
                    empty += 1;
                    '.'
                }
            };
            row.push(ch);
        }
        println!("{row}");
    }
    println!(
        "bbox={}x{} occupied={} roads={} overlap={} empty_in_bbox={}",
        maxx - minx + 1,
        maxy - miny + 1,
        occupied.len(),
        road_set.len(),
        both,
        empty
    );
}

