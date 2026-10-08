use std::{fs, io, path::Path};

pub fn legacy_outputs(build_directory: &Path) -> io::Result<Vec<std::path::PathBuf>> {
    let mut outputs = Vec::new();
    for entry in fs::read_dir(build_directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(hash) = name.to_str().and_then(|name| name.strip_prefix("ligature-toolchain-")) else {
            continue;
        };
        if !hash.is_empty() && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) && entry.file_type()?.is_dir() {
            let output = entry.path().join("out");
            if output.is_dir() {
                outputs.push(output);
            }
        }
    }
    Ok(outputs)
}

pub fn migrate_cache(outputs: &[std::path::PathBuf], storage: &Path) -> io::Result<()> {
    if storage.join("tectonic-install").exists() {
        return Ok(());
    }
    let newest = outputs
        .iter()
        .filter_map(|output| {
            fs::metadata(output.join("tectonic-install/bin/tectonic"))
                .and_then(|metadata| metadata.modified())
                .ok()
                .map(|modified| (modified, output))
        })
        .max_by_key(|(modified, _)| *modified);
    if let Some((_, output)) = newest {
        for name in ["tectonic-install", "tectonic-build"] {
            let source = output.join(name);
            if source.is_dir() && !storage.join(name).exists() {
                fs::rename(source, storage.join(name))?;
            }
        }
    }
    Ok(())
}

pub fn publish_resources(staged: &Path, storage: &Path) -> io::Result<()> {
    let resources = storage.join("resources");
    let previous = storage.join("previous-resources");
    if !resources.exists() && previous.exists() {
        fs::rename(&previous, &resources)?;
    }
    remove_directory(&previous)?;
    if resources.exists() {
        fs::rename(&resources, &previous)?;
    }
    if let Err(error) = fs::rename(staged, &resources) {
        if previous.exists() {
            fs::rename(&previous, &resources)?;
        }
        return Err(error);
    }
    remove_directory(&previous)
}

pub fn prune_legacy_outputs(outputs: &[std::path::PathBuf], resources: &Path) -> io::Result<()> {
    for output in outputs {
        let legacy_resources = output.join("resources");
        if !fs::symlink_metadata(&legacy_resources).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            remove_directory(&legacy_resources)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(resources, &legacy_resources)?;
        }
        for name in ["tectonic-install", "tectonic-build"] {
            remove_directory(&output.join(name))?;
        }
    }
    Ok(())
}

fn remove_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::remove_file(path),
        Ok(_) => fs::remove_dir_all(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
