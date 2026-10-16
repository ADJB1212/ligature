use ligature_fs::Signature;
use ligature_toolchain::ENGINE_VERSION;
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

#[derive(Default)]
struct VersionCache(Option<(PathBuf, Signature)>);

impl VersionCache {
    fn verify(&mut self, executable: &Path, probe: impl FnOnce() -> Result<bool, String>) -> Result<(), String> {
        let signature = Signature::of(executable).map_err(|error| format!("Installation error: could not inspect bundled Tectonic: {error}"))?;
        if self.0.as_ref().is_some_and(|(path, cached)| path == executable && *cached == signature) {
            return Ok(());
        }
        self.0 = None;
        if !probe()? {
            return Err(format!(
                "Installation error: bundled Tectonic must be version {ENGINE_VERSION}. Reinstall Ligature."
            ));
        }
        if Signature::of(executable).ok() == Some(signature) {
            self.0 = Some((executable.to_path_buf(), signature));
        }
        Ok(())
    }
}

pub(crate) fn verify(executable: &Path, probe: impl FnOnce() -> Result<bool, String>) -> Result<(), String> {
    static VERIFIED: OnceLock<Mutex<VersionCache>> = OnceLock::new();
    VERIFIED
        .get_or_init(|| Mutex::new(VersionCache::default()))
        .lock()
        .unwrap()
        .verify(executable, probe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_executable_skips_probe_and_replacement_requires_verification() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("tectonic");
        std::fs::write(&executable, "first").unwrap();
        let mut cache = VersionCache::default();
        cache.verify(&executable, || Ok(true)).unwrap();
        cache.verify(&executable, || panic!("unchanged executable was probed")).unwrap();
        let replacement = directory.path().join("replacement");
        std::fs::write(&replacement, "other").unwrap();
        std::fs::rename(&replacement, &executable).unwrap();
        assert!(cache.verify(&executable, || Ok(false)).unwrap_err().contains("must be version"));
        assert!(cache.verify(&executable, || Err("probe failed".into())).is_err());
        cache.verify(&executable, || Ok(true)).unwrap();
        std::fs::remove_file(&executable).unwrap();
        assert!(cache.verify(&executable, || panic!("missing executable was probed")).is_err());
    }

    #[test]
    fn different_paths_and_changes_during_probe_are_not_reused() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        std::fs::write(&first, "engine").unwrap();
        std::fs::write(&second, "engine").unwrap();
        let mut cache = VersionCache::default();
        cache.verify(&first, || Ok(true)).unwrap();
        assert!(cache.verify(&second, || Ok(false)).is_err());
        cache
            .verify(&first, || {
                std::fs::write(&first, "changed engine").unwrap();
                Ok(true)
            })
            .unwrap();
        assert!(cache.verify(&first, || Ok(false)).is_err());
    }
}
