//! Catalog data model (mirrors `schema/app.schema.json` in the catalog repo) and loading.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths;

pub const DEFAULT_CATALOG: &str =
    "https://raw.githubusercontent.com/sirmmo/fosslauncher-catalog/main/index.json";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub schema_version: u32,
    #[serde(default)]
    pub generated: Option<String>,
    pub topics: Vec<Topic>,
    pub apps: Vec<App>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Topic {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub id: String,
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub developer: String,
    pub license: String,
    pub homepage: String,
    pub source_code: String,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub screenshots: Vec<String>,
    pub topics: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub installers: Vec<Installer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Linux,
    Windows,
    Macos,
}

impl Platform {
    pub fn current() -> Option<Self> {
        match std::env::consts::OS {
            "linux" => Some(Self::Linux),
            "windows" => Some(Self::Windows),
            "macos" => Some(Self::Macos),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Linux => "Linux",
            Self::Windows => "Windows",
            Self::Macos => "macOS",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn current() -> Option<Self> {
        match std::env::consts::ARCH {
            "x86_64" => Some(Self::X86_64),
            "aarch64" => Some(Self::Aarch64),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Installer {
    pub platform: Platform,
    #[serde(default)]
    pub arch: Vec<Arch>,
    #[serde(flatten)]
    pub kind: InstallerKind,
    #[serde(default)]
    pub run: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum InstallerKind {
    Flatpak {
        #[serde(rename = "ref")]
        flatpak_ref: String,
        #[serde(default = "default_remote")]
        remote: String,
    },
    Winget {
        package: String,
    },
    Homebrew {
        package: String,
        #[serde(default = "yes")]
        cask: bool,
    },
    Download(Download),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Download {
    pub format: DownloadFormat,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub github: Option<GithubRelease>,
    #[serde(default)]
    pub exec: Option<String>,
    #[serde(default = "yes")]
    pub strip_root: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadFormat {
    Appimage,
    Archive,
    Binary,
    Installer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GithubRelease {
    pub repo: String,
    pub asset: String,
}

fn default_remote() -> String {
    "flathub".into()
}

fn yes() -> bool {
    true
}

impl InstallerKind {
    pub fn label(&self) -> String {
        match self {
            Self::Flatpak { flatpak_ref, .. } => format!("Flatpak ({flatpak_ref})"),
            Self::Winget { package } => format!("winget ({package})"),
            Self::Homebrew { package, cask } => {
                format!(
                    "Homebrew {} ({package})",
                    if *cask { "cask" } else { "formula" }
                )
            }
            Self::Download(d) => match (&d.github, &d.url) {
                (Some(gh), _) => format!("Download from GitHub releases ({})", gh.repo),
                _ => "Direct download".into(),
            },
        }
    }
}

impl Installer {
    pub fn matches_host(&self) -> bool {
        Some(self.platform) == Platform::current()
            && (self.arch.is_empty() || Arch::current().is_some_and(|a| self.arch.contains(&a)))
    }
}

impl App {
    pub fn platforms(&self) -> Vec<Platform> {
        let mut out: Vec<Platform> = Vec::new();
        for i in &self.installers {
            if !out.contains(&i.platform) {
                out.push(i.platform);
            }
        }
        out
    }

    pub fn matches_query(&self, q: &str) -> bool {
        if q.is_empty() {
            return true;
        }
        let q = q.to_lowercase();
        [&self.name, &self.summary, &self.developer, &self.id]
            .iter()
            .any(|s| s.to_lowercase().contains(&q))
            || self.tags.iter().any(|t| t.to_lowercase().contains(&q))
    }
}

/// Load a catalog from an http(s) URL, an `index.json` file, or a checkout of the catalog repo
/// (directory with `topics.json` and `apps/*.json`). Remote catalogs are cached so the launcher
/// keeps working offline; returns the catalog and, when the cache was used, the fetch error.
pub fn load(source: &str) -> Result<(Catalog, Option<String>)> {
    if source.starts_with("http://") || source.starts_with("https://") {
        let cache = paths::cache_dir().join("index.json");
        match fetch(source) {
            Ok(text) => {
                let cat = parse_index(&text)?;
                let _ = std::fs::create_dir_all(paths::cache_dir());
                let _ = std::fs::write(&cache, text);
                Ok((cat, None))
            }
            Err(e) => {
                let text = std::fs::read_to_string(&cache)
                    .with_context(|| format!("cannot fetch catalog ({e:#}) and no cached copy"))?;
                Ok((parse_index(&text)?, Some(format!("{e:#}"))))
            }
        }
    } else {
        let path = Path::new(source);
        if path.is_dir() {
            Ok((load_dir(path)?, None))
        } else {
            let text =
                std::fs::read_to_string(path).with_context(|| format!("reading {source}"))?;
            Ok((parse_index(&text)?, None))
        }
    }
}

fn fetch(url: &str) -> Result<String> {
    let resp = crate::install::http()?
        .get(url)
        .send()?
        .error_for_status()?;
    Ok(resp.text()?)
}

fn parse_index(text: &str) -> Result<Catalog> {
    let cat: Catalog = serde_json::from_str(text).context("parsing catalog index")?;
    if cat.schema_version != SCHEMA_VERSION {
        bail!(
            "catalog schema_version {} is not supported (expected {SCHEMA_VERSION}); update FOSS Launcher",
            cat.schema_version
        );
    }
    Ok(cat)
}

fn load_dir(dir: &Path) -> Result<Catalog> {
    #[derive(Deserialize)]
    struct Topics {
        topics: Vec<Topic>,
    }
    let topics: Topics = serde_json::from_str(
        &std::fs::read_to_string(dir.join("topics.json")).context("reading topics.json")?,
    )?;
    let mut apps = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir.join("apps"))
        .context("reading apps/")?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    entries.sort();
    for p in entries {
        let app: App = serde_json::from_str(&std::fs::read_to_string(&p)?)
            .with_context(|| format!("parsing {}", p.display()))?;
        apps.push(app);
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    Ok(Catalog {
        schema_version: SCHEMA_VERSION,
        generated: None,
        topics: topics.topics,
        apps,
    })
}
