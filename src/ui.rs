//! egui front end.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, RichText};

use crate::catalog::{App, Catalog, Platform};
use crate::install::{self, Db, Event};

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

enum Loaded {
    Loading,
    Ready {
        catalog: Catalog,
        warning: Option<String>,
    },
    Failed(String),
}

pub struct LauncherApp {
    source: String,
    loaded: Arc<Mutex<Loaded>>,
    db: Arc<Mutex<Db>>,
    jobs: Arc<Mutex<HashMap<String, Job>>>,
    topic: Option<String>,
    query: String,
    installed_only: bool,
    all_platforms: bool,
    selected: Option<String>,
    flash: Option<String>,
}

impl LauncherApp {
    pub fn new(cc: &eframe::CreationContext<'_>, source: String) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let app = Self {
            source,
            loaded: Arc::new(Mutex::new(Loaded::Loading)),
            db: Arc::new(Mutex::new(Db::load())),
            jobs: Arc::default(),
            topic: None,
            query: String::new(),
            installed_only: false,
            all_platforms: false,
            selected: None,
            flash: None,
        };
        app.reload(&cc.egui_ctx);
        app
    }

    fn reload(&self, ctx: &egui::Context) {
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
        let (jobs, db, ctx, app) = (self.jobs.clone(), self.db.clone(), ctx.clone(), app.clone());
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

impl eframe::App for LauncherApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let loaded = self.loaded.clone();
        let loaded = loaded.lock().unwrap();

        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("FOSS Launcher");
                ui.add_space(12.0);
                ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Search apps…")
                        .desired_width(260.0),
                );
                ui.toggle_value(&mut self.installed_only, "Installed");
                ui.checkbox(&mut self.all_platforms, "Other platforms");
                if ui.button("⟳ Refresh").clicked() {
                    self.reload(&ctx);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let host = Platform::current()
                        .map(Platform::label)
                        .unwrap_or("unsupported OS");
                    ui.weak(format!("{host} · {}", std::env::consts::ARCH));
                });
            });
            ui.add_space(4.0);
        });

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.weak(format!("Catalog: {}", self.source));
                if let Loaded::Ready { catalog, warning } = &*loaded {
                    ui.weak(format!("· {} apps", catalog.apps.len()));
                    if let Some(g) = &catalog.generated {
                        ui.weak(format!("· updated {g}"));
                    }
                    if let Some(w) = warning {
                        ui.colored_label(
                            Color32::from_rgb(200, 140, 0),
                            "· offline, using cached catalog",
                        )
                        .on_hover_text(w);
                    }
                }
                if let Some(msg) = &self.flash {
                    ui.colored_label(Color32::from_rgb(200, 60, 60), msg);
                }
            });
        });

        let catalog = match &*loaded {
            Loaded::Loading => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.centered_and_justified(|ui| ui.spinner());
                });
                return;
            }
            Loaded::Failed(e) => {
                let e = e.clone();
                drop(loaded);
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.heading("Could not load the catalog");
                    ui.label(e);
                    if ui.button("Retry").clicked() {
                        self.reload(&ctx);
                    }
                });
                return;
            }
            Loaded::Ready { catalog, .. } => catalog,
        };

        let db = self.db.lock().unwrap();
        let host = Platform::current();
        let visible = |a: &App| {
            (self.all_platforms || a.installers.iter().any(|i| Some(i.platform) == host))
                && (!self.installed_only || db.apps.contains_key(&a.id))
        };

        egui::Panel::left("topics")
            .resizable(false)
            .exact_size(190.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                let count = |t: Option<&str>| {
                    catalog
                        .apps
                        .iter()
                        .filter(|a| visible(a) && t.is_none_or(|t| a.topics.iter().any(|x| x == t)))
                        .count()
                };
                ui.selectable_value(&mut self.topic, None, format!("All apps ({})", count(None)));
                ui.separator();
                for t in &catalog.topics {
                    let n = count(Some(&t.id));
                    if n > 0 {
                        ui.selectable_value(
                            &mut self.topic,
                            Some(t.id.clone()),
                            format!("{} ({n})", t.name),
                        )
                        .on_hover_text(&t.description);
                    }
                }
            });

        let selected = self
            .selected
            .as_ref()
            .and_then(|id| catalog.apps.iter().find(|a| &a.id == id));
        let mut action: Option<(App, Op)> = None;
        let mut launch: Option<String> = None;

        if let Some(app) = selected {
            egui::Panel::right("details")
                .resizable(true)
                .default_size(360.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            icon(ui, app, 64.0);
                            ui.vertical(|ui| {
                                ui.heading(&app.name);
                                if !app.developer.is_empty() {
                                    ui.weak(&app.developer);
                                }
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                                if ui.small_button("✕").clicked() {
                                    self.selected = None;
                                }
                            });
                        });
                        ui.add_space(6.0);
                        ui.label(RichText::new(&app.summary).strong());
                        for para in app
                            .description
                            .split("\n\n")
                            .filter(|p| !p.trim().is_empty())
                        {
                            ui.add_space(4.0);
                            ui.label(para.trim());
                        }
                        ui.add_space(8.0);
                        egui::Grid::new("meta")
                            .num_columns(2)
                            .spacing([12.0, 4.0])
                            .show(ui, |ui| {
                                ui.weak("License");
                                ui.label(&app.license);
                                ui.end_row();
                                ui.weak("Website");
                                ui.hyperlink_to(&app.homepage, &app.homepage);
                                ui.end_row();
                                ui.weak("Source");
                                ui.hyperlink_to(&app.source_code, &app.source_code);
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
                                    ui.weak("Will install via");
                                    ui.label(inst.kind.label());
                                    ui.end_row();
                                }
                            });
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            action_buttons(ui, app, &db, &self.jobs, &mut action, &mut launch)
                        });
                        let missing = install::missing_tools(app);
                        if !missing.is_empty() && !db.apps.contains_key(&app.id) {
                            ui.weak(format!(
                                "Install {} to get more install options.",
                                missing.join(" or ")
                            ));
                        }
                        if let Some(job) = self.jobs.lock().unwrap().get(&app.id)
                            && !job.log.is_empty()
                        {
                            ui.add_space(8.0);
                            egui::CollapsingHeader::new("Log")
                                .default_open(job.running || job.error.is_some())
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
                    });
                });
        }

        egui::CentralPanel::default().show(ui, |ui| {
            let topic = self.topic.as_deref();
            let apps: Vec<&App> = catalog
                .apps
                .iter()
                .filter(|a| visible(a))
                .filter(|a| topic.is_none_or(|t| a.topics.iter().any(|x| x == t)))
                .filter(|a| a.matches_query(&self.query))
                .collect();
            if apps.is_empty() {
                ui.centered_and_justified(|ui| ui.weak("No apps match."));
                return;
            }
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for app in apps {
                        let is_sel = self.selected.as_deref() == Some(app.id.as_str());
                        let frame = egui::Frame::group(ui.style())
                            .fill(if is_sel {
                                ui.visuals().selection.bg_fill.gamma_multiply(0.3)
                            } else {
                                Color32::TRANSPARENT
                            })
                            .inner_margin(8.0);
                        // Sense clicks on the card background *before* adding its widgets, so the
                        // buttons inside stay on top and keep their own clicks.
                        let card = egui::UiBuilder::new().sense(egui::Sense::click());
                        let resp = ui
                            .scope_builder(card, |ui| {
                                frame.show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        icon(ui, app, 40.0);
                                        // Buttons first (right-aligned) so the text wraps in the space left over.
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                action_buttons(
                                                    ui,
                                                    app,
                                                    &db,
                                                    &self.jobs,
                                                    &mut action,
                                                    &mut launch,
                                                );
                                                ui.with_layout(
                                                    egui::Layout::top_down(egui::Align::Min),
                                                    |ui| {
                                                        ui.horizontal_wrapped(|ui| {
                                                            ui.label(
                                                                RichText::new(&app.name)
                                                                    .strong()
                                                                    .size(15.0),
                                                            );
                                                            ui.weak(&app.license);
                                                            if db.apps.contains_key(&app.id) {
                                                                ui.colored_label(
                                                                    Color32::from_rgb(60, 160, 90),
                                                                    "● installed",
                                                                );
                                                            }
                                                        });
                                                        ui.label(&app.summary);
                                                    },
                                                );
                                            },
                                        );
                                    });
                                })
                            })
                            .response;
                        if resp.clicked() {
                            self.selected = Some(app.id.clone());
                        }
                        ui.add_space(4.0);
                    }
                });
        });

        if let Some(id) = launch
            && let Some(rec) = db.apps.get(&id)
        {
            self.flash = install::launch(rec)
                .err()
                .map(|e| format!("Launch failed: {e:#}"));
        }
        drop(db);
        if let Some((app, op)) = action {
            self.flash = None;
            self.selected = Some(app.id.clone());
            self.start(&ctx, &app, op);
        }
    }
}

fn icon(ui: &mut egui::Ui, app: &App, size: f32) {
    let sz = egui::vec2(size, size);
    match &app.icon {
        Some(url) => {
            ui.add(
                egui::Image::new(url.as_str())
                    .fit_to_exact_size(sz)
                    .corner_radius(6.0),
            );
        }
        None => {
            let (rect, _) = ui.allocate_exact_size(sz, egui::Sense::hover());
            let hue = app
                .id
                .bytes()
                .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32))
                % 360;
            let color = egui::ecolor::Hsva::new(hue as f32 / 360.0, 0.45, 0.65, 1.0);
            ui.painter().rect_filled(rect, 6.0, color);
            let letter = app
                .name
                .chars()
                .next()
                .unwrap_or('?')
                .to_uppercase()
                .to_string();
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                letter,
                egui::FontId::proportional(size * 0.5),
                Color32::WHITE,
            );
        }
    }
}

fn action_buttons(
    ui: &mut egui::Ui,
    app: &App,
    db: &Db,
    jobs: &Mutex<HashMap<String, Job>>,
    action: &mut Option<(App, Op)>,
    launch: &mut Option<String>,
) {
    if let Some(job) = jobs.lock().unwrap().get(&app.id).filter(|j| j.running) {
        match job.progress {
            Some(p) => ui.add(
                egui::ProgressBar::new(p)
                    .desired_width(140.0)
                    .show_percentage(),
            ),
            None => ui.spinner(),
        };
        return;
    }
    if let Some(rec) = db.apps.get(&app.id) {
        if ui.button("Remove").clicked() {
            *action = Some((app.clone(), Op::Remove));
        }
        if !rec.run.is_empty() && ui.button("▶ Launch").clicked() {
            *launch = Some(app.id.clone());
        }
    } else if install::candidates(app).is_empty() {
        ui.add_enabled(false, egui::Button::new("Unavailable"))
            .on_disabled_hover_text("No installer for this platform, or the needed tool (flatpak/winget/brew) is missing");
    } else if ui.button("Install").clicked() {
        *action = Some((app.clone(), Op::Install));
    }
}
