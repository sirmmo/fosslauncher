//! Installing, removing and launching apps, and the record of what is installed.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalog::{App, Download, DownloadFormat, Installer, InstallerKind};
use crate::paths;

pub enum Event {
    Log(String),
    /// Fraction done, when known.
    Progress(Option<f32>),
}

pub type Report<'a> = &'a (dyn Fn(Event) + Sync);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installed {
    pub method: String,
    pub installer: Installer,
    #[serde(default)]
    pub version: Option<String>,
    /// argv for the Launch button; empty when the app can't be launched from here.
    #[serde(default)]
    pub run: Vec<String>,
    #[serde(default)]
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Db {
    pub apps: BTreeMap<String, Installed>,
}

impl Db {
    fn path() -> PathBuf {
        paths::data_dir().join("installed.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        std::fs::create_dir_all(paths::data_dir())?;
        let tmp = Self::path().with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(tmp, Self::path())?;
        Ok(())
    }
}

pub fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .user_agent(concat!("fosslauncher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(None)
        .build()?)
}

/// Is the tool needed by this installer kind present on this machine?
pub fn tool_available(kind: &InstallerKind) -> bool {
    match kind {
        InstallerKind::Flatpak { .. } => paths::which("flatpak").is_some(),
        InstallerKind::Winget { .. } => paths::which("winget").is_some(),
        InstallerKind::Homebrew { .. } => paths::which("brew").is_some(),
        InstallerKind::Download(_) => true,
    }
}

/// Installers usable on this machine, in catalog preference order.
pub fn candidates(app: &App) -> Vec<&Installer> {
    app.installers
        .iter()
        .filter(|i| i.matches_host() && tool_available(&i.kind))
        .collect()
}

/// Installers for this platform that need a tool that isn't installed (to tell the user what's missing).
pub fn missing_tools(app: &App) -> Vec<&'static str> {
    let mut out = Vec::new();
    for i in app
        .installers
        .iter()
        .filter(|i| i.matches_host() && !tool_available(&i.kind))
    {
        let tool = match i.kind {
            InstallerKind::Flatpak { .. } => "flatpak",
            InstallerKind::Winget { .. } => "winget",
            InstallerKind::Homebrew { .. } => "brew",
            InstallerKind::Download(_) => continue,
        };
        if !out.contains(&tool) {
            out.push(tool);
        }
    }
    out
}

pub fn install(app: &App, inst: &Installer, report: Report) -> Result<Installed> {
    report(Event::Log(format!(
        "Installing {} via {}",
        app.name,
        inst.kind.label()
    )));
    let mut rec = Installed {
        method: inst.kind.label(),
        installer: inst.clone(),
        version: None,
        run: Vec::new(),
        dir: None,
    };
    match &inst.kind {
        InstallerKind::Flatpak {
            flatpak_ref,
            remote,
        } => {
            if remote == "flathub" {
                run_cmd(
                    &[
                        "flatpak",
                        "remote-add",
                        "--user",
                        "--if-not-exists",
                        "flathub",
                        "https://dl.flathub.org/repo/flathub.flatpakrepo",
                    ],
                    report,
                )?;
            }
            run_cmd(
                &[
                    "flatpak",
                    "install",
                    "--user",
                    "-y",
                    "--noninteractive",
                    remote,
                    flatpak_ref,
                ],
                report,
            )?;
            rec.run = vec!["flatpak".into(), "run".into(), flatpak_ref.clone()];
        }
        InstallerKind::Winget { package } => {
            run_cmd(
                &[
                    "winget",
                    "install",
                    "--id",
                    package,
                    "--exact",
                    "--silent",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                    "--disable-interactivity",
                ],
                report,
            )?;
        }
        InstallerKind::Homebrew { package, cask } => {
            let mut args = vec!["brew", "install"];
            if *cask {
                args.push("--cask");
            }
            args.push(package);
            run_cmd(&args, report)?;
            if *cask && cfg!(target_os = "macos") {
                rec.run = vec!["open".into(), "-a".into(), app.name.clone()];
            } else if !cask {
                rec.run = vec![package.clone()];
            }
        }
        InstallerKind::Download(dl) => install_download(app, dl, &mut rec, report)?,
    }
    if !inst.run.is_empty() {
        let dir = rec
            .dir
            .as_deref()
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        rec.run = inst.run.iter().map(|a| a.replace("{dir}", &dir)).collect();
    }
    report(Event::Log(format!("{} installed", app.name)));
    Ok(rec)
}

pub fn uninstall(id: &str, rec: &Installed, report: Report) -> Result<()> {
    match &rec.installer.kind {
        InstallerKind::Flatpak { flatpak_ref, .. } => {
            run_cmd(&["flatpak", "uninstall", "--user", "-y", "--noninteractive", flatpak_ref], report)?
        }
        InstallerKind::Winget { package } => run_cmd(
            &["winget", "uninstall", "--id", package, "--exact", "--silent", "--disable-interactivity"],
            report,
        )?,
        InstallerKind::Homebrew { package, cask } => {
            let mut args = vec!["brew", "uninstall"];
            if *cask {
                args.push("--cask");
            }
            args.push(package);
            run_cmd(&args, report)?
        }
        InstallerKind::Download(dl) if dl.format == DownloadFormat::Installer => match &rec.dir {
            // A .app copied out of a DMG (macOS): removing the bundle uninstalls it.
            Some(app) if app.extension().is_some_and(|e| e == "app") => {
                if app.exists() {
                    std::fs::remove_dir_all(app).with_context(|| format!("removing {}", app.display()))?;
                }
                report(Event::Log(format!("Removed {}", app.display())));
            }
            _ => report(Event::Log(
                "This app was installed by its own installer: remove it with your system's uninstaller. \
                 Forgetting it here."
                    .into(),
            )),
        },
        InstallerKind::Download(_) => {
            let dir = paths::app_dir(id);
            if dir.exists() {
                std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
            }
            report(Event::Log(format!("Removed {}", dir.display())));
        }
    }
    Ok(())
}

pub fn launch(rec: &Installed) -> Result<()> {
    let (prog, args) = rec
        .run
        .split_first()
        .ok_or_else(|| anyhow!("no launch command known for this app"))?;
    let mut cmd = Command::new(prog);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = &rec.dir {
        cmd.current_dir(dir);
    }
    cmd.spawn().with_context(|| format!("starting {prog}"))?;
    Ok(())
}

/// Run a command to completion, streaming its output lines to `report`.
fn run_cmd(argv: &[&str], report: Report) -> Result<()> {
    report(Event::Log(format!("$ {}", argv.join(" "))));
    report(Event::Progress(None));
    let mut cmd = Command::new(argv[0]);
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .with_context(|| format!("starting {}", argv[0]))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    std::thread::scope(|s| {
        s.spawn(|| stream_lines(stderr, report));
        stream_lines(stdout, report);
    });
    let status = child.wait()?;
    if !status.success() {
        bail!("{} failed ({status})", argv[0]);
    }
    Ok(())
}

fn stream_lines(r: impl Read, report: Report) {
    // Progress output uses \r; treat it as a line break so the log stays readable.
    let mut buf = Vec::new();
    let mut reader = BufReader::new(r);
    let mut byte = [0u8; 1];
    while reader.read(&mut byte).map(|n| n == 1).unwrap_or(false) {
        if byte[0] == b'\n' || byte[0] == b'\r' {
            let line = String::from_utf8_lossy(&buf).trim().to_string();
            if !line.is_empty() {
                report(Event::Log(line));
            }
            buf.clear();
        } else {
            buf.push(byte[0]);
        }
    }
    let line = String::from_utf8_lossy(&buf).trim().to_string();
    if !line.is_empty() {
        report(Event::Log(line));
    }
    let _ = reader.fill_buf();
}

struct Resolved {
    url: String,
    file_name: String,
    sha256: Option<String>,
    version: Option<String>,
}

fn resolve(dl: &Download, report: Report) -> Result<Resolved> {
    if let Some(url) = &dl.url {
        let file_name = url
            .rsplit('/')
            .next()
            .unwrap_or("download")
            .split('?')
            .next()
            .unwrap()
            .to_string();
        return Ok(Resolved {
            url: url.clone(),
            file_name,
            sha256: dl.sha256.clone(),
            version: dl.version.clone(),
        });
    }
    let gh = dl
        .github
        .as_ref()
        .ok_or_else(|| anyhow!("download installer has neither url nor github"))?;
    report(Event::Log(format!(
        "Looking up latest release of {}",
        gh.repo
    )));

    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        assets: Vec<Asset>,
    }
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
        #[serde(default)]
        digest: Option<String>,
    }
    let mut req = http()?
        .get(format!(
            "https://api.github.com/repos/{}/releases/latest",
            gh.repo
        ))
        .header("Accept", "application/vnd.github+json");
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        req = req.bearer_auth(token);
    }
    let rel: Release = req.send()?.error_for_status()?.json()?;
    let re = regex::Regex::new(&gh.asset).context("asset pattern")?;
    let asset = rel
        .assets
        .into_iter()
        .find(|a| re.is_match(&a.name))
        .ok_or_else(|| {
            anyhow!(
                "release {} of {} has no asset matching {}",
                rel.tag_name,
                gh.repo,
                gh.asset
            )
        })?;
    Ok(Resolved {
        url: asset.browser_download_url,
        file_name: asset.name,
        sha256: asset
            .digest
            .and_then(|d| d.strip_prefix("sha256:").map(str::to_string)),
        version: Some(rel.tag_name),
    })
}

/// Download `url` to `dest`, verifying the SHA-256 when one is known.
fn download(res: &Resolved, dest: &Path, report: Report) -> Result<()> {
    report(Event::Log(format!("Downloading {}", res.url)));
    let mut resp = http()?.get(&res.url).send()?.error_for_status()?;
    let total = resp.content_length();
    let mut file =
        std::fs::File::create(dest).with_context(|| format!("creating {}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = resp.read(&mut buf)?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        report(Event::Progress(total.map(|t| done as f32 / t as f32)));
    }
    file.flush()?;
    let got = hex::encode(hasher.finalize());
    match &res.sha256 {
        Some(want) if !want.eq_ignore_ascii_case(&got) => {
            let _ = std::fs::remove_file(dest);
            bail!(
                "checksum mismatch for {}: expected {want}, got {got}",
                res.file_name
            );
        }
        Some(_) => report(Event::Log(format!("SHA-256 verified ({got})"))),
        None => report(Event::Log(format!(
            "No checksum published; SHA-256 is {got}"
        ))),
    }
    Ok(())
}

fn install_download(app: &App, dl: &Download, rec: &mut Installed, report: Report) -> Result<()> {
    let res = resolve(dl, report)?;
    rec.version = res.version.clone();

    if dl.format == DownloadFormat::Installer {
        let dir = paths::cache_dir().join("downloads");
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(&res.file_name);
        download(&res, &file, report)?;
        if cfg!(target_os = "macos") && res.file_name.to_lowercase().ends_with(".dmg") {
            let app_path = install_dmg(&file, report)?;
            let _ = std::fs::remove_file(&file);
            rec.run = vec!["open".into(), app_path.display().to_string()];
            rec.dir = Some(app_path);
            return Ok(());
        }
        report(Event::Log(
            "Opening the installer; follow its steps to finish".into(),
        ));
        open::that(&file).with_context(|| format!("opening {}", file.display()))?;
        return Ok(());
    }

    let final_dir = paths::app_dir(&app.id);
    let staging = final_dir.with_extension("staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    let downloaded = staging.join(&res.file_name);
    download(&res, &downloaded, report)?;

    let exe_rel: PathBuf = match dl.format {
        DownloadFormat::Appimage | DownloadFormat::Binary => PathBuf::from(&res.file_name),
        DownloadFormat::Archive => {
            report(Event::Log(format!("Extracting {}", res.file_name)));
            report(Event::Progress(None));
            let out = staging.join("x");
            extract(&downloaded, &out)?;
            std::fs::remove_file(&downloaded)?;
            let root = if dl.strip_root {
                single_subdir(&out)?.unwrap_or(out.clone())
            } else {
                out.clone()
            };
            // Move the extracted tree up so the staging dir is the app root.
            for entry in std::fs::read_dir(&root)? {
                let entry = entry?;
                std::fs::rename(entry.path(), staging.join(entry.file_name()))?;
            }
            std::fs::remove_dir_all(&out)?;
            PathBuf::from(
                dl.exec
                    .as_deref()
                    .ok_or_else(|| anyhow!("archive installer needs `exec`"))?,
            )
        }
        DownloadFormat::Installer => unreachable!(),
    };
    let exe = staging.join(&exe_rel);
    if !exe.is_file() {
        bail!("{} not found in the downloaded files", exe_rel.display());
    }
    make_executable(&exe)?;

    if final_dir.exists() {
        std::fs::remove_dir_all(&final_dir)?;
    }
    std::fs::rename(&staging, &final_dir)?;
    rec.dir = Some(final_dir.clone());
    rec.run = vec![final_dir.join(exe_rel).display().to_string()];
    Ok(())
}

/// macOS: mount a DMG, copy the .app it contains to ~/Applications (no admin rights needed) and
/// unmount. Returns the installed bundle path.
fn install_dmg(dmg: &Path, report: Report) -> Result<PathBuf> {
    let mount = paths::cache_dir().join(format!("mnt-{}", std::process::id()));
    std::fs::create_dir_all(&mount)?;
    let mount_s = mount.display().to_string();
    let dmg_s = dmg.display().to_string();
    run_cmd(
        &[
            "hdiutil",
            "attach",
            "-nobrowse",
            "-readonly",
            "-noautoopen",
            "-mountpoint",
            &mount_s,
            &dmg_s,
        ],
        report,
    )?;
    let result = (|| {
        let src = std::fs::read_dir(&mount)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().is_some_and(|e| e == "app"))
            .ok_or_else(|| anyhow!("no .app found in {}", dmg.display()))?;
        let apps = PathBuf::from(std::env::var_os("HOME").ok_or_else(|| anyhow!("HOME not set"))?)
            .join("Applications");
        std::fs::create_dir_all(&apps)?;
        let dest = apps.join(src.file_name().unwrap());
        if dest.exists() {
            std::fs::remove_dir_all(&dest)
                .with_context(|| format!("replacing {}", dest.display()))?;
        }
        report(Event::Log(format!(
            "Copying {} to {}",
            src.display(),
            dest.display()
        )));
        run_cmd(
            &[
                "ditto",
                &src.display().to_string(),
                &dest.display().to_string(),
            ],
            report,
        )?;
        Ok(dest)
    })();
    let _ = run_cmd(&["hdiutil", "detach", "-force", &mount_s], report);
    let _ = std::fs::remove_dir(&mount);
    result
}

fn extract(archive: &Path, out: &Path) -> Result<()> {
    std::fs::create_dir_all(out)?;
    let name = archive
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let f = std::fs::File::open(archive)?;
    if name.ends_with(".zip") {
        zip::ZipArchive::new(f)?.extract(out)?;
    } else if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        tar::Archive::new(flate2::read::GzDecoder::new(f)).unpack(out)?;
    } else {
        bail!("unsupported archive type: {name} (supported: .zip, .tar.gz)");
    }
    Ok(())
}

fn single_subdir(dir: &Path) -> Result<Option<PathBuf>> {
    let entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    Ok(match entries.as_slice() {
        [only] if only.file_type()?.is_dir() => Some(only.path()),
        _ => None,
    })
}

fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(path)?.permissions();
        perm.set_mode(perm.mode() | 0o755);
        std::fs::set_permissions(path, perm)?;
    }
    let _ = path;
    Ok(())
}
