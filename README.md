# FOSS Launcher

A small cross-platform (Linux, Windows, macOS) installer and launcher for open-source
applications, written in Rust with [egui](https://github.com/emilk/egui). Browse a curated
catalog by topic, install with one click, launch, remove.

The catalog lives in a separate repository,
[fosslauncher-catalog](https://github.com/sirmmo/fosslauncher-catalog), with one JSON manifest
per app and a JSON Schema for the format.

## How installs work

Each catalog entry lists installers in order of preference. The launcher uses the first one
that matches the OS/architecture and whose tool is present, and falls back to the next one if
it fails:

| kind | how |
|---|---|
| `flatpak` | `flatpak install --user` from Flathub (Flathub remote is added per-user if missing) |
| `winget` | `winget install --id … --exact --silent` |
| `homebrew` | `brew install [--cask]` |
| `download` | fetched by the launcher itself, pinned URL + SHA-256 or the latest GitHub release (checked against GitHub's published digest), then: AppImage/binary made executable, `.zip`/`.tar.gz` extracted, or native installer opened |

Downloaded apps go to the per-user data directory (`~/.local/share/fosslauncher/apps/<id>` on
Linux, `%APPDATA%\fosslauncher\fosslauncher\data\apps` on Windows,
`~/Library/Application Support/org.fosslauncher.fosslauncher/apps` on macOS). No admin rights are needed except
where winget/Homebrew/a native installer ask for them. What was installed and how is recorded
in `installed.json` in the same directory.

## Usage

```sh
fosslauncher                          # GUI
fosslauncher list                     # apps and how they'd install here (* = installed)
fosslauncher install org.qgis.qgis
fosslauncher run org.qgis.qgis
fosslauncher remove org.qgis.qgis
fosslauncher --catalog ../fosslauncher-catalog    # use a local catalog checkout (or index.json, or a URL)
```

Environment:

- `FOSSLAUNCHER_CATALOG`: catalog URL/path (default: the published `index.json`).
- `FOSSLAUNCHER_RENDERER=wgpu`: use wgpu instead of the default OpenGL renderer.
- `GITHUB_TOKEN`: optional, raises the GitHub API rate limit for release lookups.

The remote catalog is cached, so the launcher still works offline.

## Building

Rust 1.95 or later (required by egui 0.37).

```sh
cargo build --release
```

On Linux you need the X11/Wayland development headers egui uses
(`libxkbcommon-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev` on Debian/Ubuntu).

Installers are made with [cargo-packager](https://github.com/crabnebula-dev/cargo-packager)
(config in `[package.metadata.packager]` in `Cargo.toml`):

```sh
cargo install cargo-packager --locked
cargo build --release --target x86_64-unknown-linux-gnu
cargo packager --release --target x86_64-unknown-linux-gnu --formats deb,appimage
```

## Downloads and releases

`.github/workflows/release.yml` builds every platform on each push to `main` (rolling
[nightly](https://github.com/sirmmo/fosslauncher/releases/tag/nightly) pre-release) and on each
`vX.Y.Z` tag (versioned [release](https://github.com/sirmmo/fosslauncher/releases/latest); the
version is taken from the tag):

| platform | files |
|---|---|
| Linux x86_64, aarch64 | `.deb`, `.AppImage`, portable `.tar.gz` |
| Windows x86_64 | setup `.exe` (NSIS, per-user), `.msi`, portable `.zip` |
| Windows arm64 | setup `.exe`, portable `.zip` |
| macOS Apple Silicon, Intel | `.dmg`, portable `.tar.gz` |

Each release has a `SHA256SUMS.txt`. Builds are not code-signed yet: on macOS right-click → Open
the first time, on Windows SmartScreen asks for confirmation.

To cut a release: `git tag v0.2.0 && git push origin v0.2.0`.

## Layout

- `src/catalog.rs`: catalog data model (mirrors the catalog's JSON Schema) and loading/caching
- `src/install.rs`: installers, uninstall, launch, `installed.json`
- `src/ui.rs`: egui front end
- `src/main.rs`: CLI entry point
