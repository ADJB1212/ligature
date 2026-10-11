use std::path::{Path, PathBuf};

use ligature_settings::{BuildSettings, Indentation, PROJECT_FILE, SaveBeforeBuild, Session, Settings};

fn load(json: &str) -> (Settings, Vec<String>) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(PROJECT_FILE);
    std::fs::write(&path, json).unwrap();
    Settings::load(&path)
}

#[test]
fn defaults_are_valid_and_stable() {
    let mut settings = Settings::default();
    assert_eq!(settings.font.size, 15.0);
    assert_eq!(settings.font.family, None);
    assert_eq!(settings.indentation.style, Indentation::Spaces);
    assert_eq!(settings.indentation.width, 4);
    assert_eq!(settings.build.output_directory, Path::new("build"));
    assert!(settings.build.synctex && settings.build.keep_logs && !settings.build.keep_intermediates);
    assert_eq!(settings.build.save_before_build, SaveBeforeBuild::SaveAll);
    assert_eq!(settings.root_document, None);
    assert!(settings.validate().is_empty());
}

#[test]
fn language_settings_merge_defaults_and_round_trip_without_external_paths() {
    let (settings, errors) = load(r#"{"language_server":{"enabled":false,"hover":false}}"#);
    assert!(errors.is_empty());
    assert!(!settings.language_server.enabled && !settings.language_server.hover);
    assert!(settings.language_server.completion);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(PROJECT_FILE);
    settings.save(&path).unwrap();
    assert_eq!(Settings::load(&path), (settings, Vec::new()));
    assert!(!load(r#"{"language_server":{"executable":"external"}}"#).1.is_empty());
}

#[test]
fn empty_object_and_missing_file_both_produce_defaults_without_errors() {
    assert_eq!(load("{}"), (Settings::default(), Vec::new()));
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(Settings::load(&directory.path().join("absent.json")), (Settings::default(), Vec::new()));
}

#[test]
fn font_size_boundaries_are_inclusive() {
    for size in [8.0, 8.5, 72.0] {
        let mut settings = Settings::default();
        settings.font.size = size;
        assert!(settings.validate().is_empty(), "{size}");
        assert_eq!(settings.font.size, size);
    }
    for size in [7.99, 72.01, 0.0, -15.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut settings = Settings::default();
        settings.font.size = size;
        assert_eq!(settings.validate().len(), 1, "{size}");
        assert_eq!(settings.font.size, 15.0);
    }
}

#[test]
fn blank_font_families_fall_back_to_the_system_font() {
    for family in ["", "   ", "\t\n"] {
        let mut settings = Settings::default();
        settings.font.family = Some(family.into());
        assert_eq!(settings.validate().len(), 1, "{family:?}");
        assert_eq!(settings.font.family, None);
    }
    let mut named = Settings::default();
    named.font.family = Some("New Computer Modern".into());
    assert!(named.validate().is_empty());
    assert_eq!(named.font.family.as_deref(), Some("New Computer Modern"));
}

#[test]
fn indentation_width_boundaries_are_inclusive() {
    for width in [1, 4, 16] {
        let mut settings = Settings::default();
        settings.indentation.width = width;
        assert!(settings.validate().is_empty(), "{width}");
    }
    for width in [0, 17, usize::MAX] {
        let mut settings = Settings::default();
        settings.indentation.width = width;
        assert_eq!(settings.validate().len(), 1, "{width}");
        assert_eq!(settings.indentation.width, 4);
    }
}

#[test]
fn rerun_limit_boundaries_are_inclusive_and_invalid_values_become_automatic() {
    for reruns in [0, 1, 10] {
        let mut build = BuildSettings {
            reruns: Some(reruns),
            ..BuildSettings::default()
        };
        assert!(build.validate().is_empty(), "{reruns}");
        assert_eq!(build.reruns, Some(reruns));
    }
    let mut build = BuildSettings {
        reruns: Some(11),
        ..BuildSettings::default()
    };
    assert_eq!(build.validate().len(), 1);
    assert_eq!(build.reruns, None);
}

#[test]
fn output_directories_must_be_nonempty_relative_paths_inside_the_project() {
    for valid in ["out", "./out", "nested/out dir", "a/./b"] {
        let mut build = BuildSettings {
            output_directory: valid.into(),
            ..BuildSettings::default()
        };
        assert!(build.validate().is_empty(), "{valid}");
        assert_eq!(build.output_directory, Path::new(valid));
    }
    for invalid in ["", "/tmp/out", "../out", "a/../../out", ".."] {
        let mut build = BuildSettings {
            output_directory: invalid.into(),
            ..BuildSettings::default()
        };
        assert_eq!(build.validate().len(), 1, "{invalid:?}");
        assert_eq!(build.output_directory, Path::new("build"));
    }
}

#[test]
fn root_documents_must_be_relative_paths_inside_the_project() {
    for invalid in ["", "/abs/main.tex", "../main.tex", "sub/../../main.tex"] {
        let mut settings = Settings {
            root_document: Some(invalid.into()),
            ..Settings::default()
        };
        assert_eq!(settings.validate().len(), 1, "{invalid:?}");
        assert_eq!(settings.root_document, None);
    }
    let mut settings = Settings {
        root_document: Some("chapters/main.tex".into()),
        ..Settings::default()
    };
    assert!(settings.validate().is_empty());
}

#[test]
fn each_invalid_value_is_reported_once_and_valid_neighbors_are_kept() {
    let (settings, errors) = load(
        r#"{"font":{"size":1,"family":" "},"indentation":{"style":"tabs","width":99},"build":{"reruns":50,"output_directory":"/x","keep_logs":false},"root_document":"../r.tex"}"#,
    );
    assert_eq!(errors.len(), 6);
    assert_eq!(settings.indentation.style, Indentation::Tabs);
    assert!(!settings.build.keep_logs);
    assert_eq!(settings.font.size, 15.0);
    assert_eq!(settings.root_document, None);
}

#[test]
fn enum_values_use_snake_case() {
    let (settings, errors) = load(r#"{"indentation":{"style":"tabs"},"build":{"save_before_build":"require_clean"}}"#);
    assert!(errors.is_empty());
    assert_eq!(settings.indentation.style, Indentation::Tabs);
    assert_eq!(settings.build.save_before_build, SaveBeforeBuild::RequireClean);
    for invalid in [r#"{"indentation":{"style":"Tabs"}}"#, r#"{"build":{"save_before_build":"never"}}"#] {
        assert_eq!(load(invalid).0, Settings::default(), "{invalid}");
    }
}

#[test]
fn unknown_fields_and_wrong_types_fall_back_to_defaults_with_one_message_naming_the_file() {
    for json in [
        r#"{"unknown":1}"#,
        r#"{"font":{"colour":"red"}}"#,
        r#"{"font":{"size":"large"}}"#,
        r#"{"indentation":{"width":-1}}"#,
        r#"{"build":{"keep_logs":"yes"}}"#,
        r#"{"root_document":5}"#,
        "null",
        "42",
        "",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(PROJECT_FILE);
        std::fs::write(&path, json).unwrap();
        let (settings, errors) = Settings::load(&path);
        assert_eq!(settings, Settings::default(), "{json}");
        assert_eq!(errors.len(), 1, "{json}");
        assert!(errors[0].contains(PROJECT_FILE), "{json}");
    }
}

#[test]
fn unreadable_settings_report_an_error_instead_of_silently_defaulting() {
    let directory = tempfile::tempdir().unwrap();
    let (settings, errors) = Settings::load(directory.path());
    assert_eq!(settings, Settings::default());
    assert_eq!(errors.len(), 1);
    let path = directory.path().join("binary.json");
    std::fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
    let (settings, errors) = Settings::load(&path);
    assert_eq!(settings, Settings::default());
    assert_eq!(errors.len(), 1);
}

#[test]
fn saved_settings_round_trip_every_field() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(PROJECT_FILE);
    let mut settings = Settings::default();
    settings.font.family = Some("Menlo".into());
    settings.font.size = 12.5;
    settings.indentation.style = Indentation::Tabs;
    settings.indentation.width = 8;
    settings.build = BuildSettings {
        output_directory: "out put".into(),
        synctex: false,
        keep_logs: false,
        keep_intermediates: true,
        reruns: Some(3),
        save_before_build: SaveBeforeBuild::RequireClean,
    };
    settings.root_document = Some("src/main.tex".into());
    settings.save(&path).unwrap();
    assert_eq!(Settings::load(&path), (settings, Vec::new()));
}

#[test]
fn saving_invalid_settings_fails_and_leaves_existing_files_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(PROJECT_FILE);
    let invalid = Settings {
        root_document: Some("/abs.tex".into()),
        ..Settings::default()
    };
    assert!(invalid.save(&path).is_err());
    assert!(!path.exists());
    std::fs::write(&path, "{\"font\":{\"size\":20}}").unwrap();
    assert!(invalid.save(&path).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"font\":{\"size\":20}}");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn saving_into_a_missing_directory_reports_an_error() {
    let directory = tempfile::tempdir().unwrap();
    assert!(Settings::default().save(&directory.path().join("missing/.ligature.json")).is_err());
}

#[test]
fn legacy_build_keys_are_accepted_but_other_unknown_build_keys_are_rejected() {
    for legacy in [
        r#"{"build":{"program":null}}"#,
        r#"{"build":{"arguments":["-x"]}}"#,
        r#"{"build":{"engine":"xelatex"}}"#,
        r#"{"build":{"executable_path":"/bin/tex"}}"#,
    ] {
        assert!(serde_json::from_str::<Settings>(legacy).is_ok(), "{legacy}");
    }
    assert!(serde_json::from_str::<Settings>(r#"{"build":{"shell_escape":true}}"#).is_err());
}

#[test]
fn missing_sessions_are_empty_and_corrupt_sessions_are_errors() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(Session::load(&directory.path().join("none.json")).unwrap(), Session::default());
    for (index, json) in ["{broken", "42", r#"{"documents":"main.tex"}"#, r#"{"extra":1}"#].into_iter().enumerate() {
        let path = directory.path().join(format!("session{index}.json"));
        std::fs::write(&path, json).unwrap();
        let error = Session::load(&path).unwrap_err();
        assert!(error.starts_with("Invalid session"), "{json}: {error}");
    }
}

#[test]
fn sessions_create_missing_parent_directories_and_preserve_document_order() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("deep/er/session.json");
    let session = Session {
        project: Some(PathBuf::from("/projects/my thesis")),
        documents: vec!["b.tex".into(), "a.tex".into(), "日本.tex".into()],
        active_document: Some("a.tex".into()),
    };
    session.save(&path).unwrap();
    assert_eq!(Session::load(&path).unwrap(), session);
}

#[test]
fn partial_sessions_fill_missing_fields_with_defaults() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.json");
    std::fs::write(&path, r#"{"documents":["a.tex"]}"#).unwrap();
    let session = Session::load(&path).unwrap();
    assert_eq!(session.documents, [PathBuf::from("a.tex")]);
    assert_eq!(session.project, None);
    assert_eq!(session.active_document, None);
}

#[test]
fn overwriting_a_session_replaces_it_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("session.json");
    Session {
        documents: vec!["old.tex".into()],
        ..Session::default()
    }
    .save(&path)
    .unwrap();
    Session::default().save(&path).unwrap();
    assert_eq!(Session::load(&path).unwrap(), Session::default());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}
