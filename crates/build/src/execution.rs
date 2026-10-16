use crate::{BuildRequest, missing_package};
use ligature_diagnostics::{BuildId, Diagnostic, Provider, Severity, parse_tex};
use ligature_toolchain::{BundledResources, ENGINE_VERSION};
use std::{
    collections::BTreeSet,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock},
    thread,
    time::Duration,
};

pub const LOG_LIMIT: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BuildState {
    #[default]
    Idle,
    Running,
    Succeeded,
    Failed,
    Canceled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    Installation,
    Typesetting,
    Output,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildFailure {
    pub kind: FailureKind,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct BuildArtifacts {
    pub temporary_source: Option<crate::TemporarySource>,
    pub pdf: PathBuf,
    pub log: Option<PathBuf>,
    pub synctex: Option<PathBuf>,
    pub source_directory: PathBuf,
    pub root_document: PathBuf,
    pub sources: Arc<crate::SourceSnapshot>,
    pub sources_changed: bool,
}

#[derive(Clone, Debug, Default)]
pub struct BuildSnapshot {
    pub temporary_source: Option<crate::TemporarySource>,
    pub id: BuildId,
    pub state: BuildState,
    pub output: String,
    pub truncated: bool,
    pub failure: Option<BuildFailure>,
    pub artifacts: Option<BuildArtifacts>,
    pub diagnostics: Vec<Diagnostic>,
    pub sources: Option<Arc<crate::SourceSnapshot>>,
}

#[derive(Default)]
struct Shared {
    snapshot: BuildSnapshot,
    canceled: bool,
    started: bool,
    tex_log_available: bool,
    status_output: Option<String>,
}

#[derive(Clone)]
pub struct BuildJob(Arc<Mutex<Shared>>);

impl Default for BuildJob {
    fn default() -> Self {
        static NEXT_BUILD: AtomicU64 = AtomicU64::new(1);
        Self(Arc::new(Mutex::new(Shared {
            snapshot: BuildSnapshot {
                id: BuildId(NEXT_BUILD.fetch_add(1, Ordering::Relaxed)),
                ..BuildSnapshot::default()
            },
            ..Shared::default()
        })))
    }
}

fn failure_diagnostic(id: BuildId, failure: &BuildFailure) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        provider: Provider::Build(id),
        message: failure.message.clone(),
        path: None,
        range: None,
    }
}

fn diagnostic_key(diagnostic: &Diagnostic) -> (Severity, Option<&std::path::Path>, Option<&ligature_diagnostics::SourceRange>, &str) {
    (
        diagnostic.severity,
        diagnostic.path.as_deref(),
        diagnostic.range.as_ref(),
        diagnostic.message.trim_end_matches('.').trim_start_matches("LaTeX Warning: "),
    )
}

fn merge_diagnostics(existing: &mut Vec<Diagnostic>, parsed: Vec<Diagnostic>, tex_log_available: bool) {
    let mut seen: BTreeSet<_> = existing.iter().map(diagnostic_key).collect();
    let mut has_error = existing.iter().any(|diagnostic| diagnostic.severity == Severity::Error);
    let keep: Vec<_> = parsed
        .iter()
        .map(|diagnostic| {
            if (tex_log_available && diagnostic.severity == Severity::Warning && diagnostic.path.is_some())
                || (has_error && diagnostic.severity == Severity::Error && diagnostic.path.is_none())
            {
                return false;
            }
            if !seen.insert(diagnostic_key(diagnostic)) {
                return false;
            }
            has_error |= diagnostic.severity == Severity::Error;
            true
        })
        .collect();
    drop(seen);
    existing.extend(parsed.into_iter().zip(keep).filter_map(|(diagnostic, keep)| keep.then_some(diagnostic)));
    existing.truncate(ligature_diagnostics::MAX_DIAGNOSTICS);
}

impl BuildJob {
    pub fn fail_installation(&self, message: String) {
        let mut shared = self.0.lock().unwrap();
        if shared.started {
            return;
        }
        shared.started = true;
        shared.snapshot.state = if shared.canceled { BuildState::Canceled } else { BuildState::Failed };
        if !shared.canceled {
            shared.snapshot.diagnostics = vec![failure_diagnostic(
                shared.snapshot.id,
                &BuildFailure {
                    kind: FailureKind::Installation,
                    message: message.clone(),
                },
            )];
            shared.snapshot.failure = Some(BuildFailure {
                kind: FailureKind::Installation,
                message,
            });
        }
    }

    pub fn snapshot(&self) -> BuildSnapshot {
        self.0.lock().unwrap().snapshot.clone()
    }

    pub fn state(&self) -> BuildState {
        self.0.lock().unwrap().snapshot.state
    }

    pub fn cancel(&self) {
        let mut shared = self.0.lock().unwrap();
        if matches!(shared.snapshot.state, BuildState::Idle | BuildState::Running) {
            shared.canceled = true;
        }
    }

    fn canceled(&self) -> bool {
        self.0.lock().unwrap().canceled
    }

    /// Execute once on a background worker. Cancellation kills and reaps the child.
    pub fn run(&self, request: &BuildRequest, resources: &BundledResources) {
        self.run_request(request, resources, None);
    }

    pub fn run_text(&self, build: &crate::TextBuild, resources: &BundledResources) {
        self.run_request(&build.request, resources, Some(build.source.clone()));
    }

    fn run_request(&self, request: &BuildRequest, resources: &BundledResources, source: Option<crate::TemporarySource>) {
        {
            let mut shared = self.0.lock().unwrap();
            if shared.started {
                return;
            }
            shared.started = true;
            shared.snapshot.temporary_source = source;
            shared.snapshot.state = BuildState::Running;
        }
        let result = self.execute(request, resources);
        let output = {
            let mut shared = self.0.lock().unwrap();
            (!shared.canceled).then(|| {
                let output = shared.status_output.take().unwrap_or_else(|| shared.snapshot.output.clone());
                (shared.snapshot.id, output)
            })
        };
        let parsed = output.map_or_else(Vec::new, |(id, output)| {
            parse_tex(&output, request.filesystem_root(), request.root_document(), id)
        });
        let mut shared = self.0.lock().unwrap();
        if shared.canceled {
            drop(result);
            shared.snapshot.state = BuildState::Canceled;
            shared.snapshot.diagnostics.clear();
            shared.snapshot.temporary_source = None;
        } else {
            let tex_log_available = shared.tex_log_available;
            merge_diagnostics(&mut shared.snapshot.diagnostics, parsed, tex_log_available);
            match result {
                Ok((directory, mut artifacts)) => {
                    artifacts.temporary_source.clone_from(&shared.snapshot.temporary_source);
                    let _ = directory.keep();
                    shared.snapshot.state = BuildState::Succeeded;
                    shared.snapshot.artifacts = Some(artifacts);
                }
                Err(failure) => {
                    if failure.kind == FailureKind::Installation {
                        shared.snapshot.diagnostics.clear();
                    }
                    if shared
                        .snapshot
                        .diagnostics
                        .iter()
                        .all(|diagnostic| diagnostic.severity != Severity::Error)
                    {
                        let diagnostic = failure_diagnostic(shared.snapshot.id, &failure);
                        shared.snapshot.diagnostics.push(diagnostic);
                    }
                    shared.snapshot.state = BuildState::Failed;
                    shared.snapshot.failure = Some(failure);
                }
            }
        }
    }

    fn execute(&self, request: &BuildRequest, resources: &BundledResources) -> Result<(tempfile::TempDir, BuildArtifacts), BuildFailure> {
        let failure = |kind, message| BuildFailure { kind, message };
        if self.canceled() {
            return Err(failure(FailureKind::Output, "Build canceled".into()));
        }
        let bundle = request
            .create_session(resources)
            .map_err(|message| {
                let kind = if message.starts_with("Installation error") {
                    FailureKind::Installation
                } else {
                    FailureKind::Output
                };
                failure(kind, message)
            })?
            .bundle;
        let _lease = OutputLease::acquire(request.output_directory()).map_err(|message| failure(FailureKind::Output, message))?;
        let sources = Arc::new(crate::SourceSnapshot::capture(&request.project_root, request.output_directory()));
        self.0.lock().unwrap().snapshot.sources = Some(Arc::clone(&sources));
        crate::version::verify(&resources.executable, || {
            let mut version = Command::new(&resources.executable)
                .arg("--version")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|error| format!("Could not launch bundled Tectonic: {error}"))?;
            let version_status = self.collect(&mut version)?;
            Ok(version_status.success() && self.snapshot().output.trim() == format!("Tectonic {ENGINE_VERSION}"))
        })
        .map_err(|message| failure(FailureKind::Installation, message))?;
        self.0.lock().unwrap().snapshot.output.clear();
        let directory = tempfile::Builder::new()
            .prefix("build-")
            .tempdir_in(request.output_directory())
            .map_err(|error| failure(FailureKind::Output, format!("Could not prepare build output: {error}")))?;
        if self.canceled() {
            return Err(failure(FailureKind::Output, "Build canceled".into()));
        }
        let mut diagnostic_request = request.clone();
        diagnostic_request.options.keep_logs = true;
        let mut child = diagnostic_request
            .command_with_output(resources, directory.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| failure(FailureKind::Installation, format!("Could not launch bundled Tectonic: {error}")))?;
        let status = self.collect(&mut child).map_err(|message| failure(FailureKind::Output, message))?;
        {
            let mut shared = self.0.lock().unwrap();
            shared.status_output = Some(shared.snapshot.output.clone());
        }
        let stem = request.root_document().file_stem().unwrap().to_string_lossy();
        let artifact = |extension| directory.path().join(format!("{stem}.{extension}"));
        let log = artifact("log");
        let bibliography_log = artifact("blg");
        for path in [&log, &bibliography_log] {
            if let Ok(mut file) = std::fs::File::open(path) {
                let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
                if length > LOG_LIMIT as u64 {
                    self.0.lock().unwrap().snapshot.truncated = true;
                    let _ = file.seek(SeekFrom::End(-(LOG_LIMIT as i64)));
                }
                let mut bytes = Vec::with_capacity(length.min(LOG_LIMIT as u64) as usize);
                if file.take(LOG_LIMIT as u64).read_to_end(&mut bytes).is_ok() {
                    if path == &log && length <= LOG_LIMIT as u64 {
                        let id = self.0.lock().unwrap().snapshot.id;
                        let diagnostics = parse_tex(&String::from_utf8_lossy(&bytes), request.filesystem_root(), request.root_document(), id);
                        let mut shared = self.0.lock().unwrap();
                        shared.snapshot.diagnostics.extend(diagnostics);
                        shared.tex_log_available = true;
                    }
                    self.append(format!("\n--- {} ---\n", path.file_name().unwrap().to_string_lossy()).as_bytes());
                    self.append(&bytes);
                }
            }
        }
        if !request.options.keep_logs {
            let _ = std::fs::remove_file(&log);
            let _ = std::fs::remove_file(&bibliography_log);
        }
        if !status.success() {
            let output = self.snapshot().output;
            let message = if let Some(name) = missing_package(&output)
                && !bundle.contains(&name)
            {
                format!("Unsupported project package or document class '{name}'. Add project-local files or download the full TeX bundle.")
            } else {
                format!("Typesetting failed ({status})")
            };
            return Err(failure(FailureKind::Typesetting, message));
        }
        let pdf = artifact("pdf");
        if !pdf.is_file() {
            return Err(failure(FailureKind::Output, "Tectonic completed without producing a PDF".into()));
        }
        let synctex = artifact("synctex.gz");
        if self.canceled() {
            return Err(failure(FailureKind::Output, "Build canceled".into()));
        }
        let artifacts = BuildArtifacts {
            temporary_source: None,
            pdf,
            log: (request.options.keep_logs && log.is_file()).then_some(log),
            synctex: synctex.is_file().then_some(synctex),
            source_directory: request.filesystem_root().to_path_buf(),
            root_document: request.root_document().to_path_buf(),
            sources_changed: !sources.matches_disk(&request.project_root, request.output_directory()),
            sources,
        };
        Ok((directory, artifacts))
    }

    fn collect(&self, child: &mut Child) -> Result<ExitStatus, String> {
        let stdout = self.reader(child.stdout.take().unwrap());
        let stderr = self.reader(child.stderr.take().unwrap());
        let status = loop {
            if self.canceled() {
                let _ = child.kill();
                break child.wait();
            }
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => thread::sleep(Duration::from_millis(15)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(error);
                }
            }
        };
        let stdout = stdout.join();
        let stderr = stderr.join();
        for read in [stdout, stderr] {
            read.map_err(|_| "Build output reader stopped unexpectedly")?
                .map_err(|error| format!("Could not read engine output: {error}"))?;
        }
        status.map_err(|error| format!("Could not wait for Tectonic: {error}"))
    }

    fn reader(&self, mut reader: impl Read + Send + 'static) -> thread::JoinHandle<std::io::Result<()>> {
        let job = self.clone();
        thread::spawn(move || {
            let mut bytes = [0; 8192];
            loop {
                let count = reader.read(&mut bytes)?;
                if count == 0 {
                    return Ok(());
                }
                job.append(&bytes[..count]);
            }
        })
    }

    fn append(&self, bytes: &[u8]) {
        let mut shared = self.0.lock().unwrap();
        let snapshot = &mut shared.snapshot;
        snapshot.output.push_str(&String::from_utf8_lossy(bytes));
        if snapshot.output.len() > LOG_LIMIT {
            let mut start = snapshot.output.len() - LOG_LIMIT;
            while !snapshot.output.is_char_boundary(start) {
                start += 1;
            }
            snapshot.output.drain(..start);
            snapshot.truncated = true;
        }
    }
}

pub(super) struct OutputLease(PathBuf);

fn outputs() -> &'static Mutex<BTreeSet<PathBuf>> {
    static OUTPUTS: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    OUTPUTS.get_or_init(Mutex::default)
}

impl OutputLease {
    pub(super) fn acquire(path: &std::path::Path) -> Result<Self, String> {
        let path = std::fs::canonicalize(path).map_err(|error| error.to_string())?;
        if !outputs().lock().unwrap().insert(path.clone()) {
            return Err("Another build owns this output directory. Retry after it finishes.".into());
        }
        Ok(Self(path))
    }
}

impl Drop for OutputLease {
    fn drop(&mut self) {
        outputs().lock().unwrap().remove(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_diagnostics_preserves_order_and_normalizes_duplicates() {
        let warning = |message: &str, path: Option<&str>| Diagnostic {
            severity: Severity::Warning,
            provider: Provider::Build(BuildId(1)),
            message: message.into(),
            path: path.map(PathBuf::from),
            range: None,
        };
        let first = warning("LaTeX Warning: First.", Some("main.tex"));
        let second = warning("Second", Some("child.tex"));
        let mut existing = vec![first.clone()];
        merge_diagnostics(
            &mut existing,
            vec![warning("First", Some("main.tex")), second.clone(), second.clone()],
            false,
        );
        assert_eq!(existing, vec![first, second]);
        let generic = warning("Generic warning", None);
        merge_diagnostics(&mut existing, vec![warning("Log warning", Some("main.tex")), generic.clone()], true);
        assert_eq!(existing.last(), Some(&generic));
        assert_eq!(existing.len(), 3);

        let failure = failure_diagnostic(
            BuildId(1),
            &BuildFailure {
                kind: FailureKind::Typesetting,
                message: "failed".into(),
            },
        );
        merge_diagnostics(&mut existing, vec![failure.clone(), failure], false);
        assert_eq!(existing.len(), 4);
        let located = Diagnostic {
            path: Some("main.tex".into()),
            ..existing.last().unwrap().clone()
        };
        merge_diagnostics(&mut existing, vec![located.clone()], false);
        assert_eq!(existing.last(), Some(&located));
    }

    #[test]
    fn installation_failures_have_no_source_location_and_canceled_jobs_have_no_diagnostics() {
        let job = BuildJob::default();
        job.fail_installation("Bundle missing".into());
        let snapshot = job.snapshot();
        assert_eq!(snapshot.state, BuildState::Failed);
        assert_eq!(snapshot.diagnostics.len(), 1);
        assert_eq!(snapshot.diagnostics[0].path, None);
        assert_eq!(snapshot.diagnostics[0].range, None);
        let canceled = BuildJob::default();
        canceled.cancel();
        canceled.fail_installation("Bundle missing".into());
        assert!(canceled.snapshot().diagnostics.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn output_ownership_resolves_aliases_and_releases_on_drop() {
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("output");
        let alias = directory.path().join("alias");
        std::fs::create_dir(&output).unwrap();
        std::os::unix::fs::symlink(&output, &alias).unwrap();
        let lease = OutputLease::acquire(&output).unwrap();
        assert!(OutputLease::acquire(&alias).is_err());
        let other = tempfile::tempdir().unwrap();
        let _independent = OutputLease::acquire(other.path()).unwrap();
        drop(lease);
        assert!(OutputLease::acquire(&alias).is_ok());
    }
}
