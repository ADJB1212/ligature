//! Bundled TeX resource locations and engine compatibility metadata.

use std::path::{Path, PathBuf};

mod bundle;
mod install;
pub use bundle::LocalBundle;
use bundle::{BUNDLE_SHA256, installation_error};
pub use install::{BundleInstallProgress, install_upstream_bundle, installation_directory, installed_bundle_path};

pub const ENGINE_VERSION: &str = env!("LIGATURE_TECTONIC_VERSION");
pub const BUILT_RESOURCE_DIRECTORY: &str = env!("LIGATURE_TECTONIC_RESOURCES");
pub const BUNDLE_REVISION: &str = "1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundledResources {
    pub executable: PathBuf,
    pub bundle: PathBuf,
    pub fonts: PathBuf,
    pub format_cache: PathBuf,
    bundle_checksum: String,
}

impl BundledResources {
    pub fn discover() -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|error| format!("Could not locate Ligature: {error}"))?;
        let cache = dirs::cache_dir()
            .ok_or("Could not locate the application cache directory")?
            .join("Ligature");
        Self::for_executable(&executable, &cache)?.with_installed_bundle(&installation_directory()?)
    }

    pub fn for_executable(executable: &Path, cache_directory: &Path) -> Result<Self, String> {
        if !executable.is_absolute() {
            return Err("The application executable path must be absolute".into());
        }
        let parent = executable.parent().ok_or("The application executable has no parent directory")?;
        let resources = if parent.file_name().is_some_and(|name| name == "MacOS")
            && parent
                .parent()
                .is_some_and(|directory| directory.file_name().is_some_and(|name| name == "Contents"))
        {
            parent.parent().unwrap().join("Resources")
        } else {
            let adjacent = parent.join("resources");
            if adjacent.exists() {
                adjacent
            } else if Path::new(BUILT_RESOURCE_DIRECTORY)
                .ancestors()
                .nth(2)
                .is_some_and(|directory| parent == directory || parent == directory.join("deps"))
            {
                PathBuf::from(BUILT_RESOURCE_DIRECTORY)
            } else {
                return Err(installation_error(&adjacent, "application resources were not installed"));
            }
        };
        Self::new(&resources, cache_directory)
    }

    pub fn new(resource_directory: &Path, cache_directory: &Path) -> Result<Self, String> {
        if !resource_directory.is_absolute() || !cache_directory.is_absolute() {
            return Err("Resource and cache directories must be absolute paths".into());
        }
        Ok(Self {
            executable: resource_directory
                .join("bin")
                .join(if cfg!(windows) { "tectonic.exe" } else { "tectonic" }),
            bundle: resource_directory.join(format!("tex/ligature-bundle-{BUNDLE_REVISION}.zip")),
            fonts: resource_directory.join("newcomputermodern"),
            format_cache: cache_directory.join(format!("tectonic-{ENGINE_VERSION}-bundle-{BUNDLE_REVISION}-{}", BUNDLE_SHA256.trim())),
            bundle_checksum: BUNDLE_SHA256.trim().into(),
        })
    }

    pub fn with_installed_bundle(mut self, directory: &Path) -> Result<Self, String> {
        if !directory.is_absolute() {
            return Err("The bundle installation directory must be absolute".into());
        }
        if let Some(installed) = install::read_installation(directory)? {
            self.bundle = directory.join(installed.filename);
            self.bundle_checksum = installed.checksum;
            let cache = self.format_cache.parent().ok_or("The format cache has no parent directory")?;
            self.format_cache = cache.join(format!("tectonic-{ENGINE_VERSION}-bundle-{BUNDLE_REVISION}-{}", self.bundle_checksum));
        }
        Ok(self)
    }

    pub fn validate(&self) -> Result<LocalBundle, String> {
        for path in [&self.executable, &self.bundle, &self.fonts, &self.format_cache] {
            if !path.is_absolute() {
                return Err(installation_error(path, "resource paths must be absolute"));
            }
        }
        if !self.executable.is_file() {
            return Err(installation_error(&self.executable, "missing bundled Tectonic executable"));
        }
        let mut bundle = LocalBundle::open_with_checksum(&self.bundle, &self.bundle_checksum)?;
        for name in include_str!("../resources/ligature-bundle-1.fonts").lines() {
            let path = self.fonts.join("otf").join(name);
            let bytes = std::fs::read(&path).map_err(|error| installation_error(&path, error))?;
            if Some(bytes) != bundle.read(name)? {
                return Err(installation_error(&path, "packaged font checksum mismatch"));
            }
        }
        let expected_cache = format!("tectonic-{ENGINE_VERSION}-bundle-{BUNDLE_REVISION}-{}", self.bundle_checksum);
        if self.format_cache.file_name().and_then(|name| name.to_str()) != Some(expected_cache.as_str()) {
            return Err(installation_error(&self.format_cache, "incompatible format cache"));
        }
        Ok(bundle)
    }

    pub fn prepare_cache(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.format_cache)
            .map_err(|error| format!("Could not create application format cache '{}': {error}", self.format_cache.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packaged_application_paths_do_not_depend_on_the_launch_directory() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("Ligature.app/Contents/MacOS/Ligature");
        let resources = BundledResources::for_executable(&executable, &directory.path().join("cache")).unwrap();
        assert_eq!(
            resources.bundle,
            directory.path().join("Ligature.app/Contents/Resources/tex/ligature-bundle-1.zip")
        );
        assert!(
            BundledResources::for_executable(&directory.path().join("elsewhere/Ligature"), directory.path())
                .unwrap_err()
                .contains("Installation error")
        );
    }

    #[test]
    fn development_binaries_find_shared_resources_in_the_profile_directory() {
        let directory = tempfile::tempdir().unwrap();
        let profile = Path::new(BUILT_RESOURCE_DIRECTORY).ancestors().nth(2).unwrap();
        for parent in [profile.to_path_buf(), profile.join("deps")] {
            let resources = BundledResources::for_executable(&parent.join("ligature"), directory.path()).unwrap();
            assert_eq!(resources.bundle, Path::new(BUILT_RESOURCE_DIRECTORY).join("tex/ligature-bundle-1.zip"));
        }
    }

    #[test]
    fn local_provider_reports_missing_and_corrupt_archives_as_installation_errors() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("bundle.zip");
        assert!(LocalBundle::open(&path).err().unwrap().contains("Installation error"));
        std::fs::write(&path, "not a ZIP archive").unwrap();
        assert!(LocalBundle::open(&path).err().unwrap().contains("checksum mismatch"));
        let mut bundle = LocalBundle::open(&Path::new(BUILT_RESOURCE_DIRECTORY).join("tex/ligature-bundle-1.zip")).unwrap();
        assert!(bundle.contains("latex.ltx"));
        assert!(bundle.read("not-a-shipped-package.sty").unwrap().is_none());
        assert!(bundle.read("NewCM10-Book.otf").unwrap().unwrap().starts_with(b"OTTO"));
    }

    #[test]
    fn cache_identity_includes_engine_bundle_revision_and_content() {
        let directory = tempfile::tempdir().unwrap();
        let mut resources = BundledResources::new(Path::new(BUILT_RESOURCE_DIRECTORY), directory.path()).unwrap();
        assert!(resources.validate().is_ok());
        assert!(
            resources
                .format_cache
                .to_str()
                .unwrap()
                .contains(&format!("tectonic-{ENGINE_VERSION}-bundle-{BUNDLE_REVISION}-{}", BUNDLE_SHA256.trim()))
        );
        resources.format_cache = directory.path().join("tectonic-previous-engine-bundle-previous");
        assert!(resources.validate().err().unwrap().contains("incompatible format cache"));
    }

    #[test]
    fn missing_and_corrupt_packaged_fonts_are_installation_errors() {
        let directory = tempfile::tempdir().unwrap();
        let mut resources = BundledResources::new(Path::new(BUILT_RESOURCE_DIRECTORY), directory.path()).unwrap();
        resources.fonts = directory.path().join("newcomputermodern");
        let error = resources.validate().err().unwrap();
        assert!(error.contains("Installation error") && error.contains(".otf"));
        let name = include_str!("../resources/ligature-bundle-1.fonts").lines().next().unwrap();
        std::fs::create_dir_all(resources.fonts.join("otf")).unwrap();
        std::fs::write(resources.fonts.join("otf").join(name), "corrupt font").unwrap();
        assert!(resources.validate().err().unwrap().contains("font checksum mismatch"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn built_engine_runs_after_resources_are_relocated() {
        let directory = tempfile::tempdir().unwrap();
        let relocated = directory.path().join("resources with spaces");
        for name in ["bin", "lib"] {
            let destination = relocated.join(name);
            std::fs::create_dir_all(&destination).unwrap();
            for entry in std::fs::read_dir(Path::new(BUILT_RESOURCE_DIRECTORY).join(name)).unwrap() {
                let entry = entry.unwrap();
                std::fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
            }
        }
        let output = std::process::Command::new(relocated.join("bin/tectonic"))
            .arg("--version")
            .env_remove("DYLD_LIBRARY_PATH")
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), format!("Tectonic {ENGINE_VERSION}"));
    }

    #[test]
    fn resource_paths_are_explicit_and_versioned() {
        let directory = tempfile::tempdir().unwrap();
        let resources = BundledResources::new(directory.path(), &directory.path().join("cache")).unwrap();
        assert_eq!(resources.executable.parent().unwrap(), directory.path().join("bin"));
        assert_eq!(resources.bundle, directory.path().join("tex/ligature-bundle-1.zip"));
        assert_eq!(resources.fonts, directory.path().join("newcomputermodern"));
        assert!(resources.format_cache.starts_with(directory.path().join("cache")));
        assert!(resources.format_cache.to_str().unwrap().contains(ENGINE_VERSION));
        assert!(BundledResources::new(Path::new("resources"), directory.path()).is_err());
        assert!(BundledResources::new(directory.path(), Path::new("cache")).is_err());
    }
}
