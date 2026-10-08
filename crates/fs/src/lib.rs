//! File reads and writes, atomic saves, path handling, and filesystem watching.

mod watcher;

pub use watcher::FileWatcher;

use std::{
    error::Error,
    fmt, fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct FileError {
    pub path: PathBuf,
    pub operation: &'static str,
    pub message: String,
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Could not {} '{}': {}", self.operation, self.path.display(), self.message)
    }
}

impl Error for FileError {}

fn error(path: &Path, operation: &'static str, message: impl ToString) -> FileError {
    FileError {
        path: path.to_owned(),
        operation,
        message: message.to_string(),
    }
}

pub struct LoadedFile {
    pub path: PathBuf,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub path: PathBuf,
    pub is_directory: bool,
}

pub fn read_directory(path: &Path) -> Result<Vec<DirectoryEntry>, FileError> {
    let mut entries = fs::read_dir(path)
        .map_err(|err| error(path, "list", err))?
        .map(|entry| {
            let entry = entry.map_err(|err| error(path, "list", err))?;
            let kind = entry.file_type().map_err(|err| error(&entry.path(), "inspect", err))?;
            Ok(DirectoryEntry {
                path: entry.path(),
                is_directory: kind.is_dir(),
            })
        })
        .collect::<Result<Vec<_>, FileError>>()?;
    entries.sort_by(|a, b| b.is_directory.cmp(&a.is_directory).then_with(|| a.path.cmp(&b.path)));
    Ok(entries)
}

pub fn decode(path: &Path, bytes: Vec<u8>) -> Result<String, FileError> {
    String::from_utf8(bytes).map_err(|_| error(path, "read", "unsupported encoding; only UTF-8 text is supported"))
}

pub fn read(path: &Path) -> Result<LoadedFile, FileError> {
    let bytes = fs::read(path).map_err(|err| error(path, "read", err))?;
    let text = decode(path, bytes)?;
    let path = fs::canonicalize(path).map_err(|err| error(path, "resolve", err))?;
    Ok(LoadedFile { path, text })
}

pub fn resolve(path: &Path) -> Result<PathBuf, FileError> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .unwrap_or_else(|| Path::new("."));
            let parent = fs::canonicalize(parent).map_err(|err| error(path, "resolve", err))?;
            let name = path.file_name().ok_or_else(|| error(path, "resolve", "a file name is required"))?;
            Ok(parent.join(name))
        }
        Err(err) => Err(error(path, "resolve", err)),
    }
}

pub fn write_atomic(path: &Path, text: &str) -> Result<PathBuf, FileError> {
    write_atomic_checked(path, text, None)
}

pub fn write_atomic_checked(path: &Path, text: &str, expected: Option<&FileState>) -> Result<PathBuf, FileError> {
    let path = resolve(path)?;
    let permissions = match fs::metadata(&path) {
        Ok(metadata) if metadata.is_file() => {
            if metadata.permissions().readonly() {
                return Err(error(&path, "save", "the file is read-only"));
            }
            Some(metadata.permissions())
        }
        Ok(_) => return Err(error(&path, "save", "the destination is not a regular file")),
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => return Err(error(&path, "save", err)),
    };
    let parent = path.parent().expect("resolved file has a parent");
    let mut temporary = tempfile::Builder::new()
        .prefix(".ligature-")
        .tempfile_in(parent)
        .map_err(|err| error(&path, "create a temporary file for", err))?;
    temporary.write_all(text.as_bytes()).map_err(|err| error(&path, "write", err))?;
    if let Some(permissions) = permissions {
        temporary
            .as_file()
            .set_permissions(permissions)
            .map_err(|err| error(&path, "preserve permissions for", err))?;
    }
    temporary.as_file().sync_all().map_err(|err| error(&path, "flush", err))?;
    if let Some(expected) = expected
        && &FileState::read(&path) != expected
    {
        return Err(error(
            &path,
            "save",
            "the file changed on disk; review the external change before saving again",
        ));
    }
    temporary.persist(&path).map_err(|err| error(&path, "replace", err.error))?;
    Ok(path)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileState {
    Present(Vec<u8>),
    Missing,
    Unreadable(String),
}

impl FileState {
    pub fn read(path: &Path) -> Self {
        match fs::read(path) {
            Ok(bytes) => Self::Present(bytes),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Self::Missing,
            Err(err) => Self::Unreadable(error(path, "read", err).to_string()),
        }
    }
}
