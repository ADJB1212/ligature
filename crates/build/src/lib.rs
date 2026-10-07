//! Validated disk-based build requests and bundled Tectonic processes.

use ligature_project::Project;
use ligature_settings::{BuildSettings, SaveBeforeBuild};
use ligature_toolchain::{BundledResources, ENGINE_VERSION};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

mod execution;
mod sources;
pub use execution::{BuildArtifacts, BuildFailure, BuildJob, BuildSnapshot, BuildState, FailureKind, LOG_LIMIT};
pub use sources::SourceSnapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionOptions {
    pub synctex: bool,
    pub keep_logs: bool,
    pub keep_intermediates: bool,
    pub reruns: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildRequest {
    project_root: PathBuf,
    root_document: PathBuf,
    output_directory: PathBuf,
    options: SessionOptions,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BuildPreparation {
    SaveRequired(Vec<PathBuf>),
    Ready(BuildRequest),
}

impl BuildRequest {
    /// Save every returned document, then prepare again with current dirty paths.
    /// Failed or canceled saves must not launch a session.
    pub fn prepare(project: &Project, settings: &BuildSettings, dirty_documents: &[PathBuf]) -> Result<BuildPreparation, String> {
        let mut validated = settings.clone();
        let errors = validated.validate();
        if !errors.is_empty() {
            return Err(errors.join("; "));
        }
        let project = validated_project(project)?;
        let root_document = project.root.join(project.root_document.as_ref().unwrap());
        if root_document.file_name().and_then(|name| name.to_str()).is_none() {
            return Err("The root document filename must be UTF-8".into());
        }
        let output_directory = project.root.join(&settings.output_directory);
        validate_output(&project.root, &output_directory)?;
        let dirty: BTreeSet<_> = dirty_documents
            .iter()
            .filter_map(|path| project.relative(path).map(|relative| project.root.join(relative)))
            .collect();
        if !dirty.is_empty() {
            return match settings.save_before_build {
                SaveBeforeBuild::SaveAll => Ok(BuildPreparation::SaveRequired(dirty.into_iter().collect())),
                SaveBeforeBuild::RequireClean => Err("Save dirty project documents before building".into()),
            };
        }
        Ok(BuildPreparation::Ready(Self {
            project_root: project.root,
            root_document,
            output_directory,
            options: SessionOptions {
                synctex: settings.synctex,
                keep_logs: settings.keep_logs,
                keep_intermediates: settings.keep_intermediates,
                reruns: settings.reruns,
            },
        }))
    }

    pub fn root_document(&self) -> &Path {
        &self.root_document
    }
    pub fn filesystem_root(&self) -> &Path {
        self.root_document.parent().unwrap()
    }
    pub fn output_directory(&self) -> &Path {
        &self.output_directory
    }
    pub fn options(&self) -> &SessionOptions {
        &self.options
    }

    /// Prepare a bundled engine command for execution on a background worker.
    pub fn create_session(&self, resources: &BundledResources) -> Result<BuildSession, String> {
        let mut project = Project::open(&self.project_root)?;
        project.set_root_document(&self.root_document)?;
        validate_output(&project.root, &self.output_directory)?;
        resources.validate()?;
        resources.prepare_cache()?;
        std::fs::create_dir_all(&self.output_directory).map_err(|error| format!("Could not create build output directory: {error}"))?;
        Ok(BuildSession {
            command: self.command(resources),
            resources: resources.clone(),
            output_directory: self.output_directory.clone(),
        })
    }

    fn command(&self, resources: &BundledResources) -> Command {
        self.command_with_output(resources, &self.output_directory)
    }

    fn command_with_output(&self, resources: &BundledResources, output_directory: &Path) -> Command {
        let mut command = Command::new(&resources.executable);
        command
            .current_dir(self.filesystem_root())
            .env("TECTONIC_CACHE_DIR", &resources.format_cache)
            .stdin(Stdio::null())
            .args(["--color", "never", "--untrusted", "--only-cached", "--outfmt", "pdf", "--bundle"])
            .arg(&resources.bundle)
            .arg("--outdir")
            .arg(output_directory);
        if self.options.synctex {
            command.arg("--synctex");
        }
        if self.options.keep_logs {
            command.arg("--keep-logs");
        }
        if self.options.keep_intermediates {
            command.arg("--keep-intermediates");
        }
        if let Some(reruns) = self.options.reruns {
            command.arg("--reruns").arg(reruns.to_string());
        }
        command.arg("--").arg(&self.root_document);
        command
    }
}

pub struct BuildSession {
    command: Command,
    resources: BundledResources,
    output_directory: PathBuf,
}

impl BuildSession {
    pub fn run(&mut self) -> Result<Output, String> {
        let _lease = execution::OutputLease::acquire(&self.output_directory)?;
        self.resources.validate()?;
        let version = Command::new(&self.resources.executable)
            .arg("--version")
            .output()
            .map_err(|error| format!("Installation error: could not launch bundled Tectonic: {error}. Reinstall Ligature."))?;
        if !version.status.success() || String::from_utf8_lossy(&version.stdout).trim() != format!("Tectonic {ENGINE_VERSION}") {
            return Err(format!(
                "Installation error: bundled Tectonic must be version {ENGINE_VERSION}. Reinstall Ligature."
            ));
        }
        let output = self
            .command
            .output()
            .map_err(|error| format!("Could not launch bundled Tectonic: {error}"))?;
        if !output.status.success() {
            let diagnostics = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            if let Some(name) = missing_package(&diagnostics)
                && !self.resources.validate()?.contains(&name)
            {
                return Err(format!(
                    "Unsupported project package or document class '{name}': it is not included in the shipped TeX bundle. Add compatible project-local files or use a supported package.\n{diagnostics}"
                ));
            }
            return Err(format!(
                "Typesetting failed ({}):\n{}{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(output)
    }
}

fn missing_package(diagnostics: &str) -> Option<String> {
    let after = diagnostics.split_once("LaTeX Error: File `")?.1;
    let (name, rest) = after.split_once('\'')?;
    if rest.starts_with(" not found") && (name.ends_with(".sty") || name.ends_with(".cls")) {
        Some(name.into())
    } else {
        None
    }
}

fn validated_project(project: &Project) -> Result<Project, String> {
    let root = project.root_document.as_ref().ok_or("Select a root TeX document before building")?;
    let mut validated = Project::open(&project.root)?;
    validated.set_root_document(root)?;
    Ok(validated)
}

fn validate_output(project_root: &Path, output: &Path) -> Result<(), String> {
    let mut ancestor = output;
    while !ancestor.try_exists().map_err(|error| error.to_string())? {
        ancestor = ancestor.parent().ok_or("Invalid build output directory")?;
    }
    let resolved = std::fs::canonicalize(ancestor).map_err(|error| error.to_string())?;
    if !resolved.starts_with(project_root) || !resolved.is_dir() {
        return Err("The build output directory must be a directory within the project".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(directory: &Path) -> Project {
        std::fs::create_dir_all(directory.join("chapter one")).unwrap();
        std::fs::write(directory.join("chapter one/main α.tex"), "\\bye").unwrap();
        let mut project = Project::open(directory).unwrap();
        project.set_root_document(Path::new("chapter one/main α.tex")).unwrap();
        project
    }

    fn ready(project: &Project, settings: &BuildSettings) -> BuildRequest {
        let BuildPreparation::Ready(request) = BuildRequest::prepare(project, settings, &[]).unwrap() else {
            panic!("expected ready request")
        };
        request
    }

    #[test]
    fn request_resolves_unicode_and_spaced_paths_without_changing_working_directory() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let cwd = std::env::current_dir().unwrap();
        let settings = BuildSettings {
            output_directory: "output α/files".into(),
            ..BuildSettings::default()
        };
        let request = ready(&project, &settings);
        assert_eq!(request.root_document(), project.root.join("chapter one/main α.tex"));
        assert_eq!(request.filesystem_root(), project.root.join("chapter one"));
        assert_eq!(request.output_directory(), project.root.join("output α/files"));
        assert!(request.options().synctex && request.options().keep_logs);
        assert_eq!(std::env::current_dir().unwrap(), cwd);
        assert!(!request.output_directory().exists());
    }

    #[test]
    fn missing_deleted_and_non_tex_roots_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let mut project = project(directory.path());
        std::fs::remove_file(project.root.join(project.root_document.as_ref().unwrap())).unwrap();
        assert!(BuildRequest::prepare(&project, &BuildSettings::default(), &[]).is_err());
        std::fs::write(project.root.join("notes.txt"), "text").unwrap();
        project.root_document = Some("notes.txt".into());
        assert!(BuildRequest::prepare(&project, &BuildSettings::default(), &[]).is_err());
        project.root_document = None;
        assert!(BuildRequest::prepare(&project, &BuildSettings::default(), &[]).is_err());
    }

    #[test]
    fn all_dirty_project_documents_must_be_saved_before_a_request_is_ready() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let dirty = vec![
            PathBuf::from("references.bib"),
            PathBuf::from("chapter one/main α.tex"),
            PathBuf::from("references.bib"),
            directory.path().parent().unwrap().join("other.tex"),
        ];
        let BuildPreparation::SaveRequired(paths) = BuildRequest::prepare(&project, &BuildSettings::default(), &dirty).unwrap() else {
            panic!("expected saves")
        };
        assert_eq!(
            paths,
            vec![project.root.join("chapter one/main α.tex"), project.root.join("references.bib")]
        );
        let settings = BuildSettings {
            save_before_build: SaveBeforeBuild::RequireClean,
            ..BuildSettings::default()
        };
        assert!(BuildRequest::prepare(&project, &settings, &dirty).is_err());
        assert!(matches!(BuildRequest::prepare(&project, &settings, &[]), Ok(BuildPreparation::Ready(_))));
    }

    #[test]
    fn invalid_output_and_session_options_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        for output in ["../outside", ""] {
            let settings = BuildSettings {
                output_directory: output.into(),
                ..BuildSettings::default()
            };
            assert!(BuildRequest::prepare(&project, &settings, &[]).is_err());
        }
        let settings = BuildSettings {
            reruns: Some(11),
            ..BuildSettings::default()
        };
        assert!(BuildRequest::prepare(&project, &settings, &[]).is_err());
    }

    #[cfg(unix)]
    fn resources(project: &Project, script: &str) -> BundledResources {
        use std::os::unix::fs::PermissionsExt;

        let resources = BundledResources::new(&project.root.join("resources α"), &project.root.join("cache α")).unwrap();
        std::fs::create_dir_all(resources.executable.parent().unwrap()).unwrap();
        std::fs::create_dir_all(resources.bundle.parent().unwrap()).unwrap();
        let script = script.replacen(
            "#!/bin/sh\n",
            &format!("#!/bin/sh\nif [ \"$1\" = '--version' ]; then printf 'Tectonic {ENGINE_VERSION}\\n'; exit 0; fi\n"),
            1,
        );
        std::fs::write(&resources.executable, script).unwrap();
        std::fs::set_permissions(&resources.executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let staged = Path::new(ligature_toolchain::BUILT_RESOURCE_DIRECTORY);
        std::fs::copy(staged.join("tex/ligature-bundle-1.zip"), &resources.bundle).unwrap();
        std::fs::create_dir_all(resources.fonts.join("otf")).unwrap();
        for entry in std::fs::read_dir(staged.join("newcomputermodern/otf")).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), resources.fonts.join("otf").join(entry.file_name())).unwrap();
        }
        resources
    }

    #[cfg(unix)]
    #[test]
    fn bundled_process_receives_paths_options_and_cache_without_changing_working_directory() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let settings = BuildSettings {
            reruns: Some(2),
            keep_intermediates: true,
            ..BuildSettings::default()
        };
        let request = ready(&project, &settings);
        let resources = resources(
            &project,
            "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$TECTONIC_CACHE_DIR\" \"$@\"\nprintf 'engine diagnostic' >&2\n",
        );
        let cwd = std::env::current_dir().unwrap();
        let output = request.create_session(&resources).unwrap().run().unwrap();
        let stdout = String::from_utf8(output.stdout).unwrap();
        let lines: Vec<_> = stdout.lines().collect();
        assert_eq!(
            lines,
            vec![
                request.filesystem_root().to_str().unwrap(),
                resources.format_cache.to_str().unwrap(),
                "--color",
                "never",
                "--untrusted",
                "--only-cached",
                "--outfmt",
                "pdf",
                "--bundle",
                resources.bundle.to_str().unwrap(),
                "--outdir",
                request.output_directory().to_str().unwrap(),
                "--synctex",
                "--keep-logs",
                "--keep-intermediates",
                "--reruns",
                "2",
                "--",
                request.root_document().to_str().unwrap(),
            ]
        );
        assert_eq!(output.stderr, b"engine diagnostic");
        assert!(request.output_directory().is_dir());
        assert!(resources.format_cache.is_dir());
        assert_eq!(std::env::current_dir().unwrap(), cwd);
    }

    #[cfg(unix)]
    #[test]
    fn process_failures_include_exit_status_and_engine_output() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let request = ready(&project, &BuildSettings::default());
        let resources = resources(&project, "#!/bin/sh\nprintf 'TeX error\\n'\nprintf 'missing input\\n' >&2\nexit 7\n");
        let error = request.create_session(&resources).unwrap().run().unwrap_err();
        assert!(error.contains("7") && error.contains("TeX error") && error.contains("missing input"));
        std::fs::set_permissions(&resources.executable, {
            use std::os::unix::fs::PermissionsExt;
            std::fs::Permissions::from_mode(0o644)
        })
        .unwrap();
        assert!(
            request
                .create_session(&resources)
                .unwrap()
                .run()
                .unwrap_err()
                .contains("Installation error: could not launch")
        );
    }

    #[test]
    fn automatic_reruns_and_disabled_options_are_not_passed_to_the_engine() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let settings = BuildSettings {
            synctex: false,
            keep_logs: false,
            keep_intermediates: false,
            reruns: None,
            ..BuildSettings::default()
        };
        let request = ready(&project, &settings);
        let resources = BundledResources::new(&project.root.join("resources"), &project.root.join("cache")).unwrap();
        let command = request.command(&resources);
        let args: Vec<_> = command.get_args().collect();
        for flag in ["--synctex", "--keep-logs", "--keep-intermediates", "--reruns"] {
            assert!(!args.iter().any(|arg| *arg == flag));
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn first_and_repeat_builds_use_shipped_fonts_and_bibtex_with_an_empty_offline_cache() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let request = ready(&project, &BuildSettings::default());
        std::fs::write(
            request.root_document(),
            concat!("\\documentclass{article}\n", include_str!("../../../tools/package-resources/smoke.tex")),
        )
        .unwrap();
        std::fs::write(
            request.filesystem_root().join("references.bib"),
            "@book{knuth,author={Donald Knuth},title={The TeXbook},year={1984},publisher={Addison-Wesley}}\n",
        )
        .unwrap();
        let resources = BundledResources::new(
            Path::new(ligature_toolchain::BUILT_RESOURCE_DIRECTORY),
            &directory.path().join("empty cache"),
        )
        .unwrap();
        assert!(!resources.format_cache.exists());
        for _ in 0..2 {
            let mut session = request.create_session(&resources).unwrap();
            session
                .command
                .env("HTTP_PROXY", "http://127.0.0.1:9")
                .env("HTTPS_PROXY", "http://127.0.0.1:9")
                .env("ALL_PROXY", "http://127.0.0.1:9");
            let output = session.run().unwrap();
            assert!(!String::from_utf8_lossy(&output.stdout).contains("downloading"));
            let pdf = std::fs::read(request.output_directory().join("main α.pdf")).unwrap();
            assert!(pdf.starts_with(b"%PDF-"));
            assert!(request.output_directory().join("main α.synctex.gz").is_file());
        }
        assert!(std::fs::read_dir(resources.format_cache.join("formats")).unwrap().next().is_some());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unsupported_project_packages_are_distinct_from_installation_errors() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let request = ready(&project, &BuildSettings::default());
        std::fs::write(
            request.root_document(),
            "\\documentclass{article}\n\\usepackage{ligature-not-shipped}\n\\begin{document}Test\\end{document}\n",
        )
        .unwrap();
        let resources = BundledResources::new(Path::new(ligature_toolchain::BUILT_RESOURCE_DIRECTORY), &directory.path().join("cache")).unwrap();
        let error = request.create_session(&resources).unwrap().run().unwrap_err();
        assert!(
            error.contains("Unsupported project package") && error.contains("ligature-not-shipped.sty"),
            "{error}"
        );
        std::fs::write(
            request.filesystem_root().join("ligature-not-shipped.sty"),
            "\\ProvidesPackage{ligature-not-shipped}\n",
        )
        .unwrap();
        request.create_session(&resources).unwrap().run().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn incompatible_engine_versions_are_installation_errors() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let request = ready(&project, &BuildSettings::default());
        let resources = resources(&project, "#!/bin/sh\nexit 0\n");
        std::fs::write(&resources.executable, "#!/bin/sh\nprintf 'Tectonic wrong-version\\n'\n").unwrap();
        assert!(
            request
                .create_session(&resources)
                .unwrap()
                .run()
                .unwrap_err()
                .contains("Installation error: bundled Tectonic must be version")
        );
    }

    #[cfg(unix)]
    #[test]
    fn cancellation_reaps_the_worker_and_releases_output_for_the_next_build() {
        use std::{
            sync::mpsc,
            thread,
            time::{Duration, Instant},
        };

        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let request = ready(&project, &BuildSettings::default());
        let resources = resources(&project, "#!/bin/sh\nprintf 'worker:%s\\n' \"$$\"\nwhile :; do :; done\n");
        let job = BuildJob::default();
        let worker = job.clone();
        let worker_request = request.clone();
        let worker_resources = resources.clone();
        let (finished, completion) = mpsc::channel();
        let thread = thread::spawn(move || {
            worker.run(&worker_request, &worker_resources);
            finished.send(()).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !job.snapshot().output.starts_with("worker:") && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let output = job.snapshot().output;
        if !output.starts_with("worker:") {
            job.cancel();
            completion.recv_timeout(Duration::from_secs(10)).unwrap();
            thread.join().unwrap();
            panic!("Worker did not start: {output}");
        }
        // A repeated request on the same job must not launch another worker.
        job.run(&request, &resources);
        let other = BuildJob::default();
        other.run(&request, &resources);
        let conflict = other.snapshot();
        let session_error = request.create_session(&resources).unwrap().run().unwrap_err();
        job.cancel();
        completion.recv_timeout(Duration::from_secs(10)).unwrap();
        thread.join().unwrap();
        assert!(output.starts_with("worker:"), "{output}");
        assert_eq!(conflict.state, BuildState::Failed);
        assert!(conflict.failure.unwrap().message.contains("owns this output directory"));
        assert!(session_error.contains("owns this output directory"));
        let pid = output.trim().strip_prefix("worker:").unwrap();
        assert!(
            !Command::new("/bin/kill")
                .args(["-0", pid])
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        let snapshot = job.snapshot();
        assert_eq!(snapshot.state, BuildState::Canceled);
        assert!(snapshot.artifacts.is_none());
        assert!(snapshot.failure.is_none());
        assert_eq!(std::fs::read_dir(request.output_directory()).unwrap().count(), 0);
        let _lease = execution::OutputLease::acquire(request.output_directory()).unwrap();
    }

    #[test]
    fn cancellation_before_launch_does_not_initialize_resources() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let request = ready(&project, &BuildSettings::default());
        let resources = BundledResources::new(&project.root.join("missing resources"), &project.root.join("cache")).unwrap();
        let job = BuildJob::default();
        job.cancel();
        job.run(&request, &resources);
        job.run(&request, &resources);
        assert_eq!(job.snapshot().state, BuildState::Canceled);
        assert!(job.snapshot().failure.is_none());
        assert!(!request.output_directory().exists());
        assert!(!resources.format_cache.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn successful_single_and_multi_file_jobs_preserve_artifacts_after_failure() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let request = ready(&project, &BuildSettings::default());
        let resources = BundledResources::new(Path::new(ligature_toolchain::BUILT_RESOURCE_DIRECTORY), &directory.path().join("cache")).unwrap();
        let mut previous = None;
        for body in ["Single file", "\\input{chapter α}"] {
            std::fs::write(request.filesystem_root().join("chapter α.tex"), "Included file").unwrap();
            std::fs::write(request.root_document(), format!("\\documentclass{{article}}\n\\usepackage{{newcomputermodern}}\n\\begin{{document}}\\special{{dvipdfmx:config z 0}}{body}\\end{{document}}\n")).unwrap();
            let job = BuildJob::default();
            job.run(&request, &resources);
            let snapshot = job.snapshot();
            assert_eq!(snapshot.state, BuildState::Succeeded, "{snapshot:?}");
            let artifacts = snapshot.artifacts.unwrap();
            let pdf = std::fs::read(&artifacts.pdf).unwrap();
            assert!(pdf.starts_with(b"%PDF-"));
            assert!(String::from_utf8_lossy(&pdf).contains("NewCM"));
            assert!(artifacts.log.as_ref().unwrap().is_file());
            let document = ligature_pdf::Document::load(&artifacts.pdf).unwrap();
            assert_eq!(document.pages().len(), 1);
            let raster = document.render(0, 1.0).unwrap();
            assert!(raster.bgra.as_chunks::<4>().0.iter().any(|pixel| pixel[..3] != [255, 255, 255]));
            let mapping =
                ligature_synctex::Mapping::load(artifacts.synctex.as_ref().unwrap(), request.filesystem_root(), document.pages().len()).unwrap();
            let source_path = if body.starts_with("\\input") {
                request.filesystem_root().join("chapter α.tex")
            } else {
                request.root_document().to_path_buf()
            };
            let source = ligature_synctex::SourcePosition {
                path: std::fs::canonicalize(&source_path).unwrap(),
                line: if body.starts_with("\\input") { 1 } else { 3 },
                column: 0,
            };
            let point = mapping.forward(&source).unwrap();
            assert_eq!(point.page, 1);
            assert!(point.x >= 0.0 && point.x < document.pages()[0].width);
            assert!(point.y >= 0.0 && point.y < document.pages()[0].height);
            assert_eq!(mapping.inverse(point).unwrap().path, source.path);
            assert!(!artifacts.sources_changed);
            if let Some(path) = previous {
                assert_ne!(artifacts.pdf, path);
                assert!(std::fs::metadata(path).is_ok());
            }
            previous = Some(artifacts.pdf);
        }
        let path = previous.unwrap();
        let pdf = std::fs::read(&path).unwrap();
        std::fs::write(
            request.root_document(),
            "\\documentclass{article}\\begin{document}\\undefinedcommand\\end{document}",
        )
        .unwrap();
        let job = BuildJob::default();
        job.run(&request, &resources);
        assert_eq!(job.snapshot().state, BuildState::Failed);
        assert_eq!(job.snapshot().failure.unwrap().kind, FailureKind::Typesetting);
        assert!(job.snapshot().artifacts.is_none());
        assert_eq!(std::fs::read(path).unwrap(), pdf);
        assert_eq!(std::fs::read_dir(request.output_directory()).unwrap().count(), 2);
    }

    #[test]
    fn session_creation_revalidates_roots_and_reports_missing_resources() {
        let directory = tempfile::tempdir().unwrap();
        let project = project(directory.path());
        let request = ready(&project, &BuildSettings::default());
        let resources = BundledResources::new(&project.root.join("resources"), &project.root.join("cache")).unwrap();
        let Err(error) = request.create_session(&resources) else {
            panic!("expected missing executable")
        };
        assert!(error.contains("bundled Tectonic executable"));
        std::fs::create_dir_all(resources.executable.parent().unwrap()).unwrap();
        std::fs::write(&resources.executable, "fixture").unwrap();
        let Err(error) = request.create_session(&resources) else {
            panic!("expected missing bundle")
        };
        assert!(error.contains("Installation error") && error.contains("ligature-bundle-1.zip"));
        std::fs::remove_file(request.root_document()).unwrap();
        assert!(request.create_session(&resources).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_cannot_move_root_or_output_outside_the_project() {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let mut project = project(directory.path());
        std::fs::write(outside.path().join("main.tex"), "\\bye").unwrap();
        std::os::unix::fs::symlink(outside.path(), project.root.join("external")).unwrap();
        let settings = BuildSettings {
            output_directory: "external/output".into(),
            ..BuildSettings::default()
        };
        assert!(BuildRequest::prepare(&project, &settings, &[]).is_err());
        project.root_document = Some("external/main.tex".into());
        assert!(BuildRequest::prepare(&project, &BuildSettings::default(), &[]).is_err());
    }
}
