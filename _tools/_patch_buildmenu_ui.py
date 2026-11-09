from pathlib import Path

path = Path(r"C:\Users\fabi_\Documents\Anno 117 - Pax Romana\stamps\stamp_editor\src\app.rs")
text = path.read_text(encoding="utf-8")
start = text.index("        // Anno 117 construction bar")
end = text.index("        egui::CentralPanel::default().show(ctx, |ui| {")

new = r'''        // Anno 117 construction bar — chrome + inline need/infra layout.
        egui::TopBottomPanel::bottom("buildmenu")
            .resizable(false)
            .exact_height(168.0)
            .show(ctx, |ui| {
                let bar = ui.max_rect();
                // Real construction-bar background texture.
                if let Some(bg) = self.ui_icons.chrome(ctx, chrome::BAR_BG) {
                    ui.painter().image(
                        bg.id(),
                        bar,
                        Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    ui.painter().rect_filled(bar, 0.0, Color32::from_rgb(18, 22, 30));
                }
                {
                    let style = ui.style_mut();
                    style.spacing.item_spacing = egui::vec2(4.0, 3.0);
                }

                let tabs = self.buildmenu.tabs_for(self.region_filter).to_vec();
                if let Some(t) = self.buildmenu_tab {
                    if t >= tabs.len() {
                        self.buildmenu_tab = if tabs.is_empty() { None } else { Some(0) };
                        self.buildmenu_entry = None;
                    }
                } else if !tabs.is_empty() {
                    self.buildmenu_tab = tabs.iter().position(|t| t.is_need()).or(Some(0));
                }
                if tabs.is_empty() {
                    ui.label("Kein Construction-Menu geladen.");
                    return;
                }

                let need_indices: Vec<usize> = tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.is_need())
                    .map(|(i, _)| i)
                    .collect();
                let infra_tab = tabs.iter().find(|t| t.is_infrastructure()).cloned();
                let tab_idx = self.buildmenu_tab.unwrap_or(0);
                let need_mode = self.buildmenu_infra_entry.is_none()
                    && tabs.get(tab_idx).map(|t| t.is_need()).unwrap_or(false);

                // Resolve content entries for need / infra-category expand.
                let content_entries: Vec<MenuEntry> = if let Some(ei) = self.buildmenu_infra_entry {
                    if let Some(infra) = infra_tab.as_ref() {
                        match infra.entries.get(ei) {
                            Some(MenuEntry::Category { entries, .. }) => entries.clone(),
                            _ => Vec::new(),
                        }
                    } else {
                        Vec::new()
                    }
                } else if need_mode {
                    tabs.get(tab_idx).map(|t| t.entries.clone()).unwrap_or_default()
                } else {
                    Vec::new()
                };

                // —— Chain / nested popup ABOVE main strip ——
                let chain_buildings: Option<Vec<u32>> =
                    self.buildmenu_entry.and_then(|ei| match content_entries.get(ei) {
                        Some(MenuEntry::Chain { buildings, .. }) => Some(buildings.clone()),
                        _ => None,
                    });
                if let Some(buildings) = chain_buildings.as_ref() {
                    let row_h = 60.0_f32;
                    let (row_rect, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), row_h),
                        Sense::hover(),
                    );
                    if let Some(frame) = self.ui_icons.chrome(ctx, chrome::CHAIN_FRAME) {
                        ui.painter().image(
                            frame.id(),
                            row_rect.shrink2(Vec2::new(8.0, 2.0)),
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                    }
                    ui.allocate_ui_at_rect(row_rect, |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.add_space(16.0);
                            for &bg in buildings {
                                let lbl = self.catalog.label(bg);
                                let icon = self
                                    .buildmenu
                                    .building_icon(bg)
                                    .map(|s| s.to_string());
                                let sel = self.placing_guid == Some(bg);
                                if self
                                    .goods_tile(ui, ctx, icon.as_deref(), sel, &lbl)
                                    .clicked()
                                {
                                    self.activate_menu_building(bg, &lbl);
                                }
                            }
                        });
                    });
                } else if let Some(ei) = self.buildmenu_entry {
                    if let Some(MenuEntry::Category { entries: nested, .. }) =
                        content_entries.get(ei)
                    {
                        ui.horizontal(|ui| {
                            ui.add_space(12.0);
                            for nest in nested {
                                let tip = nest.label();
                                let sel = matches!(
                                    nest,
                                    MenuEntry::Building { guid, .. }
                                        if self.placing_guid == Some(*guid)
                                );
                                if self.goods_tile(ui, ctx, nest.icon(), sel, &tip).clicked() {
                                    match nest {
                                        MenuEntry::Building { guid, .. } => {
                                            let lbl = self.catalog.label(*guid);
                                            self.activate_menu_building(*guid, &lbl);
                                        }
                                        MenuEntry::Chain { buildings, .. } => {
                                            if let Some(&bg) = buildings.first() {
                                                let lbl = self.catalog.label(bg);
                                                self.activate_menu_building(bg, &lbl);
                                            }
                                        }
                                        MenuEntry::Category { .. } => {}
                                    }
                                }
                            }
                        });
                    }
                }

                ui.add_space(2.0);

                // —— Main chrome strip (inline like ingame) ——
                ui.horizontal_centered(|ui| {
                    ui.add_space(6.0);

                    // Access / crane: switches to infrastructure strip.
                    let crane_sel = !need_mode;
                    if self
                        .icon_tile(
                            ui,
                            ctx,
                            Some(ICON_CRANE),
                            crane_sel && self.buildmenu_infra_entry.is_none(),
                            Vec2::new(52.0, 72.0),
                            "Infrastruktur",
                        )
                        .clicked()
                    {
                        self.buildmenu_infra_entry = None;
                        // Pick first non-need tab index if any, else stay.
                        if let Some(i) = tabs.iter().position(|t| t.is_infrastructure()) {
                            self.buildmenu_tab = Some(i);
                        }
                        self.buildmenu_entry = None;
                        self.placing_guid = None;
                        self.status = "Infrastruktur".into();
                    }

                    // Also show buildings access icon (menu root).
                    let _ = self.icon_tile(
                        ui,
                        ctx,
                        Some(ICON_ACCESS_BUILDINGS),
                        need_mode,
                        Vec2::new(48.0, 68.0),
                        "Gebäude",
                    );

                    ui.add_space(4.0);

                    if need_mode {
                        // Active portrait first, then goods, then other portraits.
                        let active = tab_idx;
                        if let Some(t) = tabs.get(active) {
                            let _ = self.portrait_tile(
                                ui,
                                ctx,
                                t.category_icon.as_deref(),
                                t.secondary_icon.as_deref(),
                                true,
                                &t.short_tier_label(),
                            );
                        }

                        ui.add_space(4.0);

                        egui::ScrollArea::horizontal()
                            .id_salt("buildmenu_goods_inline")
                            .max_height(72.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    for (ei, entry) in content_entries.iter().enumerate() {
                                        let sel = self.buildmenu_entry == Some(ei);
                                        let tip = entry.label();
                                        if self
                                            .goods_tile(ui, ctx, entry.icon(), sel, &tip)
                                            .clicked()
                                        {
                                            self.buildmenu_entry = Some(ei);
                                            match entry {
                                                MenuEntry::Building { guid, .. } => {
                                                    let lbl = self.catalog.label(*guid);
                                                    self.activate_menu_building(*guid, &lbl);
                                                }
                                                MenuEntry::Chain { .. }
                                                | MenuEntry::Category { .. } => {
                                                    self.placing_guid = None;
                                                    self.road_brush_guid = None;
                                                }
                                            }
                                        }
                                    }
                                    // Empty dark slots like ingame unlock placeholders.
                                    for _ in 0..3 {
                                        let (r, _) =
                                            ui.allocate_exact_size(Vec2::splat(58.0), Sense::hover());
                                        if let Some(frame) =
                                            self.ui_icons.chrome(ctx, chrome::CHAIN_NORMAL)
                                        {
                                            ui.painter().image(
                                                frame.id(),
                                                r,
                                                Rect::from_min_max(
                                                    Pos2::ZERO,
                                                    Pos2::new(1.0, 1.0),
                                                ),
                                                Color32::from_rgba_unmultiplied(
                                                    255, 255, 255, 80,
                                                ),
                                            );
                                        }
                                    }
                                });
                            });

                        ui.add_space(6.0);

                        for &i in &need_indices {
                            if i == active {
                                continue;
                            }
                            let t = &tabs[i];
                            if self
                                .portrait_tile(
                                    ui,
                                    ctx,
                                    t.category_icon.as_deref(),
                                    t.secondary_icon.as_deref(),
                                    false,
                                    &t.short_tier_label(),
                                )
                                .clicked()
                            {
                                self.buildmenu_tab = Some(i);
                                self.buildmenu_entry = None;
                                self.buildmenu_infra_entry = None;
                                self.road_brush_guid = None;
                            }
                        }
                    } else {
                        // Infrastructure strip + all portraits on the right.
                        if let Some(infra) = infra_tab.as_ref() {
                            egui::ScrollArea::horizontal()
                                .id_salt("buildmenu_infra_strip")
                                .max_height(80.0)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        for (ei, entry) in infra.entries.iter().enumerate() {
                                            let sel = self.buildmenu_infra_entry == Some(ei);
                                            let tip = entry.label();
                                            // Nested category content uses goods row via infra_entry.
                                            let show_goods = matches!(
                                                entry,
                                                MenuEntry::Category { .. }
                                            ) && sel
                                                && !content_entries.is_empty();
                                            if self
                                                .icon_tile(
                                                    ui,
                                                    ctx,
                                                    entry.icon(),
                                                    sel,
                                                    Vec2::new(48.0, 72.0),
                                                    &tip,
                                                )
                                                .clicked()
                                            {
                                                match entry {
                                                    MenuEntry::Building { guid, .. } => {
                                                        self.buildmenu_infra_entry = Some(ei);
                                                        self.buildmenu_entry = None;
                                                        let lbl = self.catalog.label(*guid);
                                                        self.activate_menu_building(*guid, &lbl);
                                                    }
                                                    MenuEntry::Category { .. } => {
                                                        self.buildmenu_infra_entry = Some(ei);
                                                        self.buildmenu_entry = None;
                                                        self.placing_guid = None;
                                                        self.road_brush_guid = None;
                                                        self.status = format!("Kategorie: {tip}");
                                                    }
                                                    MenuEntry::Chain { buildings, .. } => {
                                                        self.buildmenu_infra_entry = Some(ei);
                                                        self.buildmenu_entry = None;
                                                        if let Some(&bg) = buildings.first() {
                                                            let lbl = self.catalog.label(bg);
                                                            self.activate_menu_building(bg, &lbl);
                                                        }
                                                    }
                                                }
                                            }
                                            let _ = show_goods;
                                        }
                                    });
                                });
                        }

                        // If an infra category is open, show its entries inline after the strip.
                        if self.buildmenu_infra_entry.is_some() && !content_entries.is_empty() {
                            ui.add_space(6.0);
                            for (ei, entry) in content_entries.iter().enumerate() {
                                let sel = self.buildmenu_entry == Some(ei);
                                let tip = entry.label();
                                if self.goods_tile(ui, ctx, entry.icon(), sel, &tip).clicked() {
                                    self.buildmenu_entry = Some(ei);
                                    if let MenuEntry::Building { guid, .. } = entry {
                                        let lbl = self.catalog.label(*guid);
                                        self.activate_menu_building(*guid, &lbl);
                                    }
                                }
                            }
                        }

                        ui.add_space(10.0);

                        for &i in &need_indices {
                            let t = &tabs[i];
                            if self
                                .portrait_tile(
                                    ui,
                                    ctx,
                                    t.category_icon.as_deref(),
                                    t.secondary_icon.as_deref(),
                                    false,
                                    &t.short_tier_label(),
                                )
                                .clicked()
                            {
                                self.buildmenu_tab = Some(i);
                                self.buildmenu_entry = None;
                                self.buildmenu_infra_entry = None;
                                self.road_brush_guid = None;
                            }
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(8.0);
                        if let Some(g) = self.placing_guid {
                            ui.label(
                                RichText::new(self.catalog.label(g))
                                    .size(12.0)
                                    .color(Color32::from_rgb(200, 185, 140)),
                            );
                        } else if let Some(r) = self.road_brush_guid {
                            ui.label(
                                RichText::new(if r == ROAD_GUID_DIRT {
                                    "Weg: Dirt"
                                } else {
                                    "Weg: Ziegel"
                                })
                                .size(12.0)
                                .color(Color32::from_rgb(200, 180, 120)),
                            );
                        }
                        ui.label(
                            RichText::new(self.region_filter.label())
                                .size(11.0)
                                .color(Color32::from_rgb(140, 150, 165)),
                        );
                    });
                });
            });

'''

path.write_text(text[:start] + new + text[end:], encoding="utf-8")
print("patched", len(new))

