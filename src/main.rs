#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod catalog;
mod install;
mod paths;
mod ui;

use anyhow::{Result, anyhow, bail};

const USAGE: &str = "\
FOSS Launcher: install and launch open-source applications

usage: fosslauncher [--catalog <url|index.json|catalog-dir>] [command]

commands:
  (none)          open the graphical launcher
  list            list apps available for this platform
  install <id>    install an app
  remove <id>     uninstall an app
  run <id>        launch an installed app

The catalog can also be set with FOSSLAUNCHER_CATALOG.";

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut source =
        std::env::var("FOSSLAUNCHER_CATALOG").unwrap_or_else(|_| catalog::DEFAULT_CATALOG.into());
    if let Some(pos) = args.iter().position(|a| a == "--catalog") {
        if pos + 1 >= args.len() {
            bail!("--catalog needs a value");
        }
        source = args.remove(pos + 1);
        args.remove(pos);
    }
    match args.first().map(String::as_str) {
        None => gui(source),
        Some("-h" | "--help") => {
            println!("{USAGE}");
            Ok(())
        }
        Some(cmd) => cli(&source, cmd, args.get(1).map(String::as_str)),
    }
}

fn gui(source: String) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("FOSS Launcher")
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([720.0, 420.0]),
        // OpenGL works on the widest range of (older) machines; FOSSLAUNCHER_RENDERER=wgpu opts out.
        renderer: match std::env::var("FOSSLAUNCHER_RENDERER").as_deref() {
            Ok("wgpu") => eframe::Renderer::Wgpu,
            _ => eframe::Renderer::Glow,
        },
        ..Default::default()
    };
    eframe::run_native(
        "fosslauncher",
        options,
        Box::new(move |cc| Ok(Box::new(ui::LauncherApp::new(cc, source)))),
    )
    .map_err(|e| anyhow!("{e}"))
}

fn cli(source: &str, cmd: &str, id: Option<&str>) -> Result<()> {
    let report = |ev: install::Event| {
        if let install::Event::Log(line) = ev {
            eprintln!("{line}");
        }
    };
    let mut db = install::Db::load();
    if cmd == "run" || cmd == "remove" {
        let id = id.ok_or_else(|| anyhow!("{cmd} needs an app id"))?;
        let rec = db
            .apps
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("{id} is not installed"))?;
        if cmd == "run" {
            return install::launch(&rec);
        }
        install::uninstall(id, &rec, &report)?;
        db.apps.remove(id);
        return db.save();
    }

    let (cat, warning) = catalog::load(source)?;
    if let Some(w) = warning {
        eprintln!("warning: using cached catalog: {w}");
    }
    match cmd {
        "list" => {
            for app in &cat.apps {
                let via = install::candidates(app).first().map(|i| i.kind.label());
                let state = if db.apps.contains_key(&app.id) {
                    "*"
                } else {
                    " "
                };
                println!(
                    "{state} {:32} {:14} {:18} {}",
                    app.id,
                    app.name,
                    app.license,
                    via.unwrap_or_else(|| "(not available here)".into())
                );
            }
            Ok(())
        }
        "install" => {
            let id = id.ok_or_else(|| anyhow!("install needs an app id"))?;
            let app = cat
                .apps
                .iter()
                .find(|a| a.id == id)
                .ok_or_else(|| anyhow!("{id} is not in the catalog"))?;
            let mut last = None;
            for inst in install::candidates(app) {
                match install::install(app, inst, &report) {
                    Ok(rec) => {
                        println!("installed {} ({})", app.name, rec.run.join(" "));
                        db.apps.insert(app.id.clone(), rec);
                        return db.save();
                    }
                    Err(e) => {
                        eprintln!("failed: {e:#}");
                        last = Some(e);
                    }
                }
            }
            Err(last.unwrap_or_else(|| anyhow!("no installer for {id} on this platform")))
        }
        other => bail!("unknown command {other}\n\n{USAGE}"),
    }
}
