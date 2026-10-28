//! The Content Browser panel, laid out like Unreal's Content Browser and Unity's two-column
//! Project window: sources on the left (favorites, the folder trees of assets, prefabs and
//! scenes, shapes and module demos), a bar of history, breadcrumbs, search and type filters,
//! a grid of thumbnail tiles, and a details column for the selected asset that says where it
//! is used. What it does to files lives in [`crate::content`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use engine::prelude::*;

use crate::content::{self, Asset, Kind, Query, Root, Usage};
use crate::demos::DEMOS;
use crate::editor::{self, Create, Editor};
use crate::models::Dragged;
use crate::theme;
use crate::thumbs::Thumbs;

/// How long a folder listing or the search index is trusted before the disk is read again.
const RELIST: Duration = Duration::from_secs(2);
/// Most search results shown.
const MAX_RESULTS: usize = 400;

/// What the asset view shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Files,
    Shapes,
    Demos,
}

/// A file or folder as listed.
#[derive(Debug, Clone)]
struct Entry {
    asset: Asset,
    kind: Kind,
    size: u64,
    modified: Option<SystemTime>,
}

impl Entry {
    fn read(asset: Asset, assets: &Path) -> Self {
        let meta = std::fs::metadata(asset.path(assets)).ok();
        let kind = if meta.as_ref().is_some_and(|m| m.is_dir()) || asset.rel.is_empty() {
            Kind::Folder
        } else {
            content::kind_of(asset.name())
        };
        Self {
            asset,
            kind,
            size: meta.as_ref().map_or(0, |m| m.len()),
            modified: meta.and_then(|m| m.modified().ok()),
        }
    }
}

/// The Content Browser's state.
#[derive(Default)]
pub struct Browser {
    pub source: Source,
    /// The folder shown (the asset root when `None`).
    pub folder: Option<Asset>,
    back: Vec<Asset>,
    forward: Vec<Asset>,
    pub search: String,
    /// Type filter chips that are on.
    pub types: Vec<Kind>,
    pub selected: Option<Asset>,
    favorites: Option<Vec<Asset>>,
    /// The asset being renamed and the name typed so far.
    pub renaming: Option<(Asset, String)>,
    rename_focus: bool,
    /// The asset a delete waits on, with what still uses it.
    pub deleting: Option<(Asset, Usage)>,
    usage: Option<(Asset, Instant, Usage)>,
    listing: Option<(Asset, Instant, Vec<Entry>)>,
    index: Option<(Instant, Vec<(Asset, Kind)>)>,
    subfolders: HashMap<Asset, (Instant, Vec<Asset>)>,
    /// When each thumbnail's file last changed, so an edited prefab gets a new picture.
    seen: HashMap<String, Option<SystemTime>>,
    /// Where tiles and folder rows were drawn last frame (for the screenshot scripts).
    pub tile_rects: HashMap<Asset, egui::Rect>,
    pub folder_rects: HashMap<Asset, egui::Rect>,
    pub save_as: String,
}

impl Browser {
    pub fn folder(&self) -> Asset {
        self.folder
            .clone()
            .unwrap_or_else(|| Asset::new(Root::Assets, ""))
    }

    /// Show `folder`, remembering where we were for Back.
    pub fn go(&mut self, folder: Asset) {
        let here = self.folder();
        if here != folder || self.source != Source::Files {
            self.back.push(here);
            self.forward.clear();
        }
        self.folder = Some(folder);
        self.source = Source::Files;
        self.search.clear();
    }

    /// Read the disk again next frame.
    pub fn refresh(&mut self) {
        self.listing = None;
        self.index = None;
        self.usage = None;
        self.subfolders.clear();
    }

    pub fn favorites(&mut self) -> &mut Vec<Asset> {
        self.favorites.get_or_insert_with(content::load_favorites)
    }

    fn entries(&mut self, assets: &Path) -> Vec<Entry> {
        let folder = self.folder();
        let stale = match &self.listing {
            Some((f, at, _)) => *f != folder || at.elapsed() > RELIST,
            None => true,
        };
        if stale {
            let entries = content::list(&folder, assets)
                .into_iter()
                .map(|a| Entry::read(a, assets))
                .collect();
            self.listing = Some((folder, Instant::now(), entries));
        }
        self.listing
            .as_ref()
            .map(|l| l.2.clone())
            .unwrap_or_default()
    }

    fn search_results(&mut self, assets: &Path) -> Vec<Entry> {
        if self
            .index
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > RELIST)
        {
            self.index = Some((Instant::now(), content::index(assets)));
        }
        let mut query = Query::parse(&self.search);
        if query.kinds.is_empty() {
            query.kinds = self.types.clone();
        }
        self.index
            .as_ref()
            .map(|(_, all)| {
                all.iter()
                    .filter(|(a, k)| query.matches(a, *k))
                    .take(MAX_RESULTS)
                    .map(|(a, _)| Entry::read(a.clone(), assets))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn subfolders(&mut self, folder: &Asset, assets: &Path) -> Vec<Asset> {
        if let Some((at, list)) = self.subfolders.get(folder)
            && at.elapsed() < RELIST
        {
            return list.clone();
        }
        let list: Vec<Asset> = content::list(folder, assets)
            .into_iter()
            .filter(|a| a.path(assets).is_dir())
            .collect();
        self.subfolders
            .insert(folder.clone(), (Instant::now(), list.clone()));
        list
    }
}

/// Something the panel asked for, done once drawing is over.
pub enum Act {
    Select(Asset),
    Open(Asset),
    Go(Asset),
    /// Show the asset in its folder, selected.
    Reveal(Asset),
    Back,
    Forward,
    Rename(Asset, String),
    Duplicate(Asset),
    AskDelete(Asset),
    Delete(Asset),
    Favorite(Asset),
    Move(Asset, Asset),
    NewFolder(Asset),
    SelectUsers(Asset),
    Create(Create),
    Import(String),
    SaveAs(String),
    NewScene,
}

fn asset_root(world: &World) -> PathBuf {
    world
        .get_resource::<engine::assets::AssetServer>()
        .map(|s| s.root().to_path_buf())
        .unwrap_or_default()
}

/// The Content tab.
pub fn content(ui: &mut egui::Ui, world: &mut World, editor: &mut Editor) {
    let assets = asset_root(world);
    let _ = crate::thumbs::ASSET_ROOT.set(assets.clone());
    let mut thumbs = world.remove_resource::<Thumbs>().unwrap_or_default();
    thumbs.receive(ui.ctx());
    let mut b = std::mem::take(&mut editor.browser);
    b.tile_rects.clear();
    b.folder_rects.clear();
    let mut acts = Vec::new();

    egui::Panel::left("content sources")
        .resizable(true)
        .default_size(190.0)
        .size_range(140.0..=360.0)
        .show(ui, |ui| sources(ui, &mut b, &assets, &mut acts));
    if b.source == Source::Files
        && let Some(selected) = b.selected.clone()
        && selected.path(&assets).exists()
    {
        egui::Panel::right("content details")
            .resizable(true)
            .default_size(230.0)
            .size_range(180.0..=420.0)
            .show(ui, |ui| {
                details(
                    ui,
                    world,
                    editor,
                    &mut b,
                    &mut thumbs,
                    &assets,
                    &selected,
                    &mut acts,
                )
            });
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(6, 2)))
        .show(ui, |ui| {
            top_bar(ui, editor, &mut b, &mut thumbs, &mut acts);
            ui.separator();
            match b.source {
                Source::Files => files(ui, &mut b, &mut thumbs, &assets, &mut acts),
                Source::Shapes => shapes(ui, thumbs.tile, &mut acts),
                Source::Demos => demos(ui, thumbs.tile, &mut acts),
            }
        });
    delete_dialog(ui.ctx(), &mut b, &mut acts);

    editor.browser = b;
    world.insert_resource(thumbs);
    for act in acts {
        run(world, editor, &assets, act);
    }
}

// ----------------------------------------------------------------------------- sources

fn sources(ui: &mut egui::Ui, b: &mut Browser, assets: &Path, acts: &mut Vec<Act>) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(2.0);
            ui.label(egui::RichText::new("★ Favorites").strong());
            let favorites = b.favorites().clone();
            if favorites.is_empty() {
                ui.label(theme::dim("Right-click an asset: Add to favorites"));
            }
            for f in favorites {
                let kind = Entry::read(f.clone(), assets).kind;
                let here = b.source == Source::Files
                    && (b.selected.as_ref() == Some(&f)
                        || (kind == Kind::Folder && b.folder() == f));
                let r = ui
                    .add(egui::Button::selectable(
                        here,
                        egui::RichText::new(format!("{}  {}", kind.icon(), f.title())),
                    ))
                    .on_hover_text(f.key());
                if r.clicked() {
                    acts.push(if kind == Kind::Folder {
                        Act::Go(f.clone())
                    } else {
                        Act::Reveal(f.clone())
                    });
                }
                r.context_menu(|ui| {
                    if ui.button("Remove from favorites").clicked() {
                        acts.push(Act::Favorite(f.clone()));
                        ui.close();
                    }
                });
            }
            ui.add_space(6.0);
            for root in [Root::Assets, Root::Prefabs, Root::Scenes] {
                folder_row(ui, b, assets, Asset::new(root, ""), 0, acts);
            }
            ui.add_space(6.0);
            ui.separator();
            for (source, label) in [
                (Source::Shapes, "◼  Shapes and lights"),
                (Source::Demos, "💠  Module demos"),
            ] {
                if ui
                    .add(egui::Button::selectable(b.source == source, label))
                    .clicked()
                {
                    b.source = source;
                }
            }
        });
}

/// A folder in the tree: click to show it, drop an asset on it to move the asset there.
fn folder_row(
    ui: &mut egui::Ui,
    b: &mut Browser,
    assets: &Path,
    folder: Asset,
    depth: usize,
    acts: &mut Vec<Act>,
) {
    let children = b.subfolders(&folder, assets);
    let current = b.source == Source::Files && b.search.is_empty() && b.folder() == folder;
    // Open down to the folder shown.
    let shown = b.folder();
    let leads_to_shown = shown.root == folder.root
        && (folder.rel.is_empty() || shown.rel.starts_with(&format!("{}/", folder.rel)));
    let label = if folder.rel.is_empty() {
        egui::RichText::new(format!("📁  {}", content::root_title(folder.root))).strong()
    } else {
        egui::RichText::new(format!("📁  {}", folder.name()))
    };
    let mut row = |ui: &mut egui::Ui| {
        let r = ui.add(egui::Button::selectable(current, label.clone()));
        drop_target(ui, &r, &folder, acts);
        if r.clicked() {
            acts.push(Act::Go(folder.clone()));
        }
        r.context_menu(|ui| {
            if ui.button("New folder").clicked() {
                acts.push(Act::NewFolder(folder.clone()));
                ui.close();
            }
            if !folder.rel.is_empty() && ui.button("Add to favorites").clicked() {
                acts.push(Act::Favorite(folder.clone()));
                ui.close();
            }
        });
        b.folder_rects.insert(folder.clone(), r.rect);
    };
    if children.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            row(ui);
        });
        return;
    }
    let id = ui.make_persistent_id(("content folder", &folder));
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        depth == 0 && folder.root == Root::Assets,
    );
    if leads_to_shown && !current {
        state.set_open(true);
    }
    state.show_header(ui, |ui| row(ui)).body(|ui| {
        for child in children {
            folder_row(ui, b, assets, child, depth + 1, acts);
        }
    });
}

/// What is being dragged, as an asset (tiles drag models and prefabs as [`Dragged`], so the
/// scene view can take them too).
fn dragged_asset(r: &egui::Response, release: bool) -> Option<Asset> {
    let from_dragged = |d: &Dragged| match d {
        Dragged::Model(key) => Some(Asset::new(Root::Assets, key.clone())),
        Dragged::Prefab(path) => path
            .strip_prefix(&format!("{}/", crate::prefab::PREFABS_DIR))
            .map(|rel| Asset::new(Root::Prefabs, rel)),
    };
    if release {
        r.dnd_release_payload::<Asset>()
            .map(|a| (*a).clone())
            .or_else(|| {
                r.dnd_release_payload::<Dragged>()
                    .and_then(|d| from_dragged(&d))
            })
    } else {
        r.dnd_hover_payload::<Asset>()
            .map(|a| (*a).clone())
            .or_else(|| {
                r.dnd_hover_payload::<Dragged>()
                    .and_then(|d| from_dragged(&d))
            })
    }
}

/// Folders take assets dropped on them: they move there.
fn drop_target(ui: &egui::Ui, r: &egui::Response, folder: &Asset, acts: &mut Vec<Act>) {
    if let Some(a) = dragged_asset(r, false)
        && a.root == folder.root
        && a != *folder
    {
        ui.painter().rect_stroke(
            r.rect.expand(1.0),
            3.0,
            egui::Stroke::new(1.5, theme::ACCENT),
            egui::StrokeKind::Outside,
        );
    }
    if let Some(a) = dragged_asset(r, true)
        && a != *folder
        && a.parent() != *folder
    {
        acts.push(Act::Move(a, folder.clone()));
    }
}

// ----------------------------------------------------------------------------- top bar

fn top_bar(
    ui: &mut egui::Ui,
    editor: &mut Editor,
    b: &mut Browser,
    thumbs: &mut Thumbs,
    acts: &mut Vec<Act>,
) {
    // Row 1: history, breadcrumbs, and what can be made here.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        if ui
            .add_enabled(!b.back.is_empty(), egui::Button::new("⬅"))
            .on_hover_text("Back")
            .clicked()
        {
            acts.push(Act::Back);
        }
        if ui
            .add_enabled(!b.forward.is_empty(), egui::Button::new("➡"))
            .on_hover_text("Forward")
            .clicked()
        {
            acts.push(Act::Forward);
        }
        let folder = b.folder();
        let files = b.source == Source::Files;
        if ui
            .add_enabled(files && !folder.rel.is_empty(), egui::Button::new("⬆"))
            .on_hover_text("Up a folder")
            .clicked()
        {
            acts.push(Act::Go(folder.parent()));
        }
        ui.add_space(6.0);
        // Breadcrumbs (each takes dropped assets too).
        match b.source {
            Source::Shapes => {
                ui.label(egui::RichText::new("Shapes and lights").strong());
            }
            Source::Demos => {
                ui.label(egui::RichText::new("Module demos").strong());
            }
            Source::Files if !b.search.trim().is_empty() => {
                ui.label(egui::RichText::new("Search results").strong());
            }
            Source::Files => {
                let mut crumb = Asset::new(folder.root, "");
                let mut parts = vec![(crumb.clone(), content::root_title(folder.root).to_string())];
                for seg in folder.rel.split('/').filter(|s| !s.is_empty()) {
                    crumb = crumb.child(seg);
                    parts.push((crumb.clone(), seg.to_string()));
                }
                let last = parts.len() - 1;
                for (i, (at, name)) in parts.into_iter().enumerate() {
                    if i > 0 {
                        ui.label(theme::dim("/"));
                    }
                    let text = if i == last {
                        egui::RichText::new(name).strong()
                    } else {
                        egui::RichText::new(name)
                    };
                    let r = ui.add(egui::Button::new(text).frame(false));
                    drop_target(ui, &r, &at, acts);
                    if r.clicked() && i != last {
                        acts.push(Act::Go(at));
                    }
                }
            }
        }
        if !files {
            return;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .small_button("🔄")
                .on_hover_text("Read the folders again")
                .clicked()
            {
                b.refresh();
                editor.models = None;
            }
            if ui
                .button("➕ Folder")
                .on_hover_text("Make a folder here")
                .clicked()
            {
                acts.push(Act::NewFolder(folder.clone()));
            }
            match folder.root {
                Root::Assets => {
                    ui.menu_button("📥 Import", |ui| {
                        ui.label("A model, an image, or a folder of texture sets:");
                        let f = ui.add(
                            egui::TextEdit::singleline(&mut editor.import_path)
                                .hint_text("path on this computer")
                                .desired_width(320.0),
                        );
                        let enter = f.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let path = editor.import_path.trim().trim_matches('"').to_string();
                        if (ui.button("Import and place").clicked() || enter) && !path.is_empty() {
                            acts.push(Act::Import(path));
                            editor.import_path.clear();
                            ui.close();
                        }
                        ui.label(theme::dim("Files dropped on the window are imported too"));
                    });
                }
                Root::Scenes => {
                    if ui.button("New scene").clicked() {
                        acts.push(Act::NewScene);
                    }
                    ui.menu_button("💾 Save as", |ui| {
                        let f = ui.add(
                            egui::TextEdit::singleline(&mut b.save_as)
                                .hint_text("scene name")
                                .desired_width(180.0),
                        );
                        let enter = f.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (ui.button("Save").clicked() || enter) && !b.save_as.trim().is_empty() {
                            acts.push(Act::SaveAs(std::mem::take(&mut b.save_as)));
                            ui.close();
                        }
                    });
                }
                Root::Prefabs => {}
            }
        });
    });
    // Row 2: search, type filters, tile size.
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if b.source != Source::Files {
            ui.label(theme::dim("Click to place where the view is looking"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                tile_slider(ui, thumbs);
            });
            return;
        }
        // Unreal's Filters menu: the kinds that are on show as chips beside it.
        let label = if b.types.is_empty() {
            "Filter ⏷".to_string()
        } else {
            format!("Filter ({}) ⏷", b.types.len())
        };
        ui.menu_button(label, |ui| {
            for (kind, name) in Kind::FILTERS {
                let mut on = b.types.contains(&kind);
                let text = egui::RichText::new(format!("{}  {name}", kind.icon()));
                if ui.checkbox(&mut on, text).changed() {
                    if on {
                        b.types.push(kind);
                    } else {
                        b.types.retain(|k| *k != kind);
                    }
                }
            }
            ui.separator();
            if ui.button("Show everything").clicked() {
                b.types.clear();
                ui.close();
            }
        });
        for kind in b.types.clone() {
            let name = Kind::FILTERS
                .iter()
                .find(|(k, _)| *k == kind)
                .map_or("", |(_, n)| *n);
            let chip = egui::Button::new(
                egui::RichText::new(format!("{name}  ✖")).color(egui::Color32::WHITE),
            )
            .fill(kind.color().gamma_multiply(0.45))
            .stroke(egui::Stroke::new(1.0, kind.color()))
            .corner_radius(9.0);
            if ui.add(chip).on_hover_text("Click to remove").clicked() {
                b.types.retain(|k| *k != kind);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            tile_slider(ui, thumbs);
            if thumbs.busy() {
                ui.spinner().on_hover_text("Drawing thumbnails");
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut b.search)
                    .hint_text("🔍 Search everything (t:model, t:prefab...)")
                    .desired_width(ui.available_width().max(80.0)),
            );
            if r.changed() && !b.search.is_empty() {
                b.source = Source::Files;
            }
        });
    });
}

fn tile_slider(ui: &mut egui::Ui, thumbs: &mut Thumbs) {
    ui.add(
        egui::Slider::new(&mut thumbs.tile, 48.0..=160.0)
            .show_value(false)
            .trailing_fill(true),
    )
    .on_hover_text("Tile size");
}

// ----------------------------------------------------------------------------- tiles

/// Lay out `n` tiles of `tile` points in rows that fill the width.
fn tile_rows(ui: &mut egui::Ui, tile: f32, n: usize, mut add: impl FnMut(&mut egui::Ui, usize)) {
    let gap = 6.0;
    let per_row = ((ui.available_width() + gap) / (tile + 8.0 + gap)).max(1.0) as usize;
    for start in (0..n).step_by(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
            for i in start..(start + per_row).min(n) {
                add(ui, i);
            }
        });
    }
}

fn files(
    ui: &mut egui::Ui,
    b: &mut Browser,
    thumbs: &mut Thumbs,
    assets: &Path,
    acts: &mut Vec<Act>,
) {
    let searching = !b.search.trim().is_empty();
    let mut entries = if searching {
        b.search_results(assets)
    } else {
        b.entries(assets)
    };
    if !searching && !b.types.is_empty() {
        entries.retain(|e| b.types.contains(&e.kind));
    }
    for e in &entries {
        let key = e.asset.key();
        if b.seen.get(&key).is_some_and(|m| *m != e.modified) {
            thumbs.forget(&key);
        }
        b.seen.insert(key, e.modified);
    }
    if searching {
        ui.label(theme::dim(format!(
            "{} found in Assets, Prefabs and Scenes{}",
            entries.len(),
            if entries.len() >= MAX_RESULTS {
                " (first ones)"
            } else {
                ""
            }
        )));
    }
    // A click on empty space clears the selection; a right-click offers a new folder. (Made
    // before the tiles, so they are on top of it.)
    let back = ui.interact(
        ui.available_rect_before_wrap(),
        ui.id().with("content background"),
        egui::Sense::click(),
    );
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if entries.is_empty() {
                ui.add_space(12.0);
                ui.vertical_centered(|ui| {
                    ui.label(theme::dim(if searching || !b.types.is_empty() {
                        "Nothing matches"
                    } else if b.folder().root == Root::Assets && b.folder().rel.is_empty() {
                        "No assets yet: import a model, or cargo run -p fetch-assets"
                    } else {
                        "This folder is empty"
                    }));
                });
            }
            let tile = thumbs.tile;
            tile_rows(ui, tile, entries.len(), |ui, i| {
                file_tile(ui, b, thumbs, assets, &entries[i], tile, searching, acts);
            });
        });
    if back.clicked() {
        b.selected = None;
    }
    let folder = b.folder();
    back.context_menu(|ui| {
        if ui.button("New folder").clicked() {
            acts.push(Act::NewFolder(folder.clone()));
            ui.close();
        }
    });
}

/// Paint a tile: the picture (or a big icon), the type strip and the name.
#[allow(clippy::too_many_arguments)]
fn paint_tile(
    ui: &egui::Ui,
    rect: egui::Rect,
    r: &egui::Response,
    tile: f32,
    picture: Option<&egui::TextureHandle>,
    icon: &str,
    color: egui::Color32,
    name: &str,
    selected: bool,
    show_name: bool,
) {
    let p = ui.painter();
    let fill = if selected {
        theme::ACCENT_DIM
    } else if r.hovered() {
        theme::HOVER
    } else {
        theme::WIDGET
    };
    p.rect_filled(rect, 4.0, fill);
    if selected {
        p.rect_stroke(
            rect,
            4.0,
            egui::Stroke::new(1.5, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    let pic = egui::Rect::from_min_size(rect.min + egui::vec2(4.0, 4.0), egui::vec2(tile, tile));
    match picture {
        Some(t) => {
            egui::Image::new(t).paint_at(ui, pic);
        }
        None => {
            p.text(
                pic.center(),
                egui::Align2::CENTER_CENTER,
                icon,
                egui::FontId::proportional(tile * 0.42),
                color,
            );
        }
    }
    // The type strip under the picture, as Unreal colors its tiles.
    p.rect_filled(
        egui::Rect::from_min_size(
            egui::pos2(rect.min.x + 4.0, pic.max.y + 1.0),
            egui::vec2(tile, 2.5),
        ),
        1.0,
        color,
    );
    if show_name {
        let mut job = egui::text::LayoutJob::simple_singleline(
            name.to_string(),
            egui::FontId::proportional(11.5),
            if selected {
                egui::Color32::WHITE
            } else {
                theme::TEXT
            },
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(tile + 2.0);
        let galley = p.layout_job(job);
        let at = egui::pos2(rect.center().x - galley.size().x / 2.0, pic.max.y + 6.0);
        p.galley(at, galley, theme::TEXT);
    }
}

fn tile_size(tile: f32) -> egui::Vec2 {
    egui::vec2(tile + 8.0, tile + 26.0)
}

#[allow(clippy::too_many_arguments)]
fn file_tile(
    ui: &mut egui::Ui,
    b: &mut Browser,
    thumbs: &mut Thumbs,
    assets: &Path,
    e: &Entry,
    tile: f32,
    searching: bool,
    acts: &mut Vec<Act>,
) {
    let a = &e.asset;
    let key = a.key();
    let picture = match e.kind {
        Kind::Model | Kind::Material | Kind::Texture | Kind::Prefab => {
            thumbs.get(&key, &a.path(assets)).cloned()
        }
        _ => None,
    };
    let icon = if e.kind == Kind::Model && thumbs.is_clip(&key) {
        "🎬"
    } else {
        e.kind.icon()
    };
    let selected = b.selected.as_ref() == Some(a);
    let renaming = b.renaming.as_ref().is_some_and(|(r, _)| r == a);
    // One widget takes clicks, double-clicks, right-clicks and drags (a drag source wrapped
    // around a button would split them between two widgets).
    let (rect, r) = ui.allocate_exact_size(tile_size(tile), egui::Sense::click_and_drag());
    paint_tile(
        ui,
        rect,
        &r,
        tile,
        picture.as_ref(),
        icon,
        e.kind.color(),
        &a.title(),
        selected,
        !renaming,
    );
    match e.kind {
        Kind::Model | Kind::Material => r.dnd_set_drag_payload(Dragged::Model(key.clone())),
        Kind::Prefab => r.dnd_set_drag_payload(Dragged::Prefab(key.clone())),
        _ => r.dnd_set_drag_payload(a.clone()),
    }
    if r.dragged()
        && let Some(at) = ui.ctx().pointer_interact_pos()
    {
        // What is being carried, under the pointer.
        let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("content drag"));
        let p = ui.ctx().layer_painter(layer);
        let small =
            egui::Rect::from_center_size(at + egui::vec2(24.0, 24.0), egui::vec2(44.0, 44.0));
        p.rect_filled(small, 4.0, theme::WIDGET.gamma_multiply(0.9));
        match &picture {
            Some(t) => {
                p.image(
                    t.id(),
                    small.shrink(3.0),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                p.text(
                    small.center(),
                    egui::Align2::CENTER_CENTER,
                    icon,
                    egui::FontId::proportional(24.0),
                    e.kind.color(),
                );
            }
        }
    }
    b.tile_rects.insert(a.clone(), r.rect);
    if e.kind == Kind::Folder {
        drop_target(ui, &r, a, acts);
    }
    if renaming {
        rename_field(ui, b, r.rect, tile, acts);
    }
    let r = r.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(a.title()).strong());
        ui.label(theme::dim(&key));
        ui.label(hint(e.kind));
    });
    if r.clicked() {
        acts.push(Act::Select(a.clone()));
    }
    if r.double_clicked() {
        acts.push(Act::Open(a.clone()));
    }
    let favorite = b.favorites().contains(a);
    r.context_menu(|ui| {
        ui.set_min_width(190.0);
        if let Some(open) = open_label(e.kind)
            && ui.button(open).clicked()
        {
            acts.push(Act::Open(a.clone()));
            ui.close();
        }
        if searching && ui.button("Show in folder").clicked() {
            acts.push(Act::Reveal(a.clone()));
            ui.close();
        }
        ui.separator();
        if ui.button("Rename").clicked() {
            b.renaming = Some((a.clone(), a.name().to_string()));
            b.rename_focus = true;
            ui.close();
        }
        if ui.button("Duplicate").clicked() {
            acts.push(Act::Duplicate(a.clone()));
            ui.close();
        }
        if ui.button("Delete...").clicked() {
            acts.push(Act::AskDelete(a.clone()));
            ui.close();
        }
        ui.separator();
        let fav = if favorite {
            "Remove from favorites"
        } else {
            "Add to favorites"
        };
        if ui.button(fav).clicked() {
            acts.push(Act::Favorite(a.clone()));
            ui.close();
        }
        if e.kind != Kind::Folder && ui.button("Select users in scene").clicked() {
            acts.push(Act::SelectUsers(a.clone()));
            ui.close();
        }
        if ui.button("Copy path").clicked() {
            ui.ctx().copy_text(key.clone());
            ui.close();
        }
    });
    // F2 renames the selected tile, as in Unreal, Unity and file browsers.
    if selected
        && !renaming
        && ui.rect_contains_pointer(ui.clip_rect())
        && ui.input(|i| i.key_pressed(egui::Key::F2))
    {
        b.renaming = Some((a.clone(), a.name().to_string()));
        b.rename_focus = true;
    }
}

/// The name field over a tile being renamed: Enter or a click away keeps it, Escape doesn't.
fn rename_field(
    ui: &mut egui::Ui,
    b: &mut Browser,
    rect: egui::Rect,
    tile: f32,
    acts: &mut Vec<Act>,
) {
    let Some((asset, text)) = b.renaming.as_mut() else {
        return;
    };
    let at = egui::pos2(rect.min.x, rect.min.y + tile + 6.0);
    let mut done = None;
    egui::Area::new(egui::Id::new("content rename"))
        .fixed_pos(at)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            let r = ui.add(
                egui::TextEdit::singleline(text)
                    .desired_width(rect.width().max(140.0))
                    .font(egui::TextStyle::Small),
            );
            if b.rename_focus {
                r.request_focus();
                // Select the name without its extension, as file browsers do.
                let stem = content::split_ext(text).0.chars().count();
                if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), r.id) {
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(stem),
                        )));
                    state.store(ui.ctx(), r.id);
                }
                b.rename_focus = false;
            }
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                done = Some(false);
            } else if r.lost_focus() {
                done = Some(true);
            }
        });
    match done {
        Some(true) => {
            let new = text.trim().to_string();
            if !new.is_empty() && new != asset.name() {
                acts.push(Act::Rename(asset.clone(), new));
            }
            b.renaming = None;
        }
        Some(false) => b.renaming = None,
        None => {}
    }
}

fn hint(kind: Kind) -> &'static str {
    match kind {
        Kind::Folder => "Double-click to open. Drop assets on it to move them here",
        Kind::Model => {
            "Double-click to place it where the view is looking, or drag it into the scene"
        }
        Kind::Material => "Double-click to put it on the selection, or drag it onto an object",
        Kind::Prefab => {
            "Double-click to place a copy, or drag it into the scene or onto an outliner row"
        }
        Kind::Scene => "Double-click to open the scene",
        Kind::Texture => "A texture: import its folder to make a material from it",
        Kind::Audio => "A sound",
        Kind::Other => "A file",
    }
}

fn open_label(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Folder => Some("Open"),
        Kind::Model | Kind::Prefab => Some("Place in scene"),
        Kind::Material => Some("Put on selection"),
        Kind::Scene => Some("Open scene"),
        _ => None,
    }
}

fn shapes(ui: &mut egui::Ui, tile: f32, acts: &mut Vec<Act>) {
    let mut items: Vec<(String, &str, Create)> = editor::shape_presets()
        .into_iter()
        .map(|(l, s)| (l.to_string(), "◼", Create::Shape(l, s)))
        .collect();
    items.push(("Point light".into(), "💡", Create::PointLight));
    items.push(("Sun".into(), "☀", Create::SunLight));
    items.push(("Camera".into(), "🎥", Create::Camera));
    items.push(("Empty".into(), "○", Create::Empty));
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            tile_rows(ui, tile, items.len(), |ui, i| {
                let (name, icon, what) = &items[i];
                let (rect, r) = ui.allocate_exact_size(tile_size(tile), egui::Sense::click());
                let color = egui::Color32::from_rgb(0x9a, 0xa4, 0xb4);
                paint_tile(ui, rect, &r, tile, None, icon, color, name, false, true);
                if r.on_hover_text(format!("Place a {}", name.to_lowercase()))
                    .clicked()
                {
                    acts.push(Act::Create(what.clone()));
                }
            });
        });
}

fn demos(ui: &mut egui::Ui, tile: f32, acts: &mut Vec<Act>) {
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            tile_rows(ui, tile, DEMOS.len(), |ui, i| {
                let (key, label, about, _) = DEMOS[i];
                let (rect, r) = ui.allocate_exact_size(tile_size(tile), egui::Sense::click());
                let color = egui::Color32::from_rgb(0x5a, 0xb4, 0xe8);
                paint_tile(ui, rect, &r, tile, None, "💠", color, label, false, true);
                if r.on_hover_text(format!("{label}: {about}")).clicked() {
                    acts.push(Act::Create(Create::Demo(key)));
                }
            });
        });
}

// ----------------------------------------------------------------------------- details

fn ago(t: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(t)
        .map_or(0, |d| d.as_secs());
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86400),
    }
}

fn size_text(bytes: u64) -> String {
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KB", bytes as f64 / 1024.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.0),
    }
}

#[allow(clippy::too_many_arguments)]
fn details(
    ui: &mut egui::Ui,
    world: &mut World,
    editor: &mut Editor,
    b: &mut Browser,
    thumbs: &mut Thumbs,
    assets: &Path,
    a: &Asset,
    acts: &mut Vec<Act>,
) {
    let e = Entry::read(a.clone(), assets);
    let key = a.key();
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(4.0);
            let side = ui.available_width().min(170.0);
            let picture = match e.kind {
                Kind::Model | Kind::Material | Kind::Texture | Kind::Prefab => {
                    thumbs.get(&key, &a.path(assets)).cloned()
                }
                _ => None,
            };
            ui.vertical_centered(|ui| {
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::hover());
                ui.painter().rect_filled(rect, 6.0, theme::WIDGET);
                match &picture {
                    Some(t) => {
                        egui::Image::new(t).paint_at(ui, rect.shrink(6.0));
                    }
                    None => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            e.kind.icon(),
                            egui::FontId::proportional(side * 0.4),
                            e.kind.color(),
                        );
                    }
                }
            });
            ui.add_space(4.0);
            ui.label(egui::RichText::new(a.title()).strong().size(15.0));
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(kind_name(e.kind)).color(e.kind.color()));
                if e.kind != Kind::Folder {
                    ui.label(theme::dim(format!("{}  ·", size_text(e.size))));
                }
                if let Some(m) = e.modified {
                    ui.label(theme::dim(format!("changed {}", ago(m))));
                }
            });
            ui.add(egui::Label::new(theme::dim(&key)).wrap());
            match e.kind {
                Kind::Model => {
                    let info = editor.model_info.entry(key.clone()).or_insert_with(|| {
                        crate::convert::inspect(&a.path(assets))
                            .map_or_else(|e| e, |s| s.describe())
                    });
                    ui.label(theme::dim(info.as_str()));
                }
                Kind::Texture => {
                    if let Ok((w, h)) = image::image_dimensions(a.path(assets)) {
                        ui.label(theme::dim(format!("{w} × {h} pixels")));
                    }
                }
                Kind::Prefab => {
                    if let Some(base) = crate::prefab::base_of(world, &key) {
                        ui.label(theme::dim(format!(
                            "Variant of {}",
                            crate::prefab::display_name(&base)
                        )));
                    }
                }
                Kind::Folder => {
                    let n = content::list(a, assets).len();
                    ui.label(theme::dim(format!("{n} items")));
                }
                _ => {}
            }
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                if let Some(open) = open_label(e.kind)
                    && ui.button(open).clicked()
                {
                    acts.push(Act::Open(a.clone()));
                }
                let fav = b.favorites().contains(a);
                if ui
                    .add(egui::Button::selectable(
                        fav,
                        if fav { "★ Favorite" } else { "☆ Favorite" },
                    ))
                    .clicked()
                {
                    acts.push(Act::Favorite(a.clone()));
                }
                if ui.button("Delete...").clicked() {
                    acts.push(Act::AskDelete(a.clone()));
                }
            });
            if e.kind == Kind::Folder {
                return;
            }
            ui.add_space(6.0);
            ui.separator();
            // Where it is used: Unreal's Reference Viewer and Unity's Find References In
            // Scene, in one list.
            let fresh = b
                .usage
                .as_ref()
                .is_some_and(|(u, at, _)| u == a && at.elapsed() < RELIST);
            if !fresh {
                let u = content::usage(world, a, assets);
                b.usage = Some((a.clone(), Instant::now(), u));
            }
            let Some((_, _, usage)) = b.usage.clone() else {
                return;
            };
            ui.label(egui::RichText::new("Used by").strong());
            if usage.is_empty() {
                ui.label(theme::dim(
                    "Nothing in this scene or in any scene or prefab file",
                ));
                return;
            }
            if !usage.entities.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(format!(
                        "{} in this scene",
                        plural(usage.entities.len(), "object", "objects")
                    ));
                    if ui.small_button("Select all").clicked() {
                        acts.push(Act::SelectUsers(a.clone()));
                    }
                });
                for &ent in usage.entities.iter().take(12) {
                    let name = world
                        .get::<Name>(ent)
                        .map_or_else(|| format!("{ent}"), |n| n.as_str().to_string());
                    let on = editor.is_selected(ent);
                    if ui
                        .add(egui::Button::selectable(
                            on,
                            format!("    {}  {name}", theme::entity_icon(world, ent)),
                        ))
                        .clicked()
                    {
                        editor.select(Some(ent));
                    }
                }
                if usage.entities.len() > 12 {
                    ui.label(theme::dim(format!(
                        "    and {} more",
                        usage.entities.len() - 12
                    )));
                }
            }
            if !usage.files.is_empty() {
                ui.add_space(2.0);
                ui.label(plural(usage.files.len(), "file", "files"));
                for f in &usage.files {
                    let name = f.rsplit('/').next().unwrap_or(f);
                    let kind = content::kind_of(name);
                    let r = ui
                        .add(egui::Button::selectable(
                            false,
                            format!("    {}  {name}", kind.icon()),
                        ))
                        .on_hover_text(format!("{f}\nClick to show it"));
                    if r.clicked()
                        && let Some(found) = asset_of_path(f, assets)
                    {
                        acts.push(Act::Reveal(found));
                    }
                }
            }
        });
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Folder => "Folder",
        Kind::Model => "Model",
        Kind::Material => "Material",
        Kind::Texture => "Texture",
        Kind::Prefab => "Prefab",
        Kind::Scene => "Scene",
        Kind::Audio => "Sound",
        Kind::Other => "File",
    }
}

/// The asset a file path names, under whichever root holds it.
fn asset_of_path(path: &str, assets: &Path) -> Option<Asset> {
    let path = path.replace('\\', "/");
    for root in [Root::Prefabs, Root::Scenes, Root::Assets] {
        let dir = Asset::root_dir(root, assets)
            .to_string_lossy()
            .replace('\\', "/");
        if let Some(rel) = path.strip_prefix(&format!("{dir}/")) {
            return Some(Asset::new(root, rel));
        }
    }
    None
}

// ----------------------------------------------------------------------------- delete

fn delete_dialog(ctx: &egui::Context, b: &mut Browser, acts: &mut Vec<Act>) {
    let Some((asset, usage)) = b.deleting.clone() else {
        return;
    };
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::new("content delete")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.label(
            egui::RichText::new(format!("Delete {}?", asset.title()))
                .strong()
                .size(16.0),
        );
        ui.label(theme::dim(asset.key()));
        ui.add_space(6.0);
        if usage.is_empty() {
            ui.label("Nothing uses it.");
        } else {
            let mut users = Vec::new();
            if !usage.entities.is_empty() {
                users.push(format!(
                    "{} in this scene",
                    plural(usage.entities.len(), "object", "objects")
                ));
            }
            if !usage.files.is_empty() {
                users.push(plural(usage.files.len(), "file", "files"));
            }
            ui.label(
                egui::RichText::new(format!("Still used by {}", users.join(" and ")))
                    .color(theme::WARN),
            );
            for f in usage.files.iter().take(8) {
                ui.label(format!("    {f}"));
            }
            ui.label(theme::dim(
                "They will show it as missing until they point at something else.",
            ));
        }
        ui.label(theme::dim(format!(
            "It goes to {}/trash/, so it can be put back by hand.",
            content::EDITOR_DIR
        )));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let go =
                egui::Button::new(egui::RichText::new("Move to trash").color(egui::Color32::WHITE))
                    .fill(egui::Color32::from_rgb(0xb0, 0x3a, 0x2e));
            if ui.add(go).clicked() {
                acts.push(Act::Delete(asset.clone()));
                close = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
    });
    if close || modal.should_close() {
        b.deleting = None;
    }
}

// ----------------------------------------------------------------------------- actions

/// Do what the panel would (for scripts).
pub fn act(world: &mut World, editor: &mut Editor, act: Act) {
    let assets = asset_root(world);
    run(world, editor, &assets, act);
}

fn run(world: &mut World, editor: &mut Editor, assets: &Path, act: Act) {
    let now = editor::now(world);
    match act {
        Act::Select(a) => editor.browser.selected = Some(a),
        Act::Go(f) => {
            editor.browser.go(f);
        }
        Act::Reveal(a) => {
            editor.browser.go(a.parent());
            editor.browser.selected = Some(a);
        }
        Act::Back => {
            let b = &mut editor.browser;
            if let Some(to) = b.back.pop() {
                b.forward.push(b.folder());
                b.folder = Some(to);
                b.source = Source::Files;
                b.search.clear();
            }
        }
        Act::Forward => {
            let b = &mut editor.browser;
            if let Some(to) = b.forward.pop() {
                b.back.push(b.folder());
                b.folder = Some(to);
                b.source = Source::Files;
                b.search.clear();
            }
        }
        Act::Open(a) => open(world, editor, assets, &a),
        Act::Rename(a, name) => {
            let name = name.replace(['/', '\\'], "_");
            // A name typed without its extension keeps the old one.
            let (_, ext) = content::split_ext(a.name());
            let name = if !ext.is_empty()
                && content::split_ext(&name).1 != ext
                && a.path(assets).is_file()
            {
                format!("{name}{ext}")
            } else {
                name
            };
            let to = a.parent().child(&name);
            moved(world, editor, assets, &a, &to, now);
        }
        Act::Move(a, folder) => {
            let to = folder.child(a.name());
            moved(world, editor, assets, &a, &to, now);
        }
        Act::Duplicate(a) => match content::duplicate(assets, &a) {
            Ok(copy) => {
                editor.info(now, format!("Duplicated {} as {}", a.key(), copy.key()));
                editor.browser.selected = Some(copy);
                changed(world, editor);
            }
            Err(e) => editor.error(now, e),
        },
        Act::AskDelete(a) => {
            let usage = content::usage(world, &a, assets);
            editor.browser.deleting = Some((a, usage));
        }
        Act::Delete(a) => match content::trash(assets, &a) {
            Ok(to) => {
                editor.info(
                    now,
                    format!(
                        "Moved {} to {}",
                        a.key(),
                        to.to_string_lossy().replace('\\', "/")
                    ),
                );
                let favorites = editor.browser.favorites();
                let before = favorites.len();
                favorites.retain(|f| *f != a && !f.rel.starts_with(&format!("{}/", a.rel)));
                if favorites.len() != before {
                    content::save_favorites(favorites);
                }
                if editor.browser.selected.as_ref() == Some(&a) {
                    editor.browser.selected = None;
                }
                if editor.browser.folder().rel.starts_with(&a.rel)
                    && editor.browser.folder().root == a.root
                    && !a.rel.is_empty()
                {
                    editor.browser.folder = Some(a.parent());
                }
                changed(world, editor);
            }
            Err(e) => editor.error(now, e),
        },
        Act::Favorite(a) => {
            let favorites = editor.browser.favorites();
            if let Some(i) = favorites.iter().position(|f| *f == a) {
                favorites.remove(i);
            } else {
                favorites.push(a);
            }
            content::save_favorites(favorites);
        }
        Act::NewFolder(parent) => match content::new_folder(assets, &parent) {
            Ok(folder) => {
                let b = &mut editor.browser;
                if b.folder() != parent {
                    b.go(parent);
                }
                b.selected = Some(folder.clone());
                b.renaming = Some((folder.clone(), folder.name().to_string()));
                b.rename_focus = true;
                b.refresh();
            }
            Err(e) => editor.error(now, e),
        },
        Act::SelectUsers(a) => {
            let usage = content::usage(world, &a, assets);
            editor.selection = usage.entities.clone();
            editor.info(
                now,
                format!(
                    "{} in this scene use {}",
                    plural(usage.entities.len(), "object", "objects"),
                    a.key()
                ),
            );
        }
        Act::Create(what) => {
            let at = editor::placement_point(world, editor);
            editor::create(world, editor, what, at);
        }
        Act::Import(path) => {
            crate::import::import_and_place(world, editor, Path::new(&path), None);
            changed(world, editor);
        }
        Act::SaveAs(name) => {
            let path = format!(
                "{}/{}.scene.json",
                editor::SCENES_DIR,
                name.trim().replace(['/', '\\'], "_")
            );
            editor::save_scene(world, editor, &path);
            editor.browser.refresh();
        }
        Act::NewScene => editor::new_scene(world, editor),
    }
}

/// After a rename or move: the open scene's path, favorites and selection follow the file.
fn moved(
    world: &mut World,
    editor: &mut Editor,
    assets: &Path,
    from: &Asset,
    to: &Asset,
    now: f64,
) {
    match content::move_asset(world, assets, from, to) {
        Ok(msg) => {
            editor.info(now, msg);
            let (old, new) = (from.key(), to.key());
            let follow = |s: &str| -> Option<String> {
                if s == old {
                    Some(new.clone())
                } else {
                    s.strip_prefix(&format!("{old}/"))
                        .map(|rest| format!("{new}/{rest}"))
                }
            };
            if let Some(p) = follow(&editor.scene_path) {
                editor.scene_path = p;
            }
            let b = &mut editor.browser;
            let follow_asset = |x: &Asset| -> Option<Asset> {
                if x.root != from.root {
                    return None;
                }
                if x.rel == from.rel {
                    Some(to.clone())
                } else {
                    x.rel
                        .strip_prefix(&format!("{}/", from.rel))
                        .map(|rest| to.child(rest))
                }
            };
            let favorites = b.favorites();
            let mut any = false;
            for f in favorites.iter_mut() {
                if let Some(n) = follow_asset(f) {
                    *f = n;
                    any = true;
                }
            }
            if any {
                content::save_favorites(favorites);
            }
            if let Some(n) = b.folder.as_ref().and_then(follow_asset) {
                b.folder = Some(n);
            }
            b.selected = Some(to.clone());
            changed(world, editor);
        }
        Err(e) => editor.error(now, e),
    }
}

/// Files changed: read folders, models and prefab files again.
fn changed(world: &mut World, editor: &mut Editor) {
    editor.browser.refresh();
    editor.models = None;
    crate::prefab::forget_files(world);
}

fn open(world: &mut World, editor: &mut Editor, assets: &Path, a: &Asset) {
    let now = editor::now(world);
    let key = a.key();
    match a.kind(assets) {
        Kind::Folder => editor.browser.go(a.clone()),
        Kind::Model => {
            let at = editor::placement_point(world, editor);
            crate::models::place(world, editor, &key, at);
        }
        Kind::Material => {
            let targets = crate::materials::selected_meshes(world, editor);
            if targets.is_empty() {
                editor.error(now, "Select the objects to put the material on first");
            } else {
                crate::materials::apply(world, editor, &key, &targets);
            }
        }
        Kind::Prefab => {
            crate::prefab::place(world, editor, &key, None, None);
        }
        Kind::Scene => editor::load_scene(world, editor, &key),
        _ => editor.browser.selected = Some(a.clone()),
    }
}
