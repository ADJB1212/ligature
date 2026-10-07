//! Bundled TeX resource locations and engine compatibility metadata.

use std::path::{Path, PathBuf};

pub const ENGINE_VERSION: &str = env!("LIGATURE_TECTONIC_VERSION");
pub const BUILT_RESOURCE_DIRECTORY: &str = env!("LIGATURE_TECTONIC_RESOURCES");
pub const BUNDLE_REVISION: &str = "1";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundledResources {
    pub executable: PathBuf,
    pub bundle: PathBuf,
    pub fonts: PathBuf,
    pub format_cache: PathBuf,
}

impl BundledResources {
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
            format_cache: cache_directory.join(format!("tectonic-{ENGINE_VERSION}-bundle-{BUNDLE_REVISION}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
