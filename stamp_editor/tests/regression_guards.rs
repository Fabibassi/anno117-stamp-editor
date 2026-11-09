//! Guards against re-breaking fixes we already shipped once.
//! If one of these fails, restore the invariant — don't "rediscover" it for hours.

#[test]
fn farm_drag_auto_links_subtile_fields() {
    let app = std::fs::read_to_string("src/app.rs").expect("src/app.rs");
    assert!(
        app.contains("farm_drags_fields") && app.contains("GridOriginWS stays put"),
        "dragging a farm main must auto move_linked so SubTileGrid fields follow"
    );
}

#[test]
fn library_does_not_steal_clicks_via_dnd_drag_source() {
    let app = std::fs::read_to_string("src/app.rs").expect("src/app.rs");
    let call = app.contains(".dnd_drag_source(") || app.contains("ui.dnd_drag_source(");
    assert!(
        !call,
        "dnd_drag_source steals stamp open-clicks; use threshold set_payload instead"
    );
}

#[test]
fn roads_are_not_drawn_inset() {
    let app = std::fs::read_to_string("src/app.rs").expect("src/app.rs");
    assert!(
        !app.contains("cell_px * 0.78") && !app.contains("cell_px*0.78"),
        "road tiles must fill the full cell (0.78 inset caused gaps)"
    );
    assert!(
        app.contains("full cell fill") || app.contains("cell_screen_rect(rect, cell.x, cell.y)"),
        "roads should be drawn via cell_screen_rect"
    );
}

#[test]
fn single_click_place_uses_top_left_center() {
    let app = std::fs::read_to_string("src/app.rs").expect("src/app.rs");
    assert!(
        app.contains("stroke_anchor_tile"),
        "place/road strokes must use stroke_anchor_tile, not raw floor()"
    );
    assert!(
        !app.contains("centers.push((stroke.start.0 as f32, stroke.start.1 as f32))"),
        "never treat stroke.start tile as building center — use place_center_from_top_left"
    );
}

#[test]
fn canvas_buff_tooltips_and_chips_wired() {
    let app = std::fs::read_to_string("src/app.rs").expect("src/app.rs");
    assert!(
        app.contains("show_tooltip_at_pointer"),
        "hover tooltips on buildings must stay wired"
    );
    assert!(
        app.contains("paint_bonus_label"),
        "buff/debuff chips on canvas must stay wired"
    );
    assert!(
        app.contains("stamp_building_tip"),
        "building hover tooltip id missing"
    );
}

#[test]
fn baulette_has_no_crane_and_centers_goods_icons() {
    let app = std::fs::read_to_string("src/app.rs").expect("src/app.rs");
    assert!(
        !app.contains("ICON_CRANE"),
        "Baukran must stay removed from the Baulette"
    );
    assert!(
        app.contains("chrome PNGs are off-center")
            && app.contains("circle_filled")
            && app.contains("Rect::from_center_size(c, Vec2::splat(icon_side))"),
        "goods_tile must draw its own centered circle + centered icon (not chrome frames)"
    );
    assert!(
        app.contains("build_bar_h") && app.contains("show_upper_preview"),
        "Baulette height must follow content (no fixed empty band)"
    );
    assert!(
        !app.contains("BUILD_BAR_H: f32 = 210"),
        "old 210px Baulette height must stay gone"
    );
    assert!(
        app.contains("allocate_new_ui")
            && app.contains("band_bottom - build_bar_h")
            && app.contains("band_bottom = rect.max.y"),
        "Baulette must dock flush to the canvas/window bottom"
    );
    assert!(
        !app.contains("ViewportCommand::InnerSize"),
        "never force InnerSize every frame (breaks window snap)"
    );
}

#[test]
fn polygon_field_modules_are_extracted() {
    let stamp = std::fs::read_to_string("src/stamp.rs").expect("src/stamp.rs");
    let app = std::fs::read_to_string("src/app.rs").expect("src/app.rs");
    assert!(
        stamp.contains("subtile_grid_cells") && stamp.contains("polygon_cells"),
        "Polygons SubTileGrid bits must become polygon_cells (Loungers/Bread empty wings)"
    );
    assert!(
        stamp.contains("* 0.25") && stamp.contains("stride"),
        "SubTileGrid must use quarter-tile scale (0.25) and 32-bit row stride — 0.5 overflowed the road frame"
    );
    assert!(
        app.contains("polygon_cells"),
        "canvas must paint polygon_cells (empty road wings otherwise)"
    );
}

#[test]
fn canvas_buildings_paint_icons_not_only_text() {
    let app = std::fs::read_to_string("src/app.rs").expect("src/app.rs");
    let paint_idx = app
        .find("paint_building_icon(ctx, &painter, r, b.guid")
        .expect("placed buildings must call paint_building_icon on footprint");
    let tip_idx = app
        .find("Hover tooltip:")
        .expect("hover tooltip marker");
    assert!(
        paint_idx < tip_idx,
        "canvas building icons must be painted before the hover-tooltip block"
    );
    assert!(
        app.contains("StrokeKind::Inside"),
        "building stroke must stay Inside the footprint (Outside looked like grid gaps)"
    );
}

#[test]
fn footprint_uses_relative_to_tile_not_naive_floor() {
    let cov = std::fs::read_to_string("src/coverage.rs").expect("src/coverage.rs");
    let mesh = std::fs::read_to_string("src/meshgraph.rs").expect("src/meshgraph.rs");
    assert!(
        mesh.contains("pub fn relative_to_tile"),
        "relative_to_tile must exist — floor(rel) breaks when DragRectCenter is *.5"
    );
    assert!(
        cov.contains("relative_to_tile(b.x - w * 0.5")
            && cov.contains("relative_to_tile(b.y - h * 0.5"),
        "footprint must snap via relative_to_tile (abs-aware), not floor/ceil/round on relative floats"
    );
}
