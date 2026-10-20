//! Updates from GitHub Releases. Fern looks for a newer published release, downloads the package
//! made for the way it was installed, checks the file against the SHA-256 GitHub lists for it, and
//! puts it in place when the user chooses "Restart to Update". A package manager's install (the
//! .deb) is updated by the package manager, so there Fern only says that a new version is out.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};
use fern_core::Store;
use gpui::{App, AppContext as _, AsyncApp, Context, Entity, Global, WeakEntity};
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

const LATEST_RELEASE: &str = "https://api.github.com/repos/atabits/fern/releases/latest";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Not right at startup, so that the first frame does not wait on the network.
const FIRST_CHECK_AFTER: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// "off" when the user turned automatic checks off in Settings → About.
const SETTING: &str = "auto_update";
const MAX_DOWNLOAD: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Downloading(String),
    /// Downloaded and checked: installs on "Restart to Update".
    Ready(String),
    /// Newer, but installed in a way Fern does not update itself: the release's page.
    Available { version: String, url: String },
    Failed(String),
}

pub struct Updater {
    store: Rc<Store>,
    install: Option<Install>,
    status: Status,
    staged: Option<PathBuf>,
}

struct GlobalUpdater(Entity<Updater>);

impl Global for GlobalUpdater {}

/// Starts the checks, unless this is a development build.
pub fn init(store: Rc<Store>, cx: &mut App) {
    let install = Install::current();
    if let Some(install) = &install {
        install.clean_up();
    }
    let updater = cx.new(|cx| {
        let mut updater = Updater { store, install, status: Status::Idle, staged: None };
        updater.start(cx);
        updater
    });
    cx.set_global(GlobalUpdater(updater));
}

pub fn updater(cx: &App) -> Option<Entity<Updater>> {
    cx.try_global::<GlobalUpdater>().map(|global| global.0.clone())
}

impl Updater {
    fn start(&mut self, cx: &mut Context<Self>) {
        if self.install.is_none() {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FIRST_CHECK_AFTER).await;
            loop {
                let checked = this.update(cx, |this, cx| {
                    if this.automatic() {
                        this.check(cx);
                    }
                });
                if checked.is_err() {
                    break;
                }
                cx.background_executor().timer(CHECK_EVERY).await;
            }
        })
        .detach();
    }

    /// Whether this copy of Fern can update at all: not a development build.
    pub fn supported(&self) -> bool {
        self.install.is_some()
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn automatic(&self) -> bool {
        self.store.setting(SETTING).ok().flatten().as_deref() != Some("off")
    }

    pub fn set_automatic(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Err(err) = self.store.set_setting(SETTING, if on { "on" } else { "off" }) {
            eprintln!("failed to save the update setting: {err:#}");
        }
        if on && matches!(self.status, Status::Idle | Status::Failed(_)) {
            self.check(cx);
        }
        cx.notify();
    }

    /// Looks for a newer release and, where Fern can install it, downloads it.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        let Some(install) = self.install.clone() else {
            return;
        };
        if matches!(self.status, Status::Checking | Status::Downloading(_) | Status::Ready(_)) {
            return;
        }
        self.status = Status::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = {
                let install = install.clone();
                cx.background_executor().spawn(async move { find_update(&install) }).await
            };
            let (version, asset) = match found {
                Ok(Found::Download { version, asset }) => (version, asset),
                Ok(Found::Available { version, url }) => return set_status(&this, cx, Status::Available { version, url }),
                Ok(Found::Current) => return set_status(&this, cx, Status::UpToDate),
                Err(err) => return set_status(&this, cx, Status::Failed(format!("{err:#}"))),
            };
            set_status(&this, cx, Status::Downloading(version.clone()));
            let staged = {
                let (install, version) = (install.clone(), version.clone());
                cx.background_executor().spawn(async move { download_and_stage(&install, &version, &asset) }).await
            };
            this.update(cx, |this, cx| {
                match staged {
                    Ok(path) => {
                        this.staged = Some(path);
                        this.status = Status::Ready(version);
                    }
                    Err(err) => {
                        eprintln!("failed to download the update: {err:#}");
                        this.status = Status::Failed(format!("{err:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// "Restart to Update": puts the downloaded version in place, starts it and quits; "Update
    /// to …" without a download opens the release's page.
    pub fn restart_to_update(&mut self, cx: &mut Context<Self>) {
        if let Status::Available { url, .. } = &self.status {
            cx.open_url(url);
            return;
        }
        let (Some(install), Some(staged)) = (&self.install, &self.staged) else {
            return;
        };
        match install.apply(staged) {
            Ok(()) => cx.quit(),
            Err(err) => {
                eprintln!("failed to install the update: {err:#}");
                self.staged = None;
                self.status = Status::Failed(format!("{err:#}"));
                cx.notify();
            }
        }
    }
}

fn set_status(this: &WeakEntity<Updater>, cx: &mut AsyncApp, status: Status) {
    this.update(cx, |this, cx| {
        this.status = status;
        cx.notify();
    })
    .ok();
}

/// How this copy of Fern was installed, which decides the package it updates from.
#[derive(Clone, Debug, PartialEq)]
enum Install {
    /// Windows, by Fern's installer; `all_users` when installed for everyone, in Program Files.
    WindowsInstaller { all_users: bool },
    /// Windows, `fern.exe` unpacked from the portable ZIP.
    WindowsPortable(PathBuf),
    /// macOS: the app bundle.
    MacApp(PathBuf),
    /// Linux: the AppImage file.
    AppImage(PathBuf),
    /// Linux: `fern` unpacked from the tar.gz.
    Tarball(PathBuf),
    /// Linux: installed by the package manager, which also updates it.
    Managed,
}

impl Install {
    fn current() -> Option<Self> {
        // A development build would replace itself with a release.
        if cfg!(debug_assertions) || std::env::var_os("FERN_DEV_STATE").is_some() {
            return None;
        }
        let exe = std::env::current_exe().ok()?;
        if cfg!(windows) {
            if exe.parent()?.join("unins000.exe").exists() {
                let program_files = std::env::var("ProgramFiles").unwrap_or_default().to_lowercase();
                let all_users = !program_files.is_empty() && exe.to_string_lossy().to_lowercase().starts_with(&program_files);
                return Some(Install::WindowsInstaller { all_users });
            }
            return Some(Install::WindowsPortable(exe));
        }
        if cfg!(target_os = "macos") {
            let app = exe.ancestors().find(|path| path.extension().is_some_and(|ext| ext == "app"))?;
            return Some(Install::MacApp(app.to_owned()));
        }
        if let Some(path) = std::env::var_os("APPIMAGE") {
            return Some(Install::AppImage(path.into()));
        }
        if exe.starts_with("/usr") {
            return Some(Install::Managed);
        }
        Some(Install::Tarball(exe))
    }

    /// The end of the package's name, after `fern-<version>-<os>-<arch>` (see script/package).
    fn package_suffix(&self) -> Option<&'static str> {
        Some(match self {
            Install::WindowsInstaller { .. } => "-setup.exe",
            Install::WindowsPortable(_) => ".zip",
            Install::MacApp(_) => ".dmg",
            Install::AppImage(_) => ".AppImage",
            Install::Tarball(_) => ".tar.gz",
            Install::Managed => return None,
        })
    }

    /// Removes what the last update left behind.
    fn clean_up(&self) {
        if let Ok(dir) = updates_dir() {
            std::fs::remove_dir_all(dir).ok();
        }
        match self {
            Install::WindowsPortable(exe) => {
                std::fs::remove_file(old_path(exe)).ok();
            }
            Install::MacApp(app) => {
                std::fs::remove_dir_all(old_path(app)).ok();
            }
            _ => {}
        }
    }

    /// Puts the downloaded version in place and starts it (the installer starts it itself).
    fn apply(&self, staged: &Path) -> Result<()> {
        match self {
            Install::WindowsInstaller { all_users } => {
                let scope = if *all_users { "/ALLUSERS" } else { "/CURRENTUSER" };
                command(staged)
                    .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/RELAUNCH=1", scope])
                    .spawn()
                    .context("cannot start the installer")?;
                return Ok(());
            }
            Install::WindowsPortable(exe) => {
                // Windows cannot overwrite a running program, but it can rename it.
                let old = old_path(exe);
                std::fs::remove_file(&old).ok();
                std::fs::rename(exe, &old).with_context(|| format!("cannot move {}", exe.display()))?;
                if let Err(err) = std::fs::copy(staged, exe) {
                    std::fs::rename(&old, exe).ok();
                    return Err(anyhow!(err).context(format!("cannot write {}", exe.display())));
                }
                command(exe).spawn()?;
            }
            Install::MacApp(app) => {
                let old = old_path(app);
                std::fs::remove_dir_all(&old).ok();
                std::fs::rename(app, &old).with_context(|| format!("cannot move {}", app.display()))?;
                let moved = std::fs::rename(staged, app).is_ok() || run(Command::new("ditto").arg(staged).arg(app)).is_ok();
                if !moved {
                    std::fs::remove_dir_all(app).ok();
                    std::fs::rename(&old, app).ok();
                    bail!("cannot write {}", app.display());
                }
                std::fs::remove_dir_all(&old).ok();
                // `-n`: a new instance, since this one is still running.
                Command::new("open").arg("-n").arg(app).spawn()?;
            }
            Install::AppImage(path) | Install::Tarball(path) => {
                replace_file(staged, path)?;
                Command::new(path).spawn()?;
            }
            Install::Managed => bail!("the package manager updates this installation"),
        }
        Ok(())
    }
}

/// `<path>.old`, where the replaced version waits to be removed.
fn old_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(".old");
    path.with_file_name(name)
}

/// Replaces the file at `target` (which may be running) with `source`, keeping it executable.
#[cfg(unix)]
fn replace_file(source: &Path, target: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let mut name = target.file_name().unwrap_or_default().to_owned();
    name.push(".new");
    let new = target.with_file_name(name);
    std::fs::copy(source, &new).with_context(|| format!("cannot write {}", new.display()))?;
    std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&new, target).inspect_err(|_| {
        std::fs::remove_file(&new).ok();
    })?;
    Ok(())
}

#[cfg(not(unix))]
fn replace_file(_: &Path, _: &Path) -> Result<()> {
    bail!("not used on this platform")
}

/// A command that, on Windows, opens no console window.
fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

fn run(command: &mut Command) -> Result<()> {
    let output = command.output().with_context(|| format!("cannot run {:?}", command.get_program()))?;
    if !output.status.success() {
        bail!("{:?} failed: {}", command.get_program(), String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(())
}

fn updates_dir() -> Result<PathBuf> {
    Ok(crate::data_dir()?.join("updates"))
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
    /// `sha256:<hex>`, which GitHub computes when the file is uploaded.
    digest: Option<String>,
}

#[derive(Debug, PartialEq)]
enum Found {
    Current,
    Download { version: String, asset: Asset },
    Available { version: String, url: String },
}

fn user_agent() -> String {
    format!("fern/{CURRENT_VERSION}")
}

/// Asks GitHub for the latest published release. Blocking: run it off the UI thread.
fn find_update(install: &Install) -> Result<Found> {
    let mut response = ureq::get(LATEST_RELEASE)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", &user_agent())
        .call()
        .context("cannot reach GitHub")?;
    let release: Release = serde_json::from_str(&response.body_mut().read_to_string()?)?;
    Ok(pick(&release, install, CURRENT_VERSION, std::env::consts::OS, std::env::consts::ARCH))
}

/// What `release` offers this installation, if it is newer than `current`.
fn pick(release: &Release, install: &Install, current: &str, os: &str, arch: &str) -> Found {
    let version = release.tag_name.trim_start_matches('v');
    if !is_newer(version, current) {
        return Found::Current;
    }
    let os = if os == "windows" || os == "linux" { os } else { "macos" };
    let arch = if arch == "aarch64" { "arm64" } else { arch };
    let asset = install
        .package_suffix()
        .map(|suffix| format!("fern-{version}-{os}-{arch}{suffix}"))
        .and_then(|name| release.assets.iter().find(|asset| asset.name == name));
    match asset {
        Some(asset) => Found::Download { version: version.to_owned(), asset: asset.clone() },
        None => Found::Available { version: version.to_owned(), url: release.html_url.clone() },
    }
}

/// Whether version `a` comes after `b`; both `major.minor.patch`, anything after `-` ignored.
fn is_newer(a: &str, b: &str) -> bool {
    let parts = |version: &str| -> Option<Vec<u64>> {
        version.split('-').next()?.split('.').map(|part| part.parse().ok()).collect()
    };
    match (parts(a), parts(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// Downloads `asset`, checks it and unpacks it; returns what `Install::apply` puts in place.
/// Blocking: run it off the UI thread.
fn download_and_stage(install: &Install, version: &str, asset: &Asset) -> Result<PathBuf> {
    let dir = updates_dir()?;
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let file = dir.join(&asset.name);
    download(asset, &file)?;
    let unpacked = |name: &str| dir.join(asset.name.trim_end_matches(install.package_suffix().unwrap_or_default())).join(name);
    match install {
        Install::WindowsInstaller { .. } => Ok(file),
        Install::WindowsPortable(_) => {
            // Windows 10 and later have bsdtar, which unpacks ZIP files too.
            run(command("tar").arg("-xf").arg(&file).arg("-C").arg(&dir))?;
            Ok(unpacked("fern.exe"))
        }
        Install::Tarball(_) => {
            run(Command::new("tar").arg("-xzf").arg(&file).arg("-C").arg(&dir))?;
            Ok(unpacked("fern"))
        }
        Install::AppImage(_) => Ok(file),
        Install::MacApp(_) => {
            let mount = dir.join("mount");
            run(Command::new("hdiutil").args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-mountpoint"]).arg(&mount).arg(&file))?;
            let copied = run(Command::new("ditto").arg(mount.join("Fern.app")).arg(dir.join("Fern.app")));
            run(Command::new("hdiutil").arg("detach").arg(&mount)).ok();
            copied?;
            Ok(dir.join("Fern.app"))
        }
        Install::Managed => bail!("the package manager updates this installation"),
    }
    .and_then(|path| {
        if !path.exists() {
            bail!("{} has no {}", asset.name, path.file_name().unwrap_or_default().to_string_lossy());
        }
        Ok(path)
    })
    .inspect_err(|_| {
        std::fs::remove_dir_all(&dir).ok();
    })
    .with_context(|| format!("version {version}"))
}

/// Saves `asset` to `path` if its SHA-256 is the one GitHub lists.
fn download(asset: &Asset, path: &Path) -> Result<()> {
    let expected = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .with_context(|| format!("GitHub lists no SHA-256 for {}", asset.name))?
        .to_lowercase();
    let mut response = ureq::get(&asset.browser_download_url).header("User-Agent", &user_agent()).call()?;
    let mut reader = response.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
    let mut file = std::fs::File::create(path).with_context(|| format!("cannot create {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])?;
    }
    file.sync_all()?;
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        std::fs::remove_file(path).ok();
        bail!("{} is damaged: its SHA-256 is {actual}, GitHub lists {expected}", asset.name);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str, names: &[&str]) -> Release {
        Release {
            tag_name: format!("v{version}"),
            html_url: format!("https://github.com/atabits/fern/releases/tag/v{version}"),
            assets: names
                .iter()
                .map(|name| Asset {
                    name: name.to_string(),
                    browser_download_url: format!("https://example.org/{name}"),
                    digest: Some("sha256:00".into()),
                })
                .collect(),
        }
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("0.1.10", "0.1.9"));
        assert!(is_newer("0.2.0", "0.1.5"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.1.5", "0.1.5"));
        assert!(!is_newer("0.1.4", "0.1.5"));
        assert!(!is_newer("0.1.5", "0.1.5-abc1234"));
        assert!(!is_newer("garbage", "0.1.5"));
    }

    #[test]
    fn picks_the_package_for_the_installation() {
        let names = [
            "fern-0.2.0-windows-x86_64-setup.exe",
            "fern-0.2.0-windows-x86_64.zip",
            "fern-0.2.0-macos-arm64.dmg",
            "fern-0.2.0-linux-arm64.AppImage",
            "fern-0.2.0-linux-x86_64.tar.gz",
            "fern-server-0.2.0-linux-x86_64.tar.gz",
        ];
        let release = release("0.2.0", &names);
        let name = |install: Install, os: &str, arch: &str| match pick(&release, &install, "0.1.5", os, arch) {
            Found::Download { version, asset } => {
                assert_eq!(version, "0.2.0");
                asset.name
            }
            other => panic!("{other:?}"),
        };
        assert_eq!(name(Install::WindowsInstaller { all_users: false }, "windows", "x86_64"), names[0]);
        assert_eq!(name(Install::WindowsPortable("fern.exe".into()), "windows", "x86_64"), names[1]);
        assert_eq!(name(Install::MacApp("/Applications/Fern.app".into()), "macos", "aarch64"), names[2]);
        assert_eq!(name(Install::AppImage("Fern.AppImage".into()), "linux", "aarch64"), names[3]);
        assert_eq!(name(Install::Tarball("fern".into()), "linux", "x86_64"), names[4]);
    }

    #[test]
    fn points_to_the_release_page_without_a_package() {
        let release = release("0.2.0", &["fern-0.2.0-linux-x86_64.deb"]);
        let expected = Found::Available { version: "0.2.0".into(), url: release.html_url.clone() };
        assert_eq!(pick(&release, &Install::Managed, "0.1.5", "linux", "x86_64"), expected);
        // An Intel Mac, with only the Apple Silicon package out.
        let release = self::release("0.2.0", &["fern-0.2.0-macos-arm64.dmg"]);
        assert!(matches!(pick(&release, &Install::MacApp("Fern.app".into()), "0.1.5", "macos", "x86_64"), Found::Available { .. }));
    }

    #[test]
    fn stays_on_the_current_version() {
        let release = release("0.1.5", &["fern-0.1.5-linux-x86_64.tar.gz"]);
        assert_eq!(pick(&release, &Install::Tarball("fern".into()), "0.1.5", "linux", "x86_64"), Found::Current);
    }

    #[test]
    fn reads_github_releases() {
        let json = r#"{"tag_name":"v0.2.0","html_url":"https://github.com/atabits/fern/releases/tag/v0.2.0","draft":false,
            "assets":[{"name":"fern-0.2.0-linux-x86_64.tar.gz","browser_download_url":"https://github.com/x","digest":"sha256:ab","size":1}]}"#;
        let release: Release = serde_json::from_str(json).unwrap();
        assert_eq!(release.assets[0].digest.as_deref(), Some("sha256:ab"));
    }

    #[test]
    fn names_the_old_copy() {
        assert_eq!(old_path(Path::new("/Applications/Fern.app")), Path::new("/Applications/Fern.app.old"));
        assert_eq!(old_path(Path::new("dir/fern.exe")), Path::new("dir/fern.exe.old"));
    }
}
