//! MeshGraph / Polygons sync for stamp layout edits.

use crate::filedb::{Attrib, DocumentOrdered, Node, Tag};
use crate::stamp::Building;

fn find_tag<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Tag> {
    nodes.iter().find_map(|n| match n {
        Node::Tag(t) if t.name == name => Some(t),
        _ => None,
    })
}

fn find_tag_mut<'a>(nodes: &'a mut [Node], name: &str) -> Option<&'a mut Tag> {
    nodes.iter_mut().find_map(|n| match n {
        Node::Tag(t) if t.name == name => Some(t),
        _ => None,
    })
}

fn find_attrib<'a>(nodes: &'a [Node], name: &str) -> Option<&'a Attrib> {
    nodes.iter().find_map(|n| match n {
        Node::Attrib(a) if a.name == name => Some(a),
        _ => None,
    })
}

fn find_attrib_mut<'a>(nodes: &'a mut [Node], name: &str) -> Option<&'a mut Attrib> {
    nodes.iter_mut().find_map(|n| match n {
        Node::Attrib(a) if a.name == name => Some(a),
        _ => None,
    })
}

fn i32_le(data: &[u8]) -> i32 {
    if data.len() < 4 {
        0
    } else {
        i32::from_le_bytes(data[0..4].try_into().unwrap())
    }
}

fn f32_pair(data: &[u8]) -> Option<(f32, f32)> {
    if data.len() < 8 {
        None
    } else {
        Some((
            f32::from_le_bytes(data[0..4].try_into().unwrap()),
            f32::from_le_bytes(data[4..8].try_into().unwrap()),
        ))
    }
}

fn write_f32_pair(a: &mut Attrib, x: f32, y: f32) {
    let mut bytes = vec![0u8; a.content.len().max(8)];
    bytes[0..4].copy_from_slice(&x.to_le_bytes());
    bytes[4..8].copy_from_slice(&y.to_le_bytes());
    a.content = bytes;
}

fn write_i32_pair(a: &mut Attrib, x: i32, y: i32) {
    let mut bytes = vec![0u8; a.content.len().max(8)];
    bytes[0..4].copy_from_slice(&x.to_le_bytes());
    bytes[4..8].copy_from_slice(&y.to_le_bytes());
    a.content = bytes;
}

fn write_i32_quad(a: &mut Attrib, v: [i32; 4]) {
    let mut bytes = vec![0u8; 16];
    for (i, n) in v.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&n.to_le_bytes());
    }
    a.content = bytes;
}

#[derive(Debug, Clone, Copy)]
pub struct MeshEdge {
    pub guid: i32,
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

#[derive(Debug, Clone)]
pub struct MeshInfo {
    pub drag_center: (f32, f32),
    pub aabb: [i32; 4],
    pub tile_count: u32,
    pub edges: Vec<MeshEdge>,
}

/// Stamp-relative float → stamp-relative integer tile, via absolute space.
///
/// Building `Pos`, mesh edges and SubTileGrid are expressed as
/// `absolute - DragRectCenter`. Absolute Anno tiles are integer-indexed. When
/// `DragRectCenter` has a fractional part (often `.5`), absolute tile edges
/// become half-integers in relative space. Naïve `floor(relative)` then picks
/// the neighboring tile — a systematic 1-cell shift (buildings onto roads).
///
/// Correct snap: `floor(rel + drag) - floor(drag)`. Equals `floor(rel)` when
/// `drag` is integer; must not be replaced by a blanket `ceil(rel)`.
pub fn relative_to_tile(rel: f32, drag: f32) -> i32 {
    (rel + drag).floor() as i32 - drag.floor() as i32
}

/// MeshGraph stores edge endpoints in subtile space ≈ 2 × tile coords,
/// while DragRectCenter / AABB use tile coords. Buildings use float offsets
/// relative to DragRectCenter.
pub fn read_mesh_info(doc: &DocumentOrdered) -> Option<MeshInfo> {
    let root = doc.roots.iter().find_map(|n| match n {
        Node::Tag(t) if t.name == "PipetteData" => Some(t),
        _ => None,
    })?;
    let mesh = find_tag(&root.children, "MeshGraphData")?;
    let drag = find_attrib(&mesh.children, "DragRectCenter")
        .and_then(|a| f32_pair(&a.content))
        .or_else(|| {
            find_attrib(&root.children, "DragRectCenter").and_then(|a| f32_pair(&a.content))
        })
        .unwrap_or((0.0, 0.0));
    let aabb = find_attrib(&mesh.children, "AABB")
        .map(|a| {
            if a.content.len() >= 16 {
                [
                    i32_le(&a.content[0..4]),
                    i32_le(&a.content[4..8]),
                    i32_le(&a.content[8..12]),
                    i32_le(&a.content[12..16]),
                ]
            } else {
                [0, 0, 0, 0]
            }
        })
        .unwrap_or([0, 0, 0, 0]);
    let tile_count = find_attrib(&mesh.children, "TileCount")
        .map(|a| i32_le(&a.content) as u32)
        .unwrap_or(0);

    let mut edges = Vec::new();
    if let Some(edges_wrap) = find_tag(&mesh.children, "Edges") {
        // First nested Edges list with children is the actual edge array.
        for child in &edges_wrap.children {
            let Node::Tag(list) = child else { continue };
            if list.name != "Edges" || list.children.is_empty() {
                continue;
            }
            for edge in &list.children {
                let Node::Tag(e) = edge else { continue };
                if e.name != "None" {
                    continue;
                }
                let guid = find_attrib(&e.children, "guid")
                    .map(|a| i32_le(&a.content))
                    .unwrap_or(0);
                let Some(pmin) = find_attrib(&e.children, "PosMin").and_then(|a| {
                    if a.content.len() >= 8 {
                        Some((i32_le(&a.content[0..4]), i32_le(&a.content[4..8])))
                    } else {
                        None
                    }
                }) else {
                    continue;
                };
                let Some(pmax) = find_attrib(&e.children, "PosMax").and_then(|a| {
                    if a.content.len() >= 8 {
                        Some((i32_le(&a.content[0..4]), i32_le(&a.content[4..8])))
                    } else {
                        None
                    }
                }) else {
                    continue;
                };
                edges.push(MeshEdge {
                    guid,
                    x0: pmin.0,
                    y0: pmin.1,
                    x1: pmax.0,
                    y1: pmax.1,
                });
            }
            break;
        }
    }

    Some(MeshInfo {
        drag_center: drag,
        aabb,
        tile_count,
        edges,
    })
}

/// Road cells in stamp-relative float/tile space (compatible with building positions).
pub fn mesh_road_cells(doc: &DocumentOrdered) -> Vec<(i32, i32, i32)> {
    // returns (x,y,guid)
    let Some(info) = read_mesh_info(doc) else {
        return Vec::new();
    };
    let (cx, cy) = info.drag_center;
    let mut cells = Vec::new();
    for e in info.edges {
        // Subtile endpoints → tile floats relative to DragRectCenter.
        // Mesh nodes sit on half-integers (tile corners), e.g. ±6.5.
        // `floor` maps each edge onto the interior-adjacent tile so a frame
        // around N-wide buildings packs flush (no empty column from `round`).
        let x0 = (e.x0 as f32) * 0.5 - cx;
        let y0 = (e.y0 as f32) * 0.5 - cy;
        let x1 = (e.x1 as f32) * 0.5 - cx;
        let y1 = (e.y1 as f32) * 0.5 - cy;
        for (x, y) in rasterize_edge_f(x0, y0, x1, y1) {
            cells.push((x, y, e.guid));
        }
    }
    cells
}

/// Mesh edge endpoints in stamp-relative tile space (for line drawing).
pub fn mesh_road_segments(doc: &DocumentOrdered) -> Vec<(f32, f32, f32, f32, i32)> {
    let Some(info) = read_mesh_info(doc) else {
        return Vec::new();
    };
    let (cx, cy) = info.drag_center;
    info.edges
        .iter()
        .map(|e| {
            (
                e.x0 as f32 * 0.5 - cx,
                e.y0 as f32 * 0.5 - cy,
                e.x1 as f32 * 0.5 - cx,
                e.y1 as f32 * 0.5 - cy,
                e.guid,
            )
        })
        .collect()
}

/// Rasterize a mesh edge in tile-float space onto integer cells.
/// Axis-aligned half-integer edges use inclusive floor ranges; diagonals use Bresenham.
fn rasterize_edge_f(x0: f32, y0: f32, x1: f32, y1: f32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    const EPS: f32 = 1e-3;
    if (y0 - y1).abs() < EPS {
        let y = y0.floor() as i32;
        let a = x0.min(x1).floor() as i32;
        let b = x0.max(x1).floor() as i32;
        for x in a..=b {
            out.push((x, y));
        }
        return out;
    }
    if (x0 - x1).abs() < EPS {
        let x = x0.floor() as i32;
        let a = y0.min(y1).floor() as i32;
        let b = y0.max(y1).floor() as i32;
        for y in a..=b {
            out.push((x, y));
        }
        return out;
    }
    // Diagonal: Bresenham on floored endpoints (include start, exclude exact end).
    let ix0 = x0.floor() as i32;
    let iy0 = y0.floor() as i32;
    let ix1 = x1.floor() as i32;
    let iy1 = y1.floor() as i32;
    let mut x = ix0;
    let mut y = iy0;
    let dx = (ix1 - ix0).abs();
    let dy = -(iy1 - iy0).abs();
    let sx = if ix0 < ix1 { 1 } else { -1 };
    let sy = if iy0 < iy1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        if x == ix1 && y == iy1 {
            break;
        }
        out.push((x, y));
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
        if out.len() > 10_000 {
            break;
        }
    }
    if out.is_empty() {
        out.push((ix0, iy0));
    }
    out
}

fn pipette_mut(doc: &mut DocumentOrdered) -> Option<&mut Tag> {
    doc.roots.iter_mut().find_map(|n| match n {
        Node::Tag(t) if t.name == "PipetteData" => Some(t),
        _ => None,
    })
}

fn owner_id(data: &[u8]) -> Option<u64> {
    if data.len() >= 8 {
        Some(u64::from_le_bytes(data[0..8].try_into().unwrap()))
    } else if data.len() >= 4 {
        Some(u32::from_le_bytes(data[0..4].try_into().unwrap()) as u64)
    } else {
        None
    }
}

/// After a building moved by delta, shift polygons (and related module buildings)
/// that share the same ComplexOwnerID, then refresh AABB.
pub fn sync_after_building_move(
    doc: &mut DocumentOrdered,
    buildings: &mut [Building],
    moved_index: usize,
    old_x: f32,
    old_y: f32,
    new_x: f32,
    new_y: f32,
) {
    let dx = new_x - old_x;
    let dy = new_y - old_y;
    if dx == 0.0 && dy == 0.0 {
        return;
    }

    let owner = buildings
        .get(moved_index)
        .and_then(|b| b.complex_owner_id)
        .filter(|id| *id != 0);

    if let Some(root) = pipette_mut(doc) {
        // 1) Polygons linked by ComplexOwnerID (farm fields etc.)
        if let Some(polygons) = find_tag_mut(&mut root.children, "Polygons") {
            for child in &mut polygons.children {
                let Node::Tag(entry) = child else { continue };
                let matches = match owner {
                    Some(oid) => find_attrib(&entry.children, "ComplexOwnerID")
                        .and_then(|a| owner_id(&a.content))
                        == Some(oid),
                    None => {
                        // Fallback: polygon RelativePos near old building center.
                        find_attrib(&entry.children, "RelativePos")
                            .and_then(|a| f32_pair(&a.content))
                            .map(|(px, py)| (px - old_x).abs() < 0.51 && (py - old_y).abs() < 0.51)
                            .unwrap_or(false)
                    }
                };
                if !matches {
                    continue;
                }
                if let Some(rel) = find_attrib_mut(&mut entry.children, "RelativePos") {
                    if let Some((px, py)) = f32_pair(&rel.content) {
                        write_f32_pair(rel, px + dx, py + dy);
                    }
                }
                // SubTileGrid.GridOriginWS is absolute tile coords; shift with the farm
                // or field cells stay behind after a ComplexOwner group move.
                if let Some(stg) = find_tag_mut(&mut entry.children, "SubTileGrid") {
                    if let Some(origin) = find_attrib_mut(&mut stg.children, "GridOriginWS") {
                        if origin.content.len() >= 8 {
                            let ox = i32_le(&origin.content[0..4]);
                            let oy = i32_le(&origin.content[4..8]);
                            write_i32_pair(
                                origin,
                                ox + dx.round() as i32,
                                oy + dy.round() as i32,
                            );
                        }
                    }
                }
            }
        }

        // 2) Other BuildingInfo entries with same ComplexOwnerID (modules) — move with parent.
        if let Some(oid) = owner {
            if let Some(info) = find_tag_mut(&mut root.children, "BuildingInfo") {
                let mut idx = 0usize;
                for child in &mut info.children {
                    let Node::Tag(entry) = child else { continue };
                    if entry.name != "None" {
                        continue;
                    }
                    let same = find_attrib(&entry.children, "ComplexOwnerID")
                        .and_then(|a| owner_id(&a.content))
                        == Some(oid);
                    if same && idx != moved_index {
                        if let Some(pos) = find_attrib_mut(&mut entry.children, "Pos") {
                            if let Some((px, py)) = f32_pair(&pos.content) {
                                write_f32_pair(pos, px + dx, py + dy);
                            }
                        }
                        if let Some(b) = buildings.get_mut(idx) {
                            b.x += dx;
                            b.y += dy;
                        }
                    }
                    idx += 1;
                }
            }
        }
    }

    recompute_bounds(doc, buildings);
}

/// Recompute AABB / TileCount / DragRectCenter from buildings + mesh edges.
/// Keeps DragRectCenter stable (stamp pivot); expands AABB in absolute tile space.
pub fn recompute_bounds(doc: &mut DocumentOrdered, buildings: &[Building]) {
    let info = read_mesh_info(doc);
    let (cx, cy) = info
        .as_ref()
        .map(|i| i.drag_center)
        .unwrap_or((0.0, 0.0));

    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;

    let mut push = |tx: i32, ty: i32| {
        min_x = min_x.min(tx);
        min_y = min_y.min(ty);
        max_x = max_x.max(tx);
        max_y = max_y.max(ty);
    };

    for b in buildings {
        // Approximate footprint as center tile.
        push((cx + b.x).round() as i32, (cy + b.y).round() as i32);
    }
    if let Some(info) = &info {
        for e in &info.edges {
            push((e.x0 as f32 * 0.5).round() as i32, (e.y0 as f32 * 0.5).round() as i32);
            push((e.x1 as f32 * 0.5).round() as i32, (e.y1 as f32 * 0.5).round() as i32);
        }
    }

    if min_x == i32::MAX {
        return;
    }

    // Include a little padding like original stamps.
    min_x -= 1;
    min_y -= 1;
    max_x += 1;
    max_y += 1;
    let tile_count = ((max_x - min_x) * (max_y - min_y)).max(0) as u32;

    let Some(root) = pipette_mut(doc) else {
        return;
    };
    if let Some(mesh) = find_tag_mut(&mut root.children, "MeshGraphData") {
        if let Some(aabb) = find_attrib_mut(&mut mesh.children, "AABB") {
            write_i32_quad(aabb, [min_x, min_y, max_x, max_y]);
        }
        if let Some(tc) = find_attrib_mut(&mut mesh.children, "TileCount") {
            let mut bytes = vec![0u8; 4];
            bytes.copy_from_slice(&tile_count.to_le_bytes());
            tc.content = bytes;
        }
        // Keep DragRectCenter as pivot; do not move with edits.
        let _ = find_attrib_mut(&mut mesh.children, "DragRectCenter");
    }
}

/// When cloning a building, also clone a matching polygon template if present.
pub fn clone_polygon_for_new_building(
    doc: &mut DocumentOrdered,
    template_guid: u32,
    new_guid: u32,
    x: f32,
    y: f32,
) {
    let Some(root) = pipette_mut(doc) else {
        return;
    };
    let Some(polygons) = find_tag_mut(&mut root.children, "Polygons") else {
        return;
    };
    let template = polygons.children.iter().find_map(|n| match n {
        Node::Tag(t) if t.name == "None" => {
            let g = find_attrib(&t.children, "GUID").map(|a| i32_le(&a.content) as u32)?;
            // Only clone a polygon that matches the template GUID — never the
            // sole unrelated field polygon (`len() == 1` used to leak farm grids).
            if g == template_guid {
                Some(t.clone())
            } else {
                None
            }
        }
        _ => None,
    });
    let Some(mut entry) = template else {
        return;
    };
    if let Some(g) = find_attrib_mut(&mut entry.children, "GUID") {
        let mut bytes = g.content.clone();
        if bytes.len() < 4 {
            bytes.resize(4, 0);
        }
        bytes[0..4].copy_from_slice(&(new_guid as i32).to_le_bytes());
        g.content = bytes;
    }
    if let Some(rel) = find_attrib_mut(&mut entry.children, "RelativePos") {
        write_f32_pair(rel, x, y);
    }
    polygons.children.push(Node::Tag(entry));
}

/// Roman Celtic Earthen Road (dirt) — default stamp brush.
pub const ROAD_GUID_DIRT: i32 = 24355;
/// Roman Celtic Paved Road (Ziegel / stone).
pub const ROAD_GUID_STONE: i32 = 24357;

/// Region-specific street GUIDs that should activate the road brush (not building place).
pub const ROAD_BRUSH_GUIDS: &[i32] = &[
    24355, 24357, // Celtic earthen / paved
    8541, 23996, // Roman earthen / paved
    150549, 153325, // Egyptian (Greco-Roman) earthen / paved
];

pub fn is_road_brush_guid(guid: i32) -> bool {
    ROAD_BRUSH_GUIDS.contains(&guid)
}

const DEFAULT_DIRT_ROAD_GUID: i32 = ROAD_GUID_DIRT;

fn edges_list_mut(doc: &mut DocumentOrdered) -> Option<&mut Tag> {
    let root = pipette_mut(doc)?;
    let mesh = find_tag_mut(&mut root.children, "MeshGraphData")?;
    let edges_wrap = find_tag_mut(&mut mesh.children, "Edges")?;
    let existing = edges_wrap.children.iter().position(|child| match child {
        Node::Tag(list) => list.name == "Edges",
        _ => false,
    });
    let idx = if let Some(i) = existing {
        i
    } else {
        let list = Tag {
            id: edges_wrap.id,
            name: "Edges".into(),
            children: Vec::new(),
        };
        edges_wrap.children.insert(0, Node::Tag(list));
        0
    };
    match edges_wrap.children.get_mut(idx) {
        Some(Node::Tag(t)) => Some(t),
        _ => None,
    }
}

fn edge_template(doc: &DocumentOrdered) -> Option<Tag> {
    let root = doc.roots.iter().find_map(|n| match n {
        Node::Tag(t) if t.name == "PipetteData" => Some(t),
        _ => None,
    })?;
    let mesh = find_tag(&root.children, "MeshGraphData")?;
    let edges_wrap = find_tag(&mesh.children, "Edges")?;
    for child in &edges_wrap.children {
        let Node::Tag(list) = child else { continue };
        if list.name != "Edges" {
            continue;
        }
        for edge in &list.children {
            if let Node::Tag(e) = edge {
                if e.name == "None" {
                    return Some(e.clone());
                }
            }
        }
    }
    None
}

fn make_cell_edge(template: &Tag, guid: i32, sx0: i32, sy0: i32, sx1: i32, sy1: i32) -> Tag {
    let mut entry = template.clone();
    entry.name = "None".into();
    if let Some(g) = find_attrib_mut(&mut entry.children, "guid") {
        let mut bytes = vec![0u8; 4];
        bytes.copy_from_slice(&guid.to_le_bytes());
        g.content = bytes;
    }
    if let Some(pmin) = find_attrib_mut(&mut entry.children, "PosMin") {
        write_i32_pair(pmin, sx0, sy0);
    }
    if let Some(pmax) = find_attrib_mut(&mut entry.children, "PosMax") {
        write_i32_pair(pmax, sx1, sy1);
    }
    entry
}

fn write_mesh_graph_count(doc: &mut DocumentOrdered, count: i32) {
    let Some(root) = pipette_mut(doc) else {
        return;
    };
    let Some(mesh) = find_tag_mut(&mut root.children, "MeshGraphData") else {
        return;
    };
    let Some(edges_wrap) = find_tag_mut(&mut mesh.children, "Edges") else {
        return;
    };
    if let Some(c) = find_attrib_mut(&mut edges_wrap.children, "MeshGraphCount") {
        let mut bytes = vec![0u8; c.content.len().max(4)];
        bytes[0..4].copy_from_slice(&count.to_le_bytes());
        c.content = bytes;
    }
}

fn rebuild_point_edges(
    doc: &mut DocumentOrdered,
    buildings: &[Building],
    cells: &[(i32, i32, i32)],
) -> bool {
    let info = read_mesh_info(doc);
    let (cx, cy) = info.as_ref().map(|i| i.drag_center).unwrap_or((0.0, 0.0));
    let Some(template) = edge_template(doc).or_else(|| {
        // Minimal synthetic edge if stamp had an empty mesh list.
        Some(Tag {
            id: 1,
            name: "None".into(),
            children: vec![
                Node::Attrib(Attrib {
                    id: 32775,
                    name: "guid".into(),
                    content: DEFAULT_DIRT_ROAD_GUID.to_le_bytes().to_vec(),
                }),
                Node::Attrib(Attrib {
                    id: 32776,
                    name: "PosMin".into(),
                    content: vec![0; 8],
                }),
                Node::Attrib(Attrib {
                    id: 32777,
                    name: "PosMax".into(),
                    content: vec![0; 8],
                }),
            ],
        })
    }) else {
        return false;
    };

    let Some(list) = edges_list_mut(doc) else {
        return false;
    };
    list.children.clear();
    for (x, y, g) in cells {
        // Half-tile stub at cell centre-line → exactly one rasterized tile
        // with inclusive floor (matches existing MeshGraph stubs).
        let rx0 = *x as f32;
        let rx1 = *x as f32 + 0.5;
        let ry = *y as f32 + 0.5;
        let sx0 = (2.0 * (cx + rx0)).round() as i32;
        let sx1 = (2.0 * (cx + rx1)).round() as i32;
        let sy = (2.0 * (cy + ry)).round() as i32;
        list.children
            .push(Node::Tag(make_cell_edge(&template, *g, sx0, sy, sx1, sy)));
    }
    let count = cells.len() as i32;
    write_mesh_graph_count(doc, count);
    recompute_bounds(doc, buildings);
    true
}

/// Toggle a road tile at stamp-relative integer cell `(tx, ty)` using `road_guid`.
/// Clicking an existing road removes it; empty cell places `road_guid`.
/// Returns `Some(true)` if the cell is a road afterwards, `Some(false)` if cleared.
pub fn toggle_road_cell(
    doc: &mut DocumentOrdered,
    buildings: &[Building],
    tx: i32,
    ty: i32,
    road_guid: i32,
) -> Option<bool> {
    let mut map: std::collections::BTreeMap<(i32, i32), i32> = std::collections::BTreeMap::new();
    for (x, y, g) in mesh_road_cells(doc) {
        map.insert((x, y), g);
    }
    let on = if map.contains_key(&(tx, ty)) {
        map.remove(&(tx, ty));
        false
    } else {
        map.insert((tx, ty), road_guid);
        true
    };
    let cells: Vec<(i32, i32, i32)> = map.into_iter().map(|((x, y), g)| (x, y, g)).collect();
    if rebuild_point_edges(doc, buildings, &cells) {
        Some(on)
    } else {
        None
    }
}

/// Remove all MeshGraph road edges (keeps structure so later painting works).
pub fn clear_roads(doc: &mut DocumentOrdered, buildings: &[Building]) -> bool {
    rebuild_point_edges(doc, buildings, &[])
}

/// Anno-style orthogonal path: straight line, or one 90° bend (longer axis first).
pub fn orthogonal_road_path(x0: i32, y0: i32, x1: i32, y1: i32) -> Vec<(i32, i32)> {
    let mut cells = Vec::new();
    let dx = (x1 - x0).abs();
    let dy = (y1 - y0).abs();
    if dx >= dy {
        let step = if x1 >= x0 { 1 } else { -1 };
        let mut x = x0;
        loop {
            cells.push((x, y0));
            if x == x1 {
                break;
            }
            x += step;
        }
        if y1 != y0 {
            let step = if y1 >= y0 { 1 } else { -1 };
            let mut y = y0;
            while y != y1 {
                y += step;
                cells.push((x1, y));
            }
        }
    } else {
        let step = if y1 >= y0 { 1 } else { -1 };
        let mut y = y0;
        loop {
            cells.push((x0, y));
            if y == y1 {
                break;
            }
            y += step;
        }
        if x1 != x0 {
            let step = if x1 >= x0 { 1 } else { -1 };
            let mut x = x0;
            while x != x1 {
                x += step;
                cells.push((x, y1));
            }
        }
    }
    cells
}

/// Apply paint/erase along an orthogonal path (single mesh rebuild).
pub fn apply_road_path(
    doc: &mut DocumentOrdered,
    buildings: &[Building],
    cells: &[(i32, i32)],
    paint_on: bool,
    road_guid: i32,
) -> usize {
    let mut map: std::collections::BTreeMap<(i32, i32), i32> = std::collections::BTreeMap::new();
    for (x, y, g) in mesh_road_cells(doc) {
        map.insert((x, y), g);
    }
    let mut n = 0usize;
    for &(tx, ty) in cells {
        if paint_on {
            match map.get(&(tx, ty)) {
                Some(g) if *g == road_guid => {}
                _ => {
                    map.insert((tx, ty), road_guid);
                    n += 1;
                }
            }
        } else if map.remove(&(tx, ty)).is_some() {
            n += 1;
        }
    }
    if n == 0 {
        return 0;
    }
    let all: Vec<(i32, i32, i32)> = map.into_iter().map(|((x, y), g)| (x, y, g)).collect();
    if rebuild_point_edges(doc, buildings, &all) {
        n
    } else {
        0
    }
}

/// Paint or erase a road cell (no toggle). When painting, uses `road_guid`
/// (also overwrites an existing road of a different type).
pub fn set_road_cell(
    doc: &mut DocumentOrdered,
    buildings: &[Building],
    tx: i32,
    ty: i32,
    enabled: bool,
    road_guid: i32,
) -> bool {
    let mut map: std::collections::BTreeMap<(i32, i32), i32> = std::collections::BTreeMap::new();
    for (x, y, g) in mesh_road_cells(doc) {
        map.insert((x, y), g);
    }
    let changed = if enabled {
        match map.get(&(tx, ty)) {
            Some(g) if *g == road_guid => false,
            _ => {
                map.insert((tx, ty), road_guid);
                true
            }
        }
    } else {
        map.remove(&(tx, ty)).is_some()
    };
    if !changed {
        return true;
    }
    let cells: Vec<(i32, i32, i32)> = map.into_iter().map(|((x, y), g)| (x, y, g)).collect();
    rebuild_point_edges(doc, buildings, &cells)
}

/// Shift all mesh edge endpoints and DragRectCenter/AABB by building-space delta.
/// Used when the whole stamp content should move (rare); kept for completeness.
#[allow(dead_code)]
pub fn shift_mesh_by_building_delta(doc: &mut DocumentOrdered, dx: f32, dy: f32) {
    let d_tile_x = dx.round() as i32;
    let d_tile_y = dy.round() as i32;
    let d_sub_x = (dx * 2.0).round() as i32;
    let d_sub_y = (dy * 2.0).round() as i32;
    let Some(root) = pipette_mut(doc) else {
        return;
    };
    if let Some(mesh) = find_tag_mut(&mut root.children, "MeshGraphData") {
        if let Some(drag) = find_attrib_mut(&mut mesh.children, "DragRectCenter") {
            if let Some((x, y)) = f32_pair(&drag.content) {
                write_f32_pair(drag, x + dx, y + dy);
            }
        }
        if let Some(aabb) = find_attrib_mut(&mut mesh.children, "AABB") {
            if aabb.content.len() >= 16 {
                let v = [
                    i32_le(&aabb.content[0..4]) + d_tile_x,
                    i32_le(&aabb.content[4..8]) + d_tile_y,
                    i32_le(&aabb.content[8..12]) + d_tile_x,
                    i32_le(&aabb.content[12..16]) + d_tile_y,
                ];
                write_i32_quad(aabb, v);
            }
        }
        if let Some(edges_wrap) = find_tag_mut(&mut mesh.children, "Edges") {
            for child in &mut edges_wrap.children {
                let Node::Tag(list) = child else { continue };
                if list.name != "Edges" {
                    continue;
                }
                for edge in &mut list.children {
                    let Node::Tag(e) = edge else { continue };
                    if let Some(pmin) = find_attrib_mut(&mut e.children, "PosMin") {
                        if let Some((x, y)) = (pmin.content.len() >= 8).then(|| {
                            (i32_le(&pmin.content[0..4]), i32_le(&pmin.content[4..8]))
                        }) {
                            write_i32_pair(pmin, x + d_sub_x, y + d_sub_y);
                        }
                    }
                    if let Some(pmax) = find_attrib_mut(&mut e.children, "PosMax") {
                        if let Some((x, y)) = (pmax.content.len() >= 8).then(|| {
                            (i32_le(&pmax.content[0..4]), i32_le(&pmax.content[4..8]))
                        }) {
                            write_i32_pair(pmax, x + d_sub_x, y + d_sub_y);
                        }
                    }
                }
            }
        }
    }
    if let Some(drag) = find_attrib_mut(&mut root.children, "DragRectCenter") {
        if let Some((x, y)) = f32_pair(&drag.content) {
            write_f32_pair(drag, x + dx, y + dy);
        }
    }
}

