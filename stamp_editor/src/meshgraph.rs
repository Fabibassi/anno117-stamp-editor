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
        // In-game stamps use multiple nested Edges lists: #0 = streets,
        // #1 = aqueducts (MinGround/MaxGround). Read all of them.
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
///
/// `TileCount` must be occupied cells (buildings + roads/aqueducts), **not** the
/// AABB area — in-game stamps use ~occupied count; area-sized counts get rejected.
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
    let mut occupied: std::collections::HashSet<(i32, i32)> = std::collections::HashSet::new();

    let mut push_abs = |tx: i32, ty: i32| {
        min_x = min_x.min(tx);
        min_y = min_y.min(ty);
        max_x = max_x.max(tx);
        max_y = max_y.max(ty);
    };

    for b in buildings {
        // Approximate footprint as center tile (absolute + stamp-relative).
        let ax = (cx + b.x).round() as i32;
        let ay = (cy + b.y).round() as i32;
        push_abs(ax, ay);
        occupied.insert((
            relative_to_tile(b.x, cx),
            relative_to_tile(b.y, cy),
        ));
    }
    if let Some(info) = &info {
        for e in &info.edges {
            push_abs(
                (e.x0 as f32 * 0.5).round() as i32,
                (e.y0 as f32 * 0.5).round() as i32,
            );
            push_abs(
                (e.x1 as f32 * 0.5).round() as i32,
                (e.y1 as f32 * 0.5).round() as i32,
            );
            let x0 = e.x0 as f32 * 0.5 - cx;
            let y0 = e.y0 as f32 * 0.5 - cy;
            let x1 = e.x1 as f32 * 0.5 - cx;
            let y1 = e.y1 as f32 * 0.5 - cy;
            for (x, y) in rasterize_edge_f(x0, y0, x1, y1) {
                occupied.insert((x, y));
            }
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
    let tile_count = occupied.len().max(1) as u32;

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

/// Aqueduct segment MeshGraph brushes (separate Edges list from roads).
/// MinGround = low aqueduct; MaxGround = raised span (crosses streets).
pub const AQUEDUCT_BRUSH_GUIDS: &[i32] = &[
    19723,  // Roman MinGround
    81354,  // Roman MaxGround
    29525,  // Celtic MinGround
    82038,  // Celtic MaxGround
    149792, // Egyptian MinGround
    149793, // Egyptian MaxGround
];

pub fn is_road_brush_guid(guid: i32) -> bool {
    ROAD_BRUSH_GUIDS.contains(&guid)
}

pub fn is_aqueduct_brush_guid(guid: i32) -> bool {
    AQUEDUCT_BRUSH_GUIDS.contains(&guid)
}

/// Raised aqueduct span (crosses streets). Ground-level corners stay MinGround.
pub fn is_aqueduct_maxground_guid(guid: i32) -> bool {
    matches!(guid, 81354 | 82038 | 149793)
}

pub fn aqueduct_guid_for_region(region_key: Option<&str>) -> i32 {
    match region_key {
        Some("Egyptian") => 149792,
        Some("Celtic") | Some("Albion") => 29525,
        _ => 19723,
    }
}

pub fn aqueduct_maxground_guid_for_region(region_key: Option<&str>) -> i32 {
    match region_key {
        Some("Egyptian") => 149793,
        Some("Celtic") | Some("Albion") => 82038,
        _ => 81354,
    }
}

/// Water inlet side of a Roman/Celtic/Egyptian cistern from pipette `Dir`
/// (0/2/4/6 = 90° steps). Confirmed via in-game "Aqueduct Cistern" stamp.
///
/// Anno stamp space is **Y-up** (+Y = north). Dir 0 faces +Y.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CisternInlet {
    North,
    West,
    South,
    East,
}

impl CisternInlet {
    pub fn from_direction(direction: i32) -> Self {
        // Snap diagonals to the next clockwise cardinal (same as 90° rotate habit).
        let d = match direction.rem_euclid(8) {
            1 => 2,
            3 => 4,
            5 => 6,
            7 => 0,
            x => x,
        };
        match d {
            2 => Self::West,
            4 => Self::South,
            6 => Self::East,
            _ => Self::North, // 0 → +Y
        }
    }
}

/// Prefer the culture-matching dirt/stone GUID for the active region filter key.
pub fn road_guid_for_region(region_key: Option<&str>, stone: bool) -> i32 {
    match (region_key, stone) {
        (Some("Roman"), false) => 8541,
        (Some("Roman"), true) => 23996,
        (Some("Egyptian"), false) => 150549,
        (Some("Egyptian"), true) => 153325,
        (_, false) => ROAD_GUID_DIRT,
        (_, true) => ROAD_GUID_STONE,
    }
}

const DEFAULT_DIRT_ROAD_GUID: i32 = ROAD_GUID_DIRT;

/// Ensure at least `n` nested `Edges` lists exist; return mutable access to index `slot`.
fn edges_list_mut(doc: &mut DocumentOrdered, slot: usize) -> Option<&mut Tag> {
    let root = pipette_mut(doc)?;
    let mesh = find_tag_mut(&mut root.children, "MeshGraphData")?;
    let edges_wrap = find_tag_mut(&mut mesh.children, "Edges")?;
    let wrap_id = edges_wrap.id;
    let mut edge_idxs: Vec<usize> = edges_wrap
        .children
        .iter()
        .enumerate()
        .filter_map(|(i, child)| match child {
            Node::Tag(list) if list.name == "Edges" => Some(i),
            _ => None,
        })
        .collect();
    while edge_idxs.len() <= slot {
        let list = Tag {
            id: wrap_id,
            name: "Edges".into(),
            children: Vec::new(),
        };
        edges_wrap.children.push(Node::Tag(list));
        edge_idxs.push(edges_wrap.children.len() - 1);
    }
    // Keep MeshGraphCount = number of Edges lists (game format: two i32, first = list count).
    let list_count = edge_idxs.len() as i32;
    if let Some(c) = find_attrib_mut(&mut edges_wrap.children, "MeshGraphCount") {
        let mut bytes = vec![0u8; c.content.len().max(8)];
        bytes[0..4].copy_from_slice(&list_count.to_le_bytes());
        if bytes.len() >= 8 {
            // preserve second dword if present, else 0
        }
        c.content = bytes;
    }
    let child_idx = edge_idxs[slot];
    match edges_wrap.children.get_mut(child_idx) {
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

/// Resolve MeshGraph edge attrib IDs from the document dictionary.
/// Hardcoded IDs are unsafe — a wrong id survives in-memory (matched by name) but
/// remaps to another name after dump/load (roads vanish or become Dirt).
fn edge_attrib_ids(doc: &DocumentOrdered) -> Option<(u16, u16, u16)> {
    let guid = doc.attribs.id_of("guid")?;
    let pmin = doc.attribs.id_of("PosMin")?;
    let pmax = doc.attribs.id_of("PosMax")?;
    Some((guid, pmin, pmax))
}

fn synthetic_edge_template(doc: &DocumentOrdered) -> Option<Tag> {
    let (guid_id, pmin_id, pmax_id) = edge_attrib_ids(doc)?;
    Some(Tag {
        id: 1, // None
        name: "None".into(),
        children: vec![
            Node::Attrib(Attrib {
                id: guid_id,
                name: "guid".into(),
                content: DEFAULT_DIRT_ROAD_GUID.to_le_bytes().to_vec(),
            }),
            Node::Attrib(Attrib {
                id: pmin_id,
                name: "PosMin".into(),
                content: vec![0; 8],
            }),
            Node::Attrib(Attrib {
                id: pmax_id,
                name: "PosMax".into(),
                content: vec![0; 8],
            }),
        ],
    })
}

fn make_cell_edge(
    template: &Tag,
    guid: i32,
    sx0: i32,
    sy0: i32,
    sx1: i32,
    sy1: i32,
    ids: (u16, u16, u16),
) -> Tag {
    let (guid_id, pmin_id, pmax_id) = ids;
    // In-game edges store the lower corner in PosMin (axis-aligned).
    let (min_x, max_x) = if sx0 <= sx1 { (sx0, sx1) } else { (sx1, sx0) };
    let (min_y, max_y) = if sy0 <= sy1 { (sy0, sy1) } else { (sy1, sy0) };
    let mut entry = template.clone();
    entry.name = "None".into();
    let guid_bytes = guid.to_le_bytes().to_vec();
    let guid_key = if find_attrib(&entry.children, "guid").is_some() {
        "guid"
    } else if find_attrib(&entry.children, "GUID").is_some() {
        "GUID"
    } else {
        ""
    };
    if !guid_key.is_empty() {
        if let Some(g) = find_attrib_mut(&mut entry.children, guid_key) {
            g.id = guid_id;
            g.name = "guid".into();
            g.content = guid_bytes;
        }
    } else {
        entry.children.push(Node::Attrib(Attrib {
            id: guid_id,
            name: "guid".into(),
            content: guid_bytes,
        }));
    }
    if let Some(pmin) = find_attrib_mut(&mut entry.children, "PosMin") {
        pmin.id = pmin_id;
        write_i32_pair(pmin, min_x, min_y);
    }
    if let Some(pmax) = find_attrib_mut(&mut entry.children, "PosMax") {
        pmax.id = pmax_id;
        write_i32_pair(pmax, max_x, max_y);
    }
    entry
}

/// Map MinGround → MaxGround (raised span over streets) per region pair.
pub fn aqueduct_raised_guid(guid: i32) -> Option<i32> {
    match guid {
        19723 => Some(81354),
        29525 => Some(82038),
        149792 => Some(149793),
        g if is_aqueduct_maxground_guid(g) => Some(g),
        _ => None,
    }
}

/// Relative position of a cell's centre given that axis' DragRectCenter.
/// Absolute centres sit on *.5; relative = floor(drag)+cell+0.5 − drag
/// (equals `cell+0.5` when drag is integer, `cell` when drag ends in `.5`).
fn cell_center_rel(cell: i32, drag: f32) -> f32 {
    drag.floor() + cell as f32 + 0.5 - drag
}

/// Coalesce painted cells into in-game-style MeshGraph edges.
///
/// Game stamps use long centre-to-centre spans (subtile Δ multiple of 2). Writing
/// one half-tile stub per cell makes Anno place a tower on every cell and shift
/// segments onto half-blocks.
fn coalesce_cells_to_edge_spans(
    cells: &[(i32, i32, i32)],
    cx: f32,
    cy: f32,
) -> Vec<(f32, f32, f32, f32, i32)> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut by_guid: BTreeMap<i32, BTreeSet<(i32, i32)>> = BTreeMap::new();
    for &(x, y, g) in cells {
        by_guid.entry(g).or_default().insert((x, y));
    }

    let mut spans = Vec::new();
    for (guid, set) in &by_guid {
        spans.extend(coalesce_set_to_spans(set, *guid, cx, cy));
    }
    spans
}

fn coalesce_set_to_spans(
    set: &std::collections::BTreeSet<(i32, i32)>,
    guid: i32,
    cx: f32,
    cy: f32,
) -> Vec<(f32, f32, f32, f32, i32)> {
    use std::collections::BTreeSet;

    let mut spans = Vec::new();
    let mut h_units: BTreeSet<(i32, i32)> = BTreeSet::new();
    let mut v_units: BTreeSet<(i32, i32)> = BTreeSet::new();
    for &(x, y) in set {
        if set.contains(&(x + 1, y)) {
            h_units.insert((x, y));
        }
        if set.contains(&(x, y + 1)) {
            v_units.insert((x, y));
        }
    }

    let mut used_h = BTreeSet::new();
    for &(x0, y) in &h_units {
        if used_h.contains(&(x0, y)) {
            continue;
        }
        let mut x1 = x0;
        while h_units.contains(&(x1, y)) {
            used_h.insert((x1, y));
            x1 += 1;
        }
        let y_line = cell_center_rel(y, cy);
        spans.push((
            cell_center_rel(x0, cx),
            y_line,
            cell_center_rel(x1, cx),
            y_line,
            guid,
        ));
    }

    let mut used_v = BTreeSet::new();
    for &(x, y0) in &v_units {
        if used_v.contains(&(x, y0)) {
            continue;
        }
        let mut y1 = y0;
        while v_units.contains(&(x, y1)) {
            used_v.insert((x, y1));
            y1 += 1;
        }
        let x_line = cell_center_rel(x, cx);
        spans.push((
            x_line,
            cell_center_rel(y0, cy),
            x_line,
            cell_center_rel(y1, cy),
            guid,
        ));
    }

    for &(x, y) in set {
        let lonely = !set.contains(&(x - 1, y))
            && !set.contains(&(x + 1, y))
            && !set.contains(&(x, y - 1))
            && !set.contains(&(x, y + 1));
        if lonely {
            let xc = cell_center_rel(x, cx);
            let yc = cell_center_rel(y, cy);
            // Half-tile stub ending at cell centre → one rasterized tile.
            spans.push((xc - 0.5, yc, xc, yc, guid));
        }
    }
    spans
}

/// Aqueduct edges: same coalescing, but unit segments that touch a street cell
/// become MaxGround (arched span) — matches Libertus / in-game crossings.
fn aqueduct_spans_with_road_crossings(
    cells: &[(i32, i32, i32)],
    roads: &std::collections::BTreeSet<(i32, i32)>,
    cx: f32,
    cy: f32,
) -> Vec<(f32, f32, f32, f32, i32)> {
    use std::collections::BTreeSet;

    // Topology ignores Min/Max — only connectivity matters.
    let mut set: BTreeSet<(i32, i32)> = BTreeSet::new();
    let mut base_guid = 19723;
    for &(x, y, g) in cells {
        set.insert((x, y));
        if is_aqueduct_brush_guid(g) && !is_aqueduct_maxground_guid(g) {
            base_guid = g;
        } else if is_aqueduct_maxground_guid(g) {
            // Prefer the matching MinGround for the region.
            if let Some(min_g) = match g {
                81354 => Some(19723),
                82038 => Some(29525),
                149793 => Some(149792),
                _ => None,
            } {
                base_guid = min_g;
            }
        }
    }
    let raised = aqueduct_raised_guid(base_guid).unwrap_or(81354);

    let mut h_units: Vec<(i32, i32, i32)> = Vec::new(); // x,y,guid for unit (x,y)-(x+1,y)
    let mut v_units: Vec<(i32, i32, i32)> = Vec::new();
    for &(x, y) in &set {
        if set.contains(&(x + 1, y)) {
            let g = if roads.contains(&(x, y)) || roads.contains(&(x + 1, y)) {
                raised
            } else {
                base_guid
            };
            h_units.push((x, y, g));
        }
        if set.contains(&(x, y + 1)) {
            let g = if roads.contains(&(x, y)) || roads.contains(&(x, y + 1)) {
                raised
            } else {
                base_guid
            };
            v_units.push((x, y, g));
        }
    }

    let mut spans = Vec::new();
    // Merge collinear unit edges that share the same guid.
    let mut used_h = BTreeSet::new();
    for &(x0, y, g0) in &h_units {
        if used_h.contains(&(x0, y)) {
            continue;
        }
        let mut x1 = x0;
        used_h.insert((x0, y));
        loop {
            let next = h_units
                .iter()
                .find(|&&(x, yy, g)| x == x1 + 1 && yy == y && g == g0 && !used_h.contains(&(x, yy)));
            match next {
                Some(&(x, _, _)) => {
                    used_h.insert((x, y));
                    x1 = x;
                }
                None => break,
            }
        }
        // unit (x0,y)-(x0+1,y) … (x1,y)-(x1+1,y) → centres of x0 .. x1+1
        let y_line = cell_center_rel(y, cy);
        spans.push((
            cell_center_rel(x0, cx),
            y_line,
            cell_center_rel(x1 + 1, cx),
            y_line,
            g0,
        ));
    }

    let mut used_v = BTreeSet::new();
    for &(x, y0, g0) in &v_units {
        if used_v.contains(&(x, y0)) {
            continue;
        }
        let mut y1 = y0;
        used_v.insert((x, y0));
        loop {
            let next = v_units
                .iter()
                .find(|&&(xx, y, g)| xx == x && y == y1 + 1 && g == g0 && !used_v.contains(&(xx, y)));
            match next {
                Some(&(_, y, _)) => {
                    used_v.insert((x, y));
                    y1 = y;
                }
                None => break,
            }
        }
        let x_line = cell_center_rel(x, cx);
        spans.push((
            x_line,
            cell_center_rel(y0, cy),
            x_line,
            cell_center_rel(y1 + 1, cy),
            g0,
        ));
    }

    for &(x, y) in &set {
        let lonely = !set.contains(&(x - 1, y))
            && !set.contains(&(x + 1, y))
            && !set.contains(&(x, y - 1))
            && !set.contains(&(x, y + 1));
        if lonely {
            let g = if roads.contains(&(x, y)) {
                raised
            } else {
                base_guid
            };
            let xc = cell_center_rel(x, cx);
            let yc = cell_center_rel(y, cy);
            spans.push((xc - 0.5, yc, xc, yc, g));
        }
    }
    spans
}

fn push_span_edges(
    list: &mut Tag,
    spans: &[(f32, f32, f32, f32, i32)],
    cx: f32,
    cy: f32,
    template: &Tag,
    ids: (u16, u16, u16),
) {
    for &(x0, y0, x1, y1, g) in spans {
        let sx0 = (2.0 * (cx + x0)).round() as i32;
        let sy0 = (2.0 * (cy + y0)).round() as i32;
        let sx1 = (2.0 * (cx + x1)).round() as i32;
        let sy1 = (2.0 * (cy + y1)).round() as i32;
        list.children.push(Node::Tag(make_cell_edge(
            template, g, sx0, sy0, sx1, sy1, ids,
        )));
    }
}

fn fill_edges_list(
    list: &mut Tag,
    cells: &[(i32, i32, i32)],
    cx: f32,
    cy: f32,
    template: &Tag,
    ids: (u16, u16, u16),
) {
    list.children.clear();
    push_span_edges(
        list,
        &coalesce_cells_to_edge_spans(cells, cx, cy),
        cx,
        cy,
        template,
        ids,
    );
}

fn fill_aqueduct_edges_list(
    list: &mut Tag,
    cells: &[(i32, i32, i32)],
    roads: &std::collections::BTreeSet<(i32, i32)>,
    cx: f32,
    cy: f32,
    template: &Tag,
    ids: (u16, u16, u16),
) {
    list.children.clear();
    push_span_edges(
        list,
        &aqueduct_spans_with_road_crossings(cells, roads, cx, cy),
        cx,
        cy,
        template,
        ids,
    );
}

fn mesh_drag_center(doc: &DocumentOrdered) -> (f32, f32) {
    read_mesh_info(doc)
        .map(|i| i.drag_center)
        .unwrap_or((0.0, 0.0))
}

/// Rasterized cells from one nested Edges list only (0 = roads, 1 = aqueducts).
fn mesh_cells_in_list(doc: &DocumentOrdered, slot: usize) -> Vec<(i32, i32, i32)> {
    let Some(info) = read_mesh_info(doc) else {
        return Vec::new();
    };
    let (cx, cy) = info.drag_center;
    let root = doc.roots.iter().find_map(|n| match n {
        Node::Tag(t) if t.name == "PipetteData" => Some(t),
        _ => None,
    });
    let Some(root) = root else {
        return Vec::new();
    };
    let Some(mesh) = find_tag(&root.children, "MeshGraphData") else {
        return Vec::new();
    };
    let Some(edges_wrap) = find_tag(&mesh.children, "Edges") else {
        return Vec::new();
    };
    let mut lists = edges_wrap.children.iter().filter_map(|c| match c {
        Node::Tag(t) if t.name == "Edges" => Some(t),
        _ => None,
    });
    let Some(list) = lists.nth(slot) else {
        return Vec::new();
    };
    let mut cells = Vec::new();
    for edge in &list.children {
        let Node::Tag(e) = edge else { continue };
        if e.name != "None" {
            continue;
        }
        let guid = find_attrib(&e.children, "guid")
            .map(|a| i32_le(&a.content))
            .unwrap_or(0);
        let Some(pmin) = find_attrib(&e.children, "PosMin").and_then(|a| {
            (a.content.len() >= 8).then(|| (i32_le(&a.content[0..4]), i32_le(&a.content[4..8])))
        }) else {
            continue;
        };
        let Some(pmax) = find_attrib(&e.children, "PosMax").and_then(|a| {
            (a.content.len() >= 8).then(|| (i32_le(&a.content[0..4]), i32_le(&a.content[4..8])))
        }) else {
            continue;
        };
        let x0 = (pmin.0 as f32) * 0.5 - cx;
        let y0 = (pmin.1 as f32) * 0.5 - cy;
        let x1 = (pmax.0 as f32) * 0.5 - cx;
        let y1 = (pmax.1 as f32) * 0.5 - cy;
        for (x, y) in rasterize_edge_f(x0, y0, x1, y1) {
            cells.push((x, y, guid));
        }
    }
    cells
}

fn rebuild_one_list(
    doc: &mut DocumentOrdered,
    buildings: &[Building],
    slot: usize,
    cells: &[(i32, i32, i32)],
) -> bool {
    let road_set: std::collections::BTreeSet<(i32, i32)> = if slot == 1 {
        mesh_cells_in_list(doc, 0)
            .into_iter()
            .map(|(x, y, _)| (x, y))
            .collect()
    } else {
        std::collections::BTreeSet::new()
    };
    let (cx, cy) = mesh_drag_center(doc);
    let Some(ids) = edge_attrib_ids(doc) else {
        return false;
    };
    let Some(template) = edge_template(doc).or_else(|| synthetic_edge_template(doc)) else {
        return false;
    };
    let Some(list) = edges_list_mut(doc, slot) else {
        return false;
    };
    if slot == 1 {
        fill_aqueduct_edges_list(list, cells, &road_set, cx, cy, &template, ids);
    } else {
        fill_edges_list(list, cells, cx, cy, &template, ids);
    }
    recompute_bounds(doc, buildings);
    true
}

/// After roads change, rewrite aqueduct edges so crossings become MaxGround.
fn refresh_aqueduct_road_crossings(doc: &mut DocumentOrdered, buildings: &[Building]) {
    let aq = mesh_cells_in_list(doc, 1);
    if aq.is_empty() {
        return;
    }
    let _ = rebuild_one_list(doc, buildings, 1, &aq);
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
    let slot = if is_aqueduct_brush_guid(road_guid) { 1 } else { 0 };
    let mut map: std::collections::BTreeMap<(i32, i32), i32> = std::collections::BTreeMap::new();
    for (x, y, g) in mesh_cells_in_list(doc, slot) {
        map.insert((x, y), g);
    }
    let on = if map.contains_key(&(tx, ty)) {
        map.remove(&(tx, ty));
        false
    } else {
        map.insert((tx, ty), road_guid);
        true
    };
    let cells: Vec<_> = map.into_iter().map(|((x, y), g)| (x, y, g)).collect();
    if rebuild_one_list(doc, buildings, slot, &cells) {
        if slot == 0 {
            refresh_aqueduct_road_crossings(doc, buildings);
        }
        Some(on)
    } else {
        None
    }
}

/// Remove all MeshGraph road edges (keeps aqueduct list untouched).
pub fn clear_roads(doc: &mut DocumentOrdered, buildings: &[Building]) -> bool {
    rebuild_one_list(doc, buildings, 0, &[])
}

/// Wipe street + aqueduct MeshGraph lists (full replace, no cell-wise leftovers).
pub fn clear_all_mesh_paths(doc: &mut DocumentOrdered, buildings: &[Building]) -> bool {
    let ok0 = rebuild_one_list(doc, buildings, 0, &[]);
    let ok1 = rebuild_one_list(doc, buildings, 1, &[]);
    ok0 && ok1
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
/// Road and aqueduct brushes are separate layers — aqueducts may share a cell with a road.
pub fn apply_road_path(
    doc: &mut DocumentOrdered,
    buildings: &[Building],
    cells: &[(i32, i32)],
    paint_on: bool,
    road_guid: i32,
) -> usize {
    // Demolish wipes both lists cell-wise without rewriting aqueduct spans wholesale
    // when only roads change — each list is edited independently.
    if road_guid == 0 && !paint_on {
        let mut n = 0usize;
        for slot in [0usize, 1usize] {
            let mut map: std::collections::BTreeMap<(i32, i32), i32> =
                std::collections::BTreeMap::new();
            for (x, y, g) in mesh_cells_in_list(doc, slot) {
                map.insert((x, y), g);
            }
            let mut touched = false;
            for &(tx, ty) in cells {
                if map.remove(&(tx, ty)).is_some() {
                    n += 1;
                    touched = true;
                }
            }
            if touched {
                let out: Vec<_> = map.into_iter().map(|((x, y), g)| (x, y, g)).collect();
                let _ = rebuild_one_list(doc, buildings, slot, &out);
            }
        }
        refresh_aqueduct_road_crossings(doc, buildings);
        return n;
    }

    let slot = if is_aqueduct_brush_guid(road_guid) { 1 } else { 0 };
    let mut map: std::collections::BTreeMap<(i32, i32), i32> =
        std::collections::BTreeMap::new();
    for (x, y, g) in mesh_cells_in_list(doc, slot) {
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
    let out: Vec<_> = map.into_iter().map(|((x, y), g)| (x, y, g)).collect();
    if rebuild_one_list(doc, buildings, slot, &out) {
        if slot == 0 {
            refresh_aqueduct_road_crossings(doc, buildings);
        }
        n
    } else {
        0
    }
}

/// Paint or erase a road/aqueduct cell (no toggle). Layer-aware like `apply_road_path`.
pub fn set_road_cell(
    doc: &mut DocumentOrdered,
    buildings: &[Building],
    tx: i32,
    ty: i32,
    enabled: bool,
    road_guid: i32,
) -> bool {
    let slot = if is_aqueduct_brush_guid(road_guid) { 1 } else { 0 };
    let mut map: std::collections::BTreeMap<(i32, i32), i32> =
        std::collections::BTreeMap::new();
    for (x, y, g) in mesh_cells_in_list(doc, slot) {
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
    let out: Vec<_> = map.into_iter().map(|((x, y), g)| (x, y, g)).collect();
    let ok = rebuild_one_list(doc, buildings, slot, &out);
    if ok && slot == 0 {
        refresh_aqueduct_road_crossings(doc, buildings);
    }
    ok
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

