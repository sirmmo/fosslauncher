//! egui front end.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect, RichText, Sense, Stroke,
    StrokeKind, TextStyle, UiBuilder, Vec2, pos2, vec2,
};

use crate::catalog::{App, Catalog, Platform};
use crate::install::{self, Db, Event};

const ACCENT: Color32 = Color32::from_rgb(124, 92, 255);
const GREEN: Color32 = Color32::from_rgb(46, 170, 104);
const RED: Color32 = Color32::from_rgb(220, 72, 72);
const CARD_MIN_WIDTH: f32 = 290.0;
const CARD_HEIGHT: f32 = 156.0;
const GAP: f32 = 14.0;

#[derive(Clone, Copy, PartialEq)]
enum Op {
    Install,
    Remove,
}

#[derive(Default)]
struct Job {
    running: bool,
    progress: Option<f32>,
    log: Vec<String>,
    error: Option<String>,
}

struct Toast {
    text: String,
    ok: bool,
    shown: Instant,
}

enum Loaded {
    Loading,
    Ready {
        catalog: Catalog,
        warning: Option<String>,
    },
    Failed(String),
}

#[derive(Clone, PartialEq)]
enum View {
    Discover,
    Installed,
    Topic(String),
}

pub struct LauncherApp {
    source: String,
    loaded: Arc<Mutex<Loaded>>,
    db: Arc<Mutex<Db>>,
    jobs: Arc<Mutex<HashMap<String, Job>>>,
    toasts: Arc<Mutex<Vec<Toast>>>,
    view: View,
    query: String,
    selected: Option<String>,
}

impl LauncherApp {
    pub fn new(cc: &eframe::CreationContext<'_>, source: String) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);
        let app = Self {
            source,
            loaded: Arc::new(Mutex::new(Loaded::Loading)),
            db: Arc::new(Mutex::new(Db::load())),
            jobs: Arc::default(),
            toasts: Arc::default(),
            view: View::Discover,
            query: String::new(),
            selected: None,
        };
        app.reload(&cc.egui_ctx);
        app
    }

    fn reload(&self, ctx: &egui::Context) {
        install::refresh_tools();
        *self.loaded.lock().unwrap() = Loaded::Loading;
        let (loaded, source, ctx) = (self.loaded.clone(), self.source.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = match crate::catalog::load(&source) {
                Ok((catalog, warning)) => Loaded::Ready { catalog, warning },
                Err(e) => Loaded::Failed(format!("{e:#}")),
            };
            *loaded.lock().unwrap() = result;
            ctx.request_repaint();
        });
    }

    fn start(&self, ctx: &egui::Context, app: &App, op: Op) {
        let (jobs, db, toasts, ctx, app) = (
            self.jobs.clone(),
            self.db.clone(),
            self.toasts.clone(),
            ctx.clone(),
            app.clone(),
        );
        jobs.lock().unwrap().insert(
            app.id.clone(),
            Job {
                running: true,
                ..Default::default()
            },
        );
        std::thread::spawn(move || {
            let report = |ev: Event| {
                if let Some(job) = jobs.lock().unwrap().get_mut(&app.id) {
                    match ev {
                        Event::Log(line) => job.log.push(line),
                        Event::Progress(p) => job.progress = p,
                    }
                }
                ctx.request_repaint();
            };
            let result = match op {
                Op::Install => try_install(&app, &report).map(|rec| {
                    db.lock().unwrap().apps.insert(app.id.clone(), rec);
                }),
                Op::Remove => {
                    let rec = db.lock().unwrap().apps.get(&app.id).cloned();
                    match rec {
                        Some(rec) => install::uninstall(&app.id, &rec, &report).map(|()| {
                            db.lock().unwrap().apps.remove(&app.id);
                        }),
                        None => Ok(()),
                    }
                }
            };
            let result = result.and_then(|()| db.lock().unwrap().save());
            let (text, ok) = match (&result, op) {
                (Ok(()), Op::Install) => (format!("{} is installed", app.name), true),
                (Ok(()), Op::Remove) => (format!("{} was removed", app.name), true),
                (Err(_), Op::Install) => (format!("Could not install {}", app.name), false),
                (Err(_), Op::Remove) => (format!("Could not remove {}", app.name), false),
            };
            toasts.lock().unwrap().push(Toast {
                text,
                ok,
                shown: Instant::now(),
            });
            if let Some(job) = jobs.lock().unwrap().get_mut(&app.id) {
                job.running = false;
                job.progress = None;
                if let Err(e) = result {
                    job.log.push(format!("Error: {e:#}"));
                    job.error = Some(format!("{e:#}"));
                }
            }
            ctx.request_repaint();
        });
    }

    fn toast(&self, text: String, ok: bool) {
        self.toasts.lock().unwrap().push(Toast {
            text,
            ok,
            shown: Instant::now(),
        });
    }
}

/// Try the usable installers in preference order until one succeeds.
fn try_install(app: &App, report: install::Report) -> anyhow::Result<install::Installed> {
    let candidates = install::candidates(app);
    let mut last_err = anyhow::anyhow!("no installer for this platform");
    for inst in candidates {
        match install::install(app, inst, report) {
            Ok(rec) => return Ok(rec),
            Err(e) => {
                report(Event::Log(format!("Failed: {e:#}")));
                last_err = e;
            }
        }
    }
    Err(last_err)
}

fn setup_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.text_styles
            .insert(TextStyle::Heading, FontId::proportional(24.0));
        s.text_styles
            .insert(TextStyle::Body, FontId::proportional(14.0));
        s.text_styles
            .insert(TextStyle::Button, FontId::proportional(14.0));
        s.text_styles
            .insert(TextStyle::Small, FontId::proportional(11.5));
        s.text_styles
            .insert(TextStyle::Monospace, FontId::monospace(12.0));
        s.spacing.item_spacing = vec2(8.0, 6.0);
        s.spacing.button_padding = vec2(12.0, 5.0);
        s.spacing.interact_size.y = 26.0;

        let v = &mut s.visuals;
        let radius = CornerRadius::same(8);
        for w in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            w.corner_radius = radius;
        }
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.selection.bg_fill = ACCENT;
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        v.hyperlink_color = if v.dark_mode {
            Color32::from_rgb(165, 145, 255)
        } else {
            Color32::from_rgb(98, 66, 230)
        };
        if v.dark_mode {
            v.panel_fill = Color32::from_rgb(24, 24, 30);
            v.window_fill = Color32::from_rgb(34, 34, 42);
            v.faint_bg_color = Color32::from_rgb(34, 34, 43);
            v.extreme_bg_color = Color32::from_rgb(17, 17, 22);
            v.widgets.inactive.weak_bg_fill = Color32::from_rgb(46, 46, 58);
            v.widgets.hovered.weak_bg_fill = Color32::from_rgb(56, 56, 70);
        } else {
            v.panel_fill = Color32::from_rgb(246, 246, 250);
            v.window_fill = Color32::WHITE;
            v.faint_bg_color = Color32::WHITE;
            v.extreme_bg_color = Color32::from_rgb(236, 236, 243);
            v.widgets.inactive.weak_bg_fill = Color32::from_rgb(232, 232, 240);
            v.widgets.hovered.weak_bg_fill = Color32::from_rgb(222, 222, 234);
        }
    });
}

/// Background of the sidebar and details pane: a shade apart from the main area.
fn side_fill(v: &egui::Visuals) -> Color32 {
    if v.dark_mode {
        Color32::from_rgb(19, 19, 24)
    } else {
        Color32::from_rgb(237, 237, 244)
    }
}

fn card_stroke(v: &egui::Visuals) -> Color32 {
    if v.dark_mode {
        Color32::from_rgb(48, 48, 60)
    } else {
        Color32::from_rgb(222, 222, 232)
    }
}

/// A stable colour per topic, for the sidebar dots.
fn topic_color(id: &str) -> Color32 {
    let hue = id
        .bytes()
        .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32))
        % 360;
    egui::ecolor::Hsva::new(hue as f32 / 360.0, 0.55, 0.85, 1.0).into()
}

impl eframe::App for LauncherApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut want_reload = false;

        // Keyboard: Ctrl+F or / focuses search, Esc clears search, then closes the details pane.
        let search_id = egui::Id::unique("search");
        let search_focused = ctx.memory(|m| m.has_focus(search_id));
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)
                || (!search_focused && i.consume_key(egui::Modifiers::NONE, egui::Key::Slash))
            {
                ctx.memory_mut(|m| m.request_focus(search_id));
            }
            if i.consume_key(egui::Modifiers::NONE, egui::Key::Escape) {
                if !self.query.is_empty() {
                    self.query.clear();
                } else {
                    self.selected = None;
                }
            }
        });

        let loaded = self.loaded.clone();
        let loaded = loaded.lock().unwrap();

        egui::Panel::top("top")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(side_fill(ui.visuals()))
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Image::new(egui::include_image!("../assets/icon-128.png"))
                            .fit_to_exact_size(Vec2::splat(30.0)),
                    );
                    ui.label(RichText::new("FOSS Launcher").size(18.0).strong());
                    ui.add_space(24.0);
                    let search = egui::TextEdit::singleline(&mut self.query)
                        .id(search_id)
                        .hint_text("Search apps…   Ctrl+F")
                        .margin(vec2(12.0, 7.0))
                        .desired_width((ui.available_width() - 160.0).clamp(160.0, 460.0));
                    let resp = ui.add(search);
                    if resp.changed() && !self.query.is_empty() {
                        self.selected = None;
                    }
                    if !self.query.is_empty()
                        && ui.small_button("✕").on_hover_text("Clear").clicked()
                    {
                        self.query.clear();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(egui::Button::new(RichText::new("⟳").size(16.0)).frame(false))
                            .on_hover_text("Reload the catalog")
                            .clicked()
                        {
                            want_reload = true;
                        }
                        let host = Platform::current()
                            .map(Platform::label)
                            .unwrap_or("unsupported OS");
                        ui.weak(format!("{host} · {}", std::env::consts::ARCH));
                    });
                });
            });

        let catalog = match &*loaded {
            Loaded::Loading => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.centered_and_justified(|ui| ui.add(egui::Spinner::new().size(32.0)));
                });
                return;
            }
            Loaded::Failed(e) => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.3);
                        ui.heading("Could not load the catalog");
                        ui.weak(e);
                        ui.add_space(8.0);
                        if accent_button(ui, "Try again").clicked() {
                            want_reload = true;
                        }
                    });
                });
                drop(loaded);
                if want_reload {
                    self.reload(&ctx);
                }
                return;
            }
            Loaded::Ready { catalog, .. } => catalog,
        };
        let warning = match &*loaded {
            Loaded::Ready { warning, .. } => warning.clone(),
            _ => None,
        };

        let db = self.db.lock().unwrap();
        // Only apps that can be installed here (or already are) are shown at all.
        let shown: Vec<&App> = catalog
            .apps
            .iter()
            .filter(|a| db.apps.contains_key(&a.id) || !install::candidates(a).is_empty())
            .collect();
        // Apps hidden only because a package manager is missing: worth a hint.
        let mut unlock: HashMap<&'static str, usize> = HashMap::new();
        for a in catalog
            .apps
            .iter()
            .filter(|a| !shown.iter().any(|s| s.id == a.id))
        {
            for tool in install::missing_tools(a) {
                *unlock.entry(tool).or_default() += 1;
            }
        }
        let installed_count = shown.iter().filter(|a| db.apps.contains_key(&a.id)).count();

        egui::Panel::left("nav")
            .resizable(false)
            .exact_size(214.0)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(side_fill(ui.visuals()))
                    .inner_margin(egui::Margin::symmetric(10, 8)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    section_label(ui, "LIBRARY");
                    if nav_item(
                        ui,
                        self.view == View::Discover,
                        ACCENT,
                        "Discover",
                        Some(shown.len()),
                    ) {
                        self.view = View::Discover;
                    }
                    if nav_item(
                        ui,
                        self.view == View::Installed,
                        GREEN,
                        "Installed",
                        Some(installed_count),
                    ) {
                        self.view = View::Installed;
                    }
                    ui.add_space(10.0);
                    section_label(ui, "CATEGORIES");
                    for t in &catalog.topics {
                        let n = shown.iter().filter(|a| a.topics.contains(&t.id)).count();
                        if n == 0 {
                            continue;
                        }
                        let sel = self.view == View::Topic(t.id.clone());
                        if nav_item(ui, sel, topic_color(&t.id), &t.name, Some(n)) {
                            self.view = View::Topic(t.id.clone());
                        }
                    }
                    ui.add_space(14.0);
                    let mut tools: Vec<_> = unlock.iter().collect();
                    tools.sort();
                    for (tool, n) in tools {
                        hint_box(
                            ui,
                            &format!("Install {} to get {n} more apps.", tool_name(tool)),
                        );
                    }
                    if let Some(w) = &warning {
                        hint_box(ui, "Offline: showing the cached catalog.").on_hover_text(w);
                    }
                });
            });

        let selected = self
            .selected
            .as_ref()
            .and_then(|id| shown.iter().copied().find(|a| &a.id == id));
        let mut action: Option<(App, Op)> = None;
        let mut launch: Option<String> = None;
        let mut close_details = false;

        if let Some(app) = selected {
            egui::Panel::right("details")
                .resizable(true)
                .default_size(380.0)
                .min_size(300.0)
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(side_fill(ui.visuals()))
                        .inner_margin(egui::Margin::same(18)),
                )
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        details(
                            ui,
                            app,
                            catalog,
                            &db,
                            &self.jobs,
                            &mut action,
                            &mut launch,
                            &mut close_details,
                        );
                    });
                });
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(egui::Margin::symmetric(24, 18)),
            )
            .show(ui, |ui| {
                let searching = !self.query.trim().is_empty();
                let (title, subtitle) = if searching {
                    (
                        format!("Results for “{}”", self.query.trim()),
                        String::new(),
                    )
                } else {
                    match &self.view {
                        View::Discover => (
                            "Discover".to_owned(),
                            format!(
                                "{} open-source apps ready to install on {}",
                                shown.len(),
                                Platform::current()
                                    .map(Platform::label)
                                    .unwrap_or("this system")
                            ),
                        ),
                        View::Installed => (
                            "Installed".to_owned(),
                            "Apps installed with FOSS Launcher".to_owned(),
                        ),
                        View::Topic(id) => catalog
                            .topics
                            .iter()
                            .find(|t| &t.id == id)
                            .map(|t| (t.name.clone(), t.description.clone()))
                            .unwrap_or_default(),
                    }
                };
                let apps: Vec<&App> = shown
                    .iter()
                    .copied()
                    .filter(|a| {
                        // A search looks through everything; otherwise the view filters.
                        searching
                            || match &self.view {
                                View::Discover => true,
                                View::Installed => db.apps.contains_key(&a.id),
                                View::Topic(t) => a.topics.iter().any(|x| x == t),
                            }
                    })
                    .filter(|a| a.matches_query(self.query.trim()))
                    .collect();

                ui.label(RichText::new(title).size(26.0).strong());
                if !subtitle.is_empty() {
                    ui.weak(subtitle);
                }
                ui.add_space(12.0);

                if apps.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.25);
                        if searching {
                            ui.label(RichText::new("No apps match your search").size(17.0));
                        } else if self.view == View::Installed {
                            ui.label(RichText::new("Nothing installed yet").size(17.0));
                            ui.add_space(6.0);
                            if accent_button(ui, "Browse apps").clicked() {
                                self.view = View::Discover;
                            }
                        } else {
                            ui.weak("No apps here.");
                        }
                    });
                    return;
                }

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        let cols = (((width + GAP) / (CARD_MIN_WIDTH + GAP)) as usize).max(1);
                        let card_w = (width - GAP * (cols - 1) as f32) / cols as f32;
                        ui.spacing_mut().item_spacing = vec2(GAP, GAP);
                        for row in apps.chunks(cols) {
                            ui.horizontal(|ui| {
                                // The tiles' inner scopes upset item spacing, so space them by hand.
                                ui.spacing_mut().item_spacing.x = 0.0;
                                for (i, app) in row.iter().enumerate() {
                                    if i > 0 {
                                        ui.add_space(GAP);
                                    }
                                    let is_sel = self.selected.as_deref() == Some(app.id.as_str());
                                    if card(
                                        ui,
                                        app,
                                        catalog,
                                        vec2(card_w, CARD_HEIGHT),
                                        is_sel,
                                        &db,
                                        &self.jobs,
                                        &mut action,
                                        &mut launch,
                                    ) {
                                        self.selected = Some(app.id.clone());
                                    }
                                }
                            });
                        }
                        ui.add_space(8.0);
                    });
            });

        // Toasts, bottom right, for four seconds each.
        {
            let mut toasts = self.toasts.lock().unwrap();
            toasts.retain(|t| t.shown.elapsed() < Duration::from_secs(4));
            if !toasts.is_empty() {
                egui::Area::new(egui::Id::unique("toasts"))
                    .anchor(Align2::RIGHT_BOTTOM, vec2(-18.0, -18.0))
                    .order(egui::Order::Foreground)
                    .show(&ctx, |ui| {
                        for t in toasts.iter() {
                            egui::Frame::popup(ui.style())
                                .corner_radius(10)
                                .inner_margin(egui::Margin::symmetric(14, 10))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let (icon, color) =
                                            if t.ok { ("✔", GREEN) } else { ("✕", RED) };
                                        ui.colored_label(color, RichText::new(icon).strong());
                                        ui.label(&t.text);
                                    });
                                });
                        }
                    });
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }

        let launch_result = launch.and_then(|id| {
            let rec = db.apps.get(&id)?;
            let name = shown.iter().find(|a| a.id == id).map(|a| a.name.clone());
            Some((install::launch(rec), name.unwrap_or(id)))
        });
        drop(db);
        drop(loaded);
        if let Some((Err(e), name)) = launch_result {
            self.toast(format!("Could not open {name}: {e:#}"), false);
        }
        if close_details {
            self.selected = None;
        }
        if let Some((app, op)) = action {
            self.selected = Some(app.id.clone());
            self.start(&ctx, &app, op);
        }
        if want_reload {
            self.reload(&ctx);
        }
    }
}

fn tool_name(tool: &str) -> &str {
    match tool {
        "flatpak" => "Flatpak",
        "winget" => "winget",
        "brew" => "Homebrew",
        t => t,
    }
}

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(
        RichText::new(text)
            .size(10.5)
            .strong()
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(2.0);
}

/// A full-width sidebar entry: icon, label and a count on the right.
fn nav_item(
    ui: &mut egui::Ui,
    selected: bool,
    dot: Color32,
    label: &str,
    count: Option<usize>,
) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let v = ui.visuals();
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 8, ACCENT);
    } else if resp.hovered() {
        painter.rect_filled(rect, 8, v.widgets.hovered.weak_bg_fill);
    }
    let fg = if selected {
        Color32::WHITE
    } else {
        v.text_color()
    };
    painter.circle_filled(
        rect.left_center() + vec2(18.0, 0.0),
        4.5,
        if selected { Color32::WHITE } else { dot },
    );
    painter.text(
        rect.left_center() + vec2(34.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        fg,
    );
    if let Some(n) = count {
        painter.text(
            rect.right_center() - vec2(12.0, 0.0),
            Align2::RIGHT_CENTER,
            n.to_string(),
            FontId::proportional(12.0),
            if selected {
                Color32::from_white_alpha(200)
            } else {
                v.weak_text_color()
            },
        );
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn hint_box(ui: &mut egui::Ui, text: &str) -> egui::Response {
    egui::Frame::new()
        .fill(ui.visuals().widgets.inactive.weak_bg_fill)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).small());
        })
        .response
}

fn accent_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
            .fill(ACCENT)
            .corner_radius(14)
            .min_size(vec2(84.0, 28.0)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small rounded label, tinted with `color`.
fn chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.16))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).color(color));
        });
}

fn topic_name<'a>(catalog: &'a Catalog, id: &'a str) -> &'a str {
    catalog
        .topics
        .iter()
        .find(|t| t.id == id)
        .map_or(id, |t| t.name.as_str())
}

/// Text cut to `rows` lines with an ellipsis.
fn clamped(ui: &egui::Ui, text: &str, rows: usize, size: f32, color: Color32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(
        text.to_owned(),
        FontId::proportional(size),
        color,
        ui.available_width(),
    );
    job.wrap.max_rows = rows;
    job.wrap.overflow_character = Some('…');
    ui.painter().layout_job(job)
}

/// One app tile in the grid. Returns true when the tile itself (not a button) was clicked.
#[allow(clippy::too_many_arguments)]
fn card(
    ui: &mut egui::Ui,
    app: &App,
    catalog: &Catalog,
    size: Vec2,
    is_sel: bool,
    db: &Db,
    jobs: &Mutex<HashMap<String, Job>>,
    action: &mut Option<(App, Op)>,
    launch: &mut Option<String>,
) -> bool {
    // Allocate (and sense) the tile before adding its widgets, so the buttons inside stay on
    // top and keep their own clicks.
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let v = ui.visuals().clone();
    let hovered = ui.rect_contains_pointer(rect);
    let stroke = if is_sel {
        Stroke::new(2.0, ACCENT)
    } else if hovered {
        Stroke::new(1.0, ACCENT.gamma_multiply(0.7))
    } else {
        Stroke::new(1.0, card_stroke(&v))
    };
    let fill = if hovered && !is_sel {
        v.faint_bg_color.lerp_to_gamma(ACCENT, 0.04)
    } else {
        v.faint_bg_color
    };
    ui.painter()
        .rect(rect, 14, fill, stroke, StrokeKind::Inside);

    let inner = rect.shrink(14.0);
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
            ui.horizontal(|ui| {
                icon(ui, app, 48.0);
                ui.vertical(|ui| {
                    ui.add_space(2.0);
                    ui.add(
                        egui::Label::new(RichText::new(&app.name).size(16.0).strong()).truncate(),
                    );
                    let by = if app.developer.is_empty() {
                        app.license.clone()
                    } else {
                        format!("{} · {}", app.developer, app.license)
                    };
                    ui.add(egui::Label::new(RichText::new(by).small().weak()).truncate());
                });
            });
            ui.add_space(4.0);
            let summary = clamped(ui, &app.summary, 2, 13.0, v.text_color());
            ui.add(egui::Label::new(summary).selectable(false));
        },
    );

    let bottom = Rect::from_min_max(pos2(inner.left(), inner.bottom() - 28.0), inner.max);
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(bottom)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            if let Some(t) = app.topics.first() {
                chip(ui, topic_name(catalog, t), v.weak_text_color());
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                action_buttons(ui, app, db, jobs, action, launch, false);
            });
        },
    );

    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

#[allow(clippy::too_many_arguments)]
fn details(
    ui: &mut egui::Ui,
    app: &App,
    catalog: &Catalog,
    db: &Db,
    jobs: &Mutex<HashMap<String, Job>>,
    action: &mut Option<(App, Op)>,
    launch: &mut Option<String>,
    close: &mut bool,
) {
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            if ui
                .add(egui::Button::new(RichText::new("✕").size(14.0)).frame(false))
                .on_hover_text("Close (Esc)")
                .clicked()
            {
                *close = true;
            }
        });
    });
    ui.horizontal(|ui| {
        icon(ui, app, 84.0);
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new(&app.name).size(24.0).strong());
            if !app.developer.is_empty() {
                ui.weak(&app.developer);
            }
            if db.apps.contains_key(&app.id) {
                ui.colored_label(GREEN, "✔ Installed");
            }
        });
    });
    ui.add_space(12.0);
    ui.horizontal(|ui| action_buttons(ui, app, db, jobs, action, launch, true));
    if let Some(job) = jobs.lock().unwrap().get(&app.id)
        && let Some(err) = &job.error
        && !job.running
    {
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(RED.gamma_multiply(0.14))
            .corner_radius(8)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.colored_label(RED, err);
            });
    }
    ui.add_space(14.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
        for t in &app.topics {
            chip(ui, topic_name(catalog, t), ACCENT);
        }
        chip(ui, &app.license, ui.visuals().weak_text_color());
    });
    ui.add_space(12.0);
    ui.label(RichText::new(&app.summary).size(15.0).strong());
    for para in app
        .description
        .split("\n\n")
        .filter(|p| !p.trim().is_empty())
    {
        ui.add_space(4.0);
        ui.label(para.trim());
    }

    ui.add_space(14.0);
    section_label(ui, "LINKS");
    ui.hyperlink_to("Website", &app.homepage);
    ui.hyperlink_to("Source code", &app.source_code);

    ui.add_space(10.0);
    section_label(ui, "INFORMATION");
    egui::Grid::new("meta")
        .num_columns(2)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            ui.weak("License");
            ui.label(&app.license);
            ui.end_row();
            ui.weak("Platforms");
            ui.label(
                app.platforms()
                    .iter()
                    .map(|p| p.label())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            ui.end_row();
            if let Some(rec) = db.apps.get(&app.id) {
                ui.weak("Installed via");
                ui.label(&rec.method);
                ui.end_row();
                if let Some(v) = &rec.version {
                    ui.weak("Version");
                    ui.label(v);
                    ui.end_row();
                }
            } else if let Some(inst) = install::candidates(app).first() {
                ui.weak("Installs via");
                ui.label(inst.kind.label());
                ui.end_row();
            }
        });

    if let Some(job) = jobs.lock().unwrap().get(&app.id)
        && !job.log.is_empty()
    {
        ui.add_space(10.0);
        egui::CollapsingHeader::new("Log")
            .id_salt(("log", &app.id))
            .default_open(job.running)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(220.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &job.log {
                            ui.monospace(line);
                        }
                    });
            });
    }
}

fn icon(ui: &mut egui::Ui, app: &App, size: f32) {
    let sz = Vec2::splat(size);
    let radius = (size * 0.22) as u8;
    match &app.icon {
        Some(url) => {
            ui.add(
                egui::Image::new(url.as_str())
                    .fit_to_exact_size(sz)
                    .corner_radius(radius),
            );
        }
        None => {
            let (rect, _) = ui.allocate_exact_size(sz, Sense::hover());
            let hue = app
                .id
                .bytes()
                .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32))
                % 360;
            let color = egui::ecolor::Hsva::new(hue as f32 / 360.0, 0.5, 0.7, 1.0);
            ui.painter().rect_filled(rect, radius, color);
            let letter = app
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                letter,
                FontId::proportional(size * 0.5),
                Color32::WHITE,
            );
        }
    }
}

/// Install / Open / Remove, or the progress of a running job. `full` adds Remove (details pane).
fn action_buttons(
    ui: &mut egui::Ui,
    app: &App,
    db: &Db,
    jobs: &Mutex<HashMap<String, Job>>,
    action: &mut Option<(App, Op)>,
    launch: &mut Option<String>,
    full: bool,
) {
    if let Some(job) = jobs.lock().unwrap().get(&app.id).filter(|j| j.running) {
        let installing = !db.apps.contains_key(&app.id);
        match job.progress {
            Some(p) => {
                ui.add(
                    egui::ProgressBar::new(p)
                        .desired_width(if full { 200.0 } else { 120.0 })
                        .desired_height(8.0)
                        .corner_radius(4)
                        .fill(ACCENT),
                );
                ui.weak(format!("{:.0}%", p * 100.0));
            }
            None => {
                ui.spinner();
                ui.weak(if installing {
                    "Installing…"
                } else {
                    "Removing…"
                });
            }
        }
        return;
    }
    if let Some(rec) = db.apps.get(&app.id) {
        if !rec.run.is_empty() && accent_button(ui, "Open").clicked() {
            *launch = Some(app.id.clone());
        }
        if full {
            let remove = ui
                .add(
                    egui::Button::new(RichText::new("Remove").color(RED))
                        .corner_radius(14)
                        .stroke(Stroke::new(1.0, RED.gamma_multiply(0.6)))
                        .fill(Color32::TRANSPARENT)
                        .min_size(vec2(72.0, 28.0)),
                )
                .on_hover_text("Uninstall this app");
            if remove.clicked() {
                *action = Some((app.clone(), Op::Remove));
            }
        } else if rec.run.is_empty() {
            ui.colored_label(GREEN, "✔ Installed");
        }
    } else if accent_button(ui, "Install").clicked() {
        *action = Some((app.clone(), Op::Install));
    }
}
