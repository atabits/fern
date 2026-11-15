//! Portable profiles live beside the artifact the user can move or update.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const MARKER: &str = ".portable";
const PROFILE: &str = "Data";

pub fn data_dir() -> Option<&'static Path> {
    // Keep using the startup location if the marker changes while Fern is running.
    static DATA_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DATA_DIR
        .get_or_init(|| {
            #[cfg(debug_assertions)]
            if std::env::var_os("FERN_DATA_DIR").is_some() {
                return None;
            }
            let exe = std::env::current_exe().ok()?;
            let bundle = cfg!(target_os = "macos")
                .then(|| mac_bundle(&exe))
                .flatten();
            profile_dir(&exe, app_image().as_deref(), bundle)
        })
        .as_deref()
}

pub fn app_image() -> Option<PathBuf> {
    // AppImage mounts the internal executable read-only. The launcher passes the real image path.
    if cfg!(target_os = "linux") {
        std::env::var_os("APPIMAGE")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    } else {
        None
    }
}

pub fn mac_bundle(exe: &Path) -> Option<&Path> {
    exe.ancestors()
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
}

fn mac_legacy_dir(bundle: &Path) -> PathBuf {
    bundle.join("Contents").join("MacOS")
}

/// Used by the updater before replacing or deleting a bundle. Preserve data even if the user
/// removed the marker or can no longer read the profile directory.
pub fn has_legacy_mac_profile(bundle: &Path) -> bool {
    let dir = mac_legacy_dir(bundle);
    [dir.join(MARKER), dir.join(PROFILE)].iter().any(|path| {
        std::fs::symlink_metadata(path)
            .map_or_else(|error| error.kind() != ErrorKind::NotFound, |_| true)
    })
}

fn profile_dir(exe: &Path, image: Option<&Path>, bundle: Option<&Path>) -> Option<PathBuf> {
    let root = if let Some(bundle) = bundle {
        let legacy = mac_legacy_dir(bundle);
        if legacy.join(MARKER).is_file() {
            // Keep existing installs on the same profile until the user moves it out of the bundle.
            return Some(legacy.join(PROFILE));
        }
        bundle.parent()?
    } else {
        image.unwrap_or(exe).parent()?
    };
    root.join(MARKER).is_file().then(|| root.join(PROFILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_profiles_survive_until_moved_beside_the_bundle() -> anyhow::Result<()> {
        let root = temp_dir()?;
        let bundle = root.join("Fern.app");
        let legacy = mac_legacy_dir(&bundle);
        let exe = legacy.join("fern");
        std::fs::create_dir_all(legacy.join(PROFILE))?;
        std::fs::write(legacy.join(MARKER), "")?;
        std::fs::write(legacy.join(PROFILE).join("tasks.db"), "previous profile")?;
        assert_eq!(mac_bundle(&exe), Some(bundle.as_path()));
        assert_eq!(
            profile_dir(&exe, None, mac_bundle(&exe)),
            Some(legacy.join(PROFILE))
        );
        assert!(has_legacy_mac_profile(&bundle));

        // Copying a marker outside by mistake must not switch to a new empty profile.
        std::fs::write(root.join(MARKER), "")?;
        assert_eq!(
            profile_dir(&exe, None, mac_bundle(&exe)),
            Some(legacy.join(PROFILE))
        );

        std::fs::remove_file(legacy.join(MARKER))?;
        assert!(
            has_legacy_mac_profile(&bundle),
            "data without a marker is still protected"
        );
        std::fs::rename(legacy.join(PROFILE), root.join(PROFILE))?;
        assert!(!has_legacy_mac_profile(&bundle));
        let external = profile_dir(&exe, None, mac_bundle(&exe)).unwrap();
        assert_eq!(external, root.join(PROFILE));
        assert_eq!(
            std::fs::read_to_string(external.join("tasks.db"))?,
            "previous profile"
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    #[test]
    fn appimage_uses_the_marker_beside_the_image() -> anyhow::Result<()> {
        let root = temp_dir()?;
        let mount = root.join("mount").join("bin");
        let exe = mount.join("fern");
        let image = root.join("Fern.AppImage");
        std::fs::create_dir_all(&mount)?;
        std::fs::write(mount.join(MARKER), "")?;
        assert_eq!(profile_dir(&exe, Some(&image), None), None);
        std::fs::write(root.join(MARKER), "")?;
        assert_eq!(
            profile_dir(&exe, Some(&image), None),
            Some(root.join(PROFILE))
        );
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    fn temp_dir() -> std::io::Result<PathBuf> {
        let root =
            std::env::temp_dir().join(format!("fern-test-portable-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&root)?;
        Ok(root)
    }
}
