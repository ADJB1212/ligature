//! Typed project configuration and workspace session persistence.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const PROJECT_FILE: &str = ".ligature.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontSettings {
    pub family: Option<String>,
    pub size: f32,
}
impl Default for FontSettings {
    fn default() -> Self {
        Self { family: None, size: 15.0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Indentation {
    Tabs,
    #[default]
    Spaces,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IndentationSettings {
    pub style: Indentation,
    pub width: usize,
}
impl Default for IndentationSettings {
    fn default() -> Self {
        Self {
            style: Indentation::Spaces,
            width: 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BuildSettings {
    pub program: String,
    pub arguments: Vec<String>,
    pub output_directory: PathBuf,
}
impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            program: "latexmk".into(),
            arguments: vec!["-pdf".into(), "-synctex=1".into(), "-interaction=nonstopmode".into()],
            output_directory: "build".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub font: FontSettings,
    pub indentation: IndentationSettings,
    pub build: BuildSettings,
    pub root_document: Option<PathBuf>,
}
impl Settings {
    pub fn validate(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        if !self.font.size.is_finite() || !(8.0..=72.0).contains(&self.font.size) {
            errors.push("font.size must be between 8 and 72; using 15".into());
            self.font.size = 15.0;
        }
        if self.font.family.as_ref().is_some_and(|name| name.trim().is_empty()) {
            errors.push("font.family must not be empty; using the system font".into());
            self.font.family = None;
        }
        if !(1..=16).contains(&self.indentation.width) {
            errors.push("indentation.width must be between 1 and 16; using 4".into());
            self.indentation.width = 4;
        }
        if self.build.program.trim().is_empty() {
            errors.push("build.program must not be empty; using latexmk".into());
            self.build.program = "latexmk".into();
        }
        if !relative_path(&self.build.output_directory) {
            errors.push("build.output_directory must be within the project; using build".into());
            self.build.output_directory = "build".into();
        }
        if self.root_document.as_ref().is_some_and(|path| !relative_path(path)) {
            errors.push("root_document must be within the project; ignoring it".into());
            self.root_document = None;
        }
        errors
    }
    pub fn load(path: &Path) -> (Self, Vec<String>) {
        let text = match ligature_fs::read(path) {
            Ok(file) => file.text,
            Err(_) if matches!(path.try_exists(), Ok(false)) => return (Self::default(), Vec::new()),
            Err(error) => return (Self::default(), vec![error.to_string()]),
        };
        let mut settings: Self = match serde_json::from_str(&text) {
            Ok(settings) => settings,
            Err(error) => {
                return (
                    Self::default(),
                    vec![format!("Invalid settings in '{}': {error}; using defaults", path.display())],
                );
            }
        };
        let errors = settings.validate();
        (settings, errors)
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut settings = self.clone();
        let errors = settings.validate();
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        persist(path, &settings)
    }
}
fn relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_) | std::path::Component::CurDir))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Session {
    pub project: Option<PathBuf>,
    pub documents: Vec<PathBuf>,
    pub active_document: Option<PathBuf>,
}
impl Session {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.try_exists().map_err(|error| error.to_string())? {
            return Ok(Self::default());
        }
        let file = ligature_fs::read(path).map_err(|error| error.to_string())?;
        serde_json::from_str(&file.text).map_err(|error| format!("Invalid session: {error}"))
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        persist(path, self)
    }
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|root| root.join("ligature/session.json"))
    }
}
fn persist(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    ligature_fs::write_atomic(path, &text).map(|_| ()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overrides_merge_with_defaults_and_invalid_values_fall_back() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PROJECT_FILE);
        std::fs::write(&path, r#"{"font":{"size":20},"indentation":{"width":0},"build":{"program":""}}"#).unwrap();
        let (settings, errors) = Settings::load(&path);
        assert_eq!(settings.font.size, 20.0);
        assert_eq!(settings.indentation.width, 4);
        assert_eq!(settings.build, BuildSettings::default());
        assert_eq!(errors.len(), 2);
        std::fs::write(&path, "{broken").unwrap();
        assert_eq!(Settings::load(&path).0, Settings::default());
        assert!(!Settings::load(&path).1.is_empty());
    }
    #[test]
    fn settings_and_session_round_trip_paths_with_spaces() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PROJECT_FILE);
        let settings = Settings {
            root_document: Some("chapter one/main.tex".into()),
            ..Settings::default()
        };
        settings.save(&path).unwrap();
        assert_eq!(Settings::load(&path), (settings, Vec::new()));
        let session = Session {
            project: Some(directory.path().into()),
            documents: vec!["chapter one/main.tex".into()],
            active_document: None,
        };
        let path = directory.path().join("config/session.json");
        session.save(&path).unwrap();
        assert_eq!(Session::load(&path).unwrap(), session);
    }
}
