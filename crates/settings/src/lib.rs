//! Typed application and project configuration and workspace session persistence.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    path::{Path, PathBuf},
};

pub const SETTINGS_FILE: &str = "settings.json";

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveBeforeBuild {
    #[default]
    SaveAll,
    RequireClean,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "BuildSettingsInput")]
pub struct BuildSettings {
    pub output_directory: PathBuf,
    pub synctex: bool,
    pub keep_logs: bool,
    pub keep_intermediates: bool,
    pub reruns: Option<usize>,
    pub save_before_build: SaveBeforeBuild,
}
impl Default for BuildSettings {
    fn default() -> Self {
        Self {
            output_directory: "build".into(),
            synctex: true,
            keep_logs: true,
            keep_intermediates: false,
            reruns: None,
            save_before_build: SaveBeforeBuild::SaveAll,
        }
    }
}

impl BuildSettings {
    pub fn validate(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.reruns.is_some_and(|reruns| reruns > 10) {
            errors.push("build.reruns must be at most 10; using automatic reruns".into());
            self.reruns = None;
        }
        if !relative_path(&self.output_directory) {
            errors.push("build.output_directory must be within the project; using build".into());
            self.output_directory = "build".into();
        }
        errors
    }
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct BuildSettingsInput {
    output_directory: PathBuf,
    synctex: bool,
    keep_logs: bool,
    keep_intermediates: bool,
    reruns: Option<usize>,
    save_before_build: SaveBeforeBuild,
    program: Option<serde_json::Value>,
    arguments: Option<serde_json::Value>,
    engine: Option<serde_json::Value>,
    executable_path: Option<serde_json::Value>,
}
impl Default for BuildSettingsInput {
    fn default() -> Self {
        let defaults = BuildSettings::default();
        Self {
            output_directory: defaults.output_directory,
            synctex: defaults.synctex,
            keep_logs: defaults.keep_logs,
            keep_intermediates: defaults.keep_intermediates,
            reruns: defaults.reruns,
            save_before_build: defaults.save_before_build,
            program: None,
            arguments: None,
            engine: None,
            executable_path: None,
        }
    }
}
impl From<BuildSettingsInput> for BuildSettings {
    fn from(input: BuildSettingsInput) -> Self {
        Self {
            output_directory: input.output_directory,
            synctex: input.synctex,
            keep_logs: input.keep_logs,
            keep_intermediates: input.keep_intermediates,
            reruns: input.reruns,
            save_before_build: input.save_before_build,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub font: FontSettings,
    pub indentation: IndentationSettings,
    pub build: BuildSettings,
    pub language_server: LanguageServerSettings,
    pub root_document: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LanguageServerSettings {
    pub enabled: bool,
    pub completion: bool,
    pub hover: bool,
}

impl Default for LanguageServerSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            completion: true,
            hover: true,
        }
    }
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
        errors.extend(self.build.validate());
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

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    pub defaults: Settings,
    pub projects: BTreeMap<PathBuf, Settings>,
}

impl Configuration {
    pub fn settings_for(&self, project: Option<&Path>) -> Settings {
        project.and_then(|path| self.projects.get(path)).unwrap_or(&self.defaults).clone()
    }

    pub fn default_path() -> Option<PathBuf> {
        config_directory().map(|directory| directory.join(SETTINGS_FILE))
    }

    pub fn load(path: &Path) -> (Self, Vec<String>) {
        let text = match ligature_fs::read(path) {
            Ok(file) => file.text,
            Err(_) if matches!(path.try_exists(), Ok(false)) => return (Self::default(), Vec::new()),
            Err(error) => return (Self::default(), vec![error.to_string()]),
        };
        let mut configuration: Self = match serde_json::from_str(&text) {
            Ok(configuration) => configuration,
            Err(error) => {
                return (
                    Self::default(),
                    vec![format!("Invalid settings in '{}': {error}; using defaults", path.display())],
                );
            }
        };
        let errors = configuration.validate();
        (configuration, errors)
    }

    fn validate(&mut self) -> Vec<String> {
        let mut errors = self.defaults.validate();
        if self.defaults.root_document.take().is_some() {
            errors.push("The main document must be configured for a project".into());
        }
        self.projects.retain(|path, settings| {
            if !path.is_absolute() {
                errors.push(format!("Project '{}' must have an absolute path; ignoring it", path.display()));
                return false;
            }
            errors.extend(settings.validate().into_iter().map(|error| format!("{}: {error}", path.display())));
            true
        });
        errors
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut configuration = self.clone();
        let errors = configuration.validate();
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        persist(path, self)
    }
}

pub fn config_directory() -> Option<PathBuf> {
    config_directory_from(std::env::var_os("XDG_CONFIG_HOME").as_deref(), std::env::var_os("HOME").as_deref())
}

fn config_directory_from(xdg: Option<&OsStr>, home: Option<&OsStr>) -> Option<PathBuf> {
    xdg.map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(PathBuf::from).filter(|path| path.is_absolute()).map(|path| path.join(".config")))
        .map(|path| path.join("ligature"))
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
        persist(path, self)
    }
    pub fn default_path() -> Option<PathBuf> {
        config_directory().map(|root| root.join("session.json"))
    }
}
fn persist(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|error| error.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    ligature_fs::write_atomic(path, &text).map(|_| ()).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn config_paths_use_absolute_xdg_or_home_config_and_never_the_working_directory() {
        assert_eq!(
            config_directory_from(Some(OsStr::new("/config")), Some(OsStr::new("/home/user"))),
            Some("/config/ligature".into())
        );
        for xdg in [None, Some(OsStr::new("")), Some(OsStr::new("relative"))] {
            assert_eq!(
                config_directory_from(xdg, Some(OsStr::new("/home/user"))),
                Some("/home/user/.config/ligature".into())
            );
            assert_eq!(config_directory_from(xdg, None), None);
        }
        assert_eq!(config_directory_from(None, Some(OsStr::new("relative"))), None);
    }
    #[test]
    fn overrides_merge_with_defaults_and_invalid_values_fall_back() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(SETTINGS_FILE);
        std::fs::write(&path, r#"{"font":{"size":20},"indentation":{"width":0},"build":{"reruns":11}}"#).unwrap();
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
        let path = directory.path().join(SETTINGS_FILE);
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

    #[test]
    fn legacy_engine_settings_migrate_without_losing_project_overrides() {
        let settings: Settings = serde_json::from_str(
            r#"{"font":{"size":21},"root_document":"main.tex","build":{"program":"/usr/bin/latexmk","arguments":["-pdf"],"engine":"xelatex","executable_path":"/custom/tex","output_directory":"output files"}}"#,
        ).unwrap();
        assert_eq!(settings.font.size, 21.0);
        assert_eq!(settings.root_document, Some("main.tex".into()));
        assert_eq!(settings.build.output_directory, PathBuf::from("output files"));
        assert!(settings.build.synctex && settings.build.keep_logs);
        let serialized = serde_json::to_string(&settings).unwrap();
        for legacy in ["program", "arguments", "engine", "executable_path"] {
            assert!(!serialized.contains(legacy));
        }
        assert_eq!(serde_json::from_str::<Settings>(&serialized).unwrap(), settings);
    }

    #[test]
    fn unknown_build_options_are_rejected() {
        assert!(serde_json::from_str::<Settings>(r#"{"build":{"synctx":false}}"#).is_err());
    }
}
