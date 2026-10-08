use crate::{
    BundledResources, ENGINE_VERSION,
    bundle::{BUNDLE_SHA256, LocalBundle, installation_error},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const UPSTREAM_URL: &str = "https://data1b.fullyjustified.net/tlextras-2022.0r0.tar";
const UPSTREAM_DIGEST: &str = "6ffe055852f8faf66c0acbe1a7fb27f87b869a90bad1204f3bf4d9683f597c7c";
const UPSTREAM_LENGTH: u64 = 2_881_562_112;
const INSTALLATION_FILE: &str = "ligature-installed-bundle.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleInstallProgress {
    Downloading { received: u64, total: Option<u64> },
    Verifying,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Installation {
    pub filename: String,
    pub checksum: String,
    engine: String,
    fonts: String,
    upstream: String,
}

pub fn installation_directory() -> Result<PathBuf, String> {
    Ok(dirs::data_local_dir()
        .ok_or("Could not locate the application data directory")?
        .join("Ligature/tex"))
}

pub(crate) fn read_installation(directory: &Path) -> Result<Option<Installation>, String> {
    let path = directory.join(INSTALLATION_FILE);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(installation_error(&path, error)),
    };
    let installation: Installation = serde_json::from_slice(&bytes).map_err(|error| installation_error(&path, error))?;
    if installation.engine != ENGINE_VERSION || installation.fonts != BUNDLE_SHA256.trim() || installation.upstream != UPSTREAM_DIGEST {
        return Ok(None);
    }
    if Path::new(&installation.filename).file_name().and_then(|name| name.to_str()) != Some(installation.filename.as_str())
        || !installation.filename.ends_with(".zip")
        || installation.checksum.len() != 64
        || !installation.checksum.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(installation_error(&path, "invalid installation metadata"));
    }
    Ok(Some(installation))
}

pub fn installed_bundle_path(directory: &Path) -> Result<Option<PathBuf>, String> {
    if !directory.is_absolute() {
        return Err("The bundle installation directory must be absolute".into());
    }
    let Some(installation) = read_installation(directory)? else {
        return Ok(None);
    };
    let path = directory.join(installation.filename);
    LocalBundle::open_with_checksum(&path, &installation.checksum)?;
    Ok(Some(path))
}

/// Downloads only in response to an explicit user action. Return false to cancel.
pub fn install_upstream_bundle(
    resources: &BundledResources, directory: &Path, mut progress: impl FnMut(BundleInstallProgress) -> bool,
) -> Result<PathBuf, String> {
    if !directory.is_absolute() {
        return Err("The bundle installation directory must be absolute".into());
    }
    let overlay = resources.validate()?;
    fs::create_dir_all(directory).map_err(|error| error.to_string())?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(directory.join("ligature-bundle-install.lock"))
        .map_err(|error| error.to_string())?;
    lock.try_lock().map_err(|_| "A TeX bundle installation is already running".to_string())?;
    if let Ok(Some(path)) = installed_bundle_path(directory) {
        return Ok(path);
    }
    if !progress(BundleInstallProgress::Downloading { received: 0, total: None }) {
        return Err("TeX bundle download canceled".into());
    }
    let response = reqwest::blocking::Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(3600))
        .build()
        .map_err(|error| format!("Could not start bundle download: {error}"))?
        .get(UPSTREAM_URL)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| format!("Could not download the upstream TeX bundle: {error}"))?;
    let total = response.content_length();
    if total.is_some_and(|length| length != UPSTREAM_LENGTH) {
        return Err("The upstream TeX bundle size has changed; this version of Ligature requires its pinned resource revision".into());
    }
    let callback = RefCell::new(progress);
    let reader = ProgressReader {
        reader: response,
        received: 0,
        total: Some(UPSTREAM_LENGTH),
        progress: &callback,
    };
    install_archive(reader, overlay, directory, &mut |status| callback.borrow_mut()(status))
}

struct ProgressReader<'a, R, F> {
    reader: R,
    received: u64,
    total: Option<u64>,
    progress: &'a RefCell<F>,
}

impl<R: Read, F: FnMut(BundleInstallProgress) -> bool> Read for ProgressReader<'_, R, F> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if !(self.progress.borrow_mut())(BundleInstallProgress::Downloading {
            received: self.received,
            total: self.total,
        }) {
            return Err(std::io::Error::other("TeX bundle download canceled"));
        }
        let length = self.reader.read(buffer)?;
        self.received += length as u64;
        if length == 0 && self.total.is_some_and(|total| self.received != total) {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "Incomplete TeX bundle download"));
        }
        Ok(length)
    }
}

fn install_archive(
    reader: impl Read, mut overlay: LocalBundle, directory: &Path, progress: &mut impl FnMut(BundleInstallProgress) -> bool,
) -> Result<PathBuf, String> {
    let mut temporary = tempfile::Builder::new()
        .prefix(".ligature-full-")
        .suffix(".zip")
        .tempfile_in(directory)
        .map_err(|error| error.to_string())?;
    let mut writer = ZipWriter::new(temporary.as_file_mut());
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(7))
        .unix_permissions(0o644)
        .large_file(true);
    let mut names = BTreeSet::new();
    let mut source_digest = None;
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        if entry.header().entry_type().is_dir() {
            continue;
        }
        let path = entry.path().map_err(|error| error.to_string())?;
        let path = path.to_str().ok_or("Non-UTF-8 upstream resource name")?;
        let name = path.strip_prefix("./").unwrap_or(path).to_string();
        if !entry.header().entry_type().is_file()
            || Path::new(&name).file_name().and_then(|name| name.to_str()) != Some(name.as_str())
            || !names.insert(name.clone())
        {
            return Err(format!("Invalid or duplicate upstream resource '{name}'"));
        }
        if name == "SHA256SUM" {
            let mut digest = String::new();
            entry.take(128).read_to_string(&mut digest).map_err(|error| error.to_string())?;
            source_digest = Some(digest.trim().to_owned());
            continue;
        }
        if overlay.contains(&name) && (name.ends_with(".otf") || name == "NewComputerModern-LICENSE.txt") {
            continue;
        }
        writer.start_file(&name, options).map_err(|error| error.to_string())?;
        std::io::copy(&mut entry, &mut writer).map_err(|error| format!("Could not install resource '{name}': {error}"))?;
    }
    std::io::copy(&mut archive.into_inner(), &mut std::io::sink()).map_err(|error| error.to_string())?;
    if source_digest.as_deref() != Some(UPSTREAM_DIGEST) || !names.contains("tectonic-format-latex.tex") || !names.contains("latex.ltx") {
        return Err("The downloaded TeX bundle is corrupt or incompatible with the bundled engine".into());
    }
    if !progress(BundleInstallProgress::Verifying) {
        return Err("TeX bundle download canceled".into());
    }
    let fonts: Vec<_> = overlay
        .names()
        .filter(|name| name.ends_with(".otf") || *name == "NewComputerModern-LICENSE.txt")
        .map(str::to_owned)
        .collect();
    for name in fonts {
        writer.start_file(&name, options).map_err(|error| error.to_string())?;
        writer
            .write_all(&overlay.read(&name)?.ok_or("Missing shipped font")?)
            .map_err(|error| error.to_string())?;
    }
    writer.start_file("SHA256SUM", options).map_err(|error| error.to_string())?;
    let identity = hex::encode(Sha256::digest(format!("{UPSTREAM_DIGEST}:{}", BUNDLE_SHA256.trim())));
    writer.write_all(identity.as_bytes()).map_err(|error| error.to_string())?;
    writer.finish().map_err(|error| error.to_string())?;
    temporary.as_file().sync_all().map_err(|error| error.to_string())?;
    let checksum = checksum(temporary.path())?;
    let filename = temporary.path().file_name().unwrap().to_str().unwrap().to_string();
    let installation = Installation {
        filename,
        checksum,
        engine: ENGINE_VERSION.into(),
        fonts: BUNDLE_SHA256.trim().into(),
        upstream: UPSTREAM_DIGEST.into(),
    };
    LocalBundle::open_with_checksum(temporary.path(), &installation.checksum)?;
    let mut metadata = tempfile::NamedTempFile::new_in(directory).map_err(|error| error.to_string())?;
    serde_json::to_writer(&mut metadata, &installation).map_err(|error| error.to_string())?;
    metadata.as_file().sync_all().map_err(|error| error.to_string())?;
    if !progress(BundleInstallProgress::Verifying) {
        return Err("TeX bundle download canceled".into());
    }
    metadata.persist(directory.join(INSTALLATION_FILE)).map_err(|error| error.to_string())?;
    let (_, path) = temporary.keep().map_err(|error| error.to_string())?;
    Ok(path)
}

fn checksum(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let length = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
    }
    Ok(hex::encode(hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, bytes) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, *bytes).unwrap();
        }
        builder.into_inner().unwrap()
    }

    fn valid_archive() -> Vec<u8> {
        archive(&[
            ("SHA256SUM", UPSTREAM_DIGEST.as_bytes()),
            ("tectonic-format-latex.tex", b"format"),
            ("latex.ltx", b"latex"),
            ("extra.sty", b"extra package"),
        ])
    }

    fn overlay() -> LocalBundle {
        LocalBundle::open(&Path::new(crate::BUILT_RESOURCE_DIRECTORY).join("tex/ligature-bundle-1.zip")).unwrap()
    }

    #[test]
    fn install_selects_a_local_bundle_and_invalidates_the_previous_format_cache() {
        let directory = tempfile::tempdir().unwrap();
        let path = install_archive(Cursor::new(valid_archive()), overlay(), directory.path(), &mut |_| true).unwrap();
        assert_eq!(installed_bundle_path(directory.path()).unwrap(), Some(path.clone()));
        let shipped = BundledResources::new(Path::new(crate::BUILT_RESOURCE_DIRECTORY), &directory.path().join("cache")).unwrap();
        let resources = shipped.clone().with_installed_bundle(directory.path()).unwrap();
        assert_eq!(resources.bundle, path);
        assert_ne!(resources.format_cache, shipped.format_cache);
        let mut bundle = resources.validate().unwrap();
        assert_eq!(bundle.read("extra.sty").unwrap().unwrap(), b"extra package");
        assert!(bundle.contains("NewCM10-Book.otf"));
        assert_eq!(
            BundledResources::new(Path::new(crate::BUILT_RESOURCE_DIRECTORY), &directory.path().join("cache"))
                .unwrap()
                .with_installed_bundle(directory.path())
                .unwrap(),
            resources
        );
    }

    #[test]
    fn failed_or_canceled_installs_preserve_the_current_bundle() {
        let directory = tempfile::tempdir().unwrap();
        let original = install_archive(Cursor::new(valid_archive()), overlay(), directory.path(), &mut |_| true).unwrap();
        let metadata = fs::read(directory.path().join(INSTALLATION_FILE)).unwrap();
        for entries in [
            vec![("SHA256SUM", b"wrong".as_slice())],
            vec![("duplicate.sty", b"one".as_slice()), ("duplicate.sty", b"two".as_slice())],
            vec![("nested/escape.sty", b"unsafe".as_slice())],
        ] {
            assert!(install_archive(Cursor::new(archive(&entries)), overlay(), directory.path(), &mut |_| true).is_err());
        }
        assert!(
            install_archive(Cursor::new(valid_archive()), overlay(), directory.path(), &mut |_| false)
                .unwrap_err()
                .contains("canceled")
        );
        let truncated = valid_archive();
        assert!(install_archive(Cursor::new(&truncated[..700]), overlay(), directory.path(), &mut |_| true).is_err());
        assert_eq!(fs::read(directory.path().join(INSTALLATION_FILE)).unwrap(), metadata);
        assert!(original.is_file());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }

    #[test]
    fn incompatible_installations_fall_back_to_shipped_resources() {
        let directory = tempfile::tempdir().unwrap();
        install_archive(Cursor::new(valid_archive()), overlay(), directory.path(), &mut |_| true).unwrap();
        let mut installation = read_installation(directory.path()).unwrap().unwrap();
        installation.engine = "older-engine".into();
        fs::write(directory.path().join(INSTALLATION_FILE), serde_json::to_vec(&installation).unwrap()).unwrap();
        assert!(read_installation(directory.path()).unwrap().is_none());
        assert!(installed_bundle_path(directory.path()).unwrap().is_none());
        installation.engine = ENGINE_VERSION.into();
        installation.fonts = "older-font-revision".into();
        fs::write(directory.path().join(INSTALLATION_FILE), serde_json::to_vec(&installation).unwrap()).unwrap();
        assert!(read_installation(directory.path()).unwrap().is_none());
    }

    #[test]
    fn installed_bundle_detection_rejects_missing_and_corrupt_archives() {
        let directory = tempfile::tempdir().unwrap();
        assert!(installed_bundle_path(directory.path()).unwrap().is_none());
        let path = install_archive(Cursor::new(valid_archive()), overlay(), directory.path(), &mut |_| true).unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, b"corrupt").unwrap();
        assert!(installed_bundle_path(directory.path()).unwrap_err().contains("checksum mismatch"));
        fs::remove_file(&path).unwrap();
        assert!(installed_bundle_path(directory.path()).is_err());
        fs::write(&path, bytes).unwrap();
        assert_eq!(installed_bundle_path(directory.path()).unwrap(), Some(path));
    }

    #[test]
    fn installing_an_existing_valid_bundle_skips_the_download() {
        let directory = tempfile::tempdir().unwrap();
        let path = install_archive(Cursor::new(valid_archive()), overlay(), directory.path(), &mut |_| true).unwrap();
        let resources = BundledResources::new(Path::new(crate::BUILT_RESOURCE_DIRECTORY), &directory.path().join("cache")).unwrap();
        let installed = install_upstream_bundle(&resources, directory.path(), |_| panic!("an installed bundle must not download")).unwrap();
        assert_eq!(installed, path);
    }

    #[test]
    fn download_reader_reports_progress_and_stops_when_canceled() {
        let mut updates = Vec::new();
        let callback = RefCell::new(|status| {
            updates.push(status);
            updates.len() < 2
        });
        let mut reader = ProgressReader {
            reader: Cursor::new(b"download"),
            received: 0,
            total: Some(8),
            progress: &callback,
        };
        let mut buffer = [0; 4];
        assert_eq!(reader.read(&mut buffer).unwrap(), 4);
        assert!(reader.read(&mut buffer).unwrap_err().to_string().contains("canceled"));
        assert_eq!(
            updates,
            vec![
                BundleInstallProgress::Downloading { received: 0, total: Some(8) },
                BundleInstallProgress::Downloading { received: 4, total: Some(8) }
            ]
        );
    }
}
