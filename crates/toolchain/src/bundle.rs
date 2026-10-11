use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
use zip::ZipArchive;

pub const BUNDLE_SHA256: &str = include_str!("../resources/ligature-bundle-1.sha256");

pub struct LocalBundle {
    archive: ZipArchive<File>,
}

impl LocalBundle {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_with_checksum(path, BUNDLE_SHA256.trim())
    }

    pub(crate) fn open_with_checksum(path: &Path, checksum: &str) -> Result<Self, String> {
        let (file, digest) = Self::hashed_file(path)?;
        if digest != checksum {
            return Err(installation_error(path, "bundled TeX resource checksum mismatch"));
        }
        Self::from_file(file, path)
    }

    pub(crate) fn checksum_and_validate(path: &Path) -> Result<String, String> {
        let (file, digest) = Self::hashed_file(path)?;
        Self::from_file(file, path)?;
        Ok(digest)
    }

    fn hashed_file(path: &Path) -> Result<(File, String), String> {
        let mut file = File::open(path).map_err(|error| installation_error(path, error))?;
        let mut hash = Sha256::new();
        let mut buffer = [0; 65536];
        loop {
            let length = file.read(&mut buffer).map_err(|error| installation_error(path, error))?;
            if length == 0 {
                break;
            }
            hash.update(&buffer[..length]);
        }
        file.seek(SeekFrom::Start(0)).map_err(|error| installation_error(path, error))?;
        Ok((file, hex::encode(hash.finalize())))
    }

    fn from_file(file: File, path: &Path) -> Result<Self, String> {
        let archive = ZipArchive::new(file).map_err(|error| installation_error(path, error))?;
        Ok(Self { archive })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.archive.index_for_name(name).is_some()
    }

    pub fn names(&self) -> Result<Vec<String>, String> {
        self.archive
            .file_names()
            .map(|entry| {
                entry
                    .map(|name| name.into_owned())
                    .map_err(|error| format!("Installation error: could not decode bundled resource name: {error}"))
            })
            .collect()
    }

    pub fn read(&mut self, name: &str) -> Result<Option<Vec<u8>>, String> {
        let mut entry = match self.archive.by_name(name) {
            Ok(entry) => entry,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(error) => return Err(format!("Installation error: could not read bundled resource '{name}': {error}")),
        };
        let mut bytes = Vec::new();
        entry
            .read_to_end(&mut bytes)
            .map_err(|error| format!("Installation error: corrupt bundled resource '{name}': {error}"))?;
        Ok(Some(bytes))
    }
}

pub fn installation_error(path: &Path, error: impl std::fmt::Display) -> String {
    format!(
        "Installation error: packaged resource '{}' is missing, corrupt, or incompatible: {error}. Reinstall Ligature.",
        path.display()
    )
}
