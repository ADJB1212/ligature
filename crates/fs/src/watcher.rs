use crate::{FileState, Signature, resolve};
use notify::{Config, Event, EventKind, PollWatcher, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak, mpsc},
    thread,
    time::Duration,
};

struct Subscription {
    previous: FileState,
    sender: async_channel::Sender<FileState>,
}

#[derive(Default)]
struct Watched {
    signature: Option<Signature>,
    subscribers: Vec<Subscription>,
}

type Subscriptions = Arc<Mutex<BTreeMap<PathBuf, Watched>>>;

fn reconcile(subscriptions: &Subscriptions, event: notify::Result<Event>) {
    if matches!(&event, Ok(event) if matches!(event.kind, EventKind::Access(_))) {
        return;
    }
    let mut subscriptions = subscriptions.lock().unwrap();
    for (path, watched) in subscriptions.iter_mut() {
        if let Ok(event) = &event
            && !event.paths.is_empty()
            && !event.paths.iter().any(|changed| path.starts_with(changed) || changed.starts_with(path))
        {
            continue;
        }
        let signature = Signature::of(path).ok();
        if signature.is_some() && signature == watched.signature {
            continue;
        }
        let current = FileState::read(path);
        watched.signature = signature;
        for subscriber in &mut watched.subscribers {
            if current != subscriber.previous {
                subscriber.previous = current.clone();
                let _ = subscriber.sender.force_send(subscriber.previous.clone());
            }
        }
    }
}

struct DirectoryWatch {
    users: usize,
    native: bool,
}

struct WatchService {
    native: Option<RecommendedWatcher>,
    polling: Option<PollWatcher>,
    directories: BTreeMap<PathBuf, DirectoryWatch>,
    subscriptions: Subscriptions,
    _reconcile_stop: mpsc::Sender<()>,
}

impl WatchService {
    fn new() -> Self {
        let subscriptions = Subscriptions::default();
        let callback = Arc::clone(&subscriptions);
        let native = notify::recommended_watcher(move |event| reconcile(&callback, event)).ok();
        let (stop, stopped) = mpsc::channel();
        let periodic = Arc::downgrade(&subscriptions);
        thread::spawn(move || {
            while matches!(stopped.recv_timeout(Duration::from_millis(500)), Err(mpsc::RecvTimeoutError::Timeout)) {
                let Some(subscriptions) = periodic.upgrade() else { break };
                reconcile(&subscriptions, Ok(Event::new(EventKind::Any)));
            }
        });
        Self {
            native,
            polling: None,
            directories: BTreeMap::new(),
            subscriptions,
            _reconcile_stop: stop,
        }
    }

    fn watch(&mut self, directory: &Path) -> io::Result<()> {
        if let Some(watch) = self.directories.get_mut(directory) {
            watch.users += 1;
            return Ok(());
        }
        let native = self
            .native
            .as_mut()
            .is_some_and(|watcher| watcher.watch(directory, RecursiveMode::NonRecursive).is_ok());
        if !native {
            if self.polling.is_none() {
                let callback = Arc::clone(&self.subscriptions);
                self.polling = Some(
                    PollWatcher::new(
                        move |event| reconcile(&callback, event),
                        Config::default()
                            .with_poll_interval(Duration::from_millis(500))
                            .with_compare_contents(true),
                    )
                    .map_err(io::Error::other)?,
                );
            }
            self.polling
                .as_mut()
                .unwrap()
                .watch(directory, RecursiveMode::NonRecursive)
                .map_err(io::Error::other)?;
        }
        self.directories.insert(directory.to_owned(), DirectoryWatch { users: 1, native });
        if native {
            // FSEvents restarts its stream when the watched directories change.
            reconcile(&self.subscriptions, Ok(Event::new(EventKind::Any)));
        }
        Ok(())
    }

    fn unwatch(&mut self, directory: &Path) {
        let watch = self.directories.get_mut(directory).unwrap();
        watch.users -= 1;
        if watch.users == 0 {
            let watch = self.directories.remove(directory).unwrap();
            if watch.native {
                let _ = self.native.as_mut().unwrap().unwatch(directory);
                reconcile(&self.subscriptions, Ok(Event::new(EventKind::Any)));
            } else {
                let _ = self.polling.as_mut().unwrap().unwatch(directory);
            }
        }
    }
}

fn service() -> Arc<Mutex<WatchService>> {
    static SERVICE: OnceLock<Mutex<Weak<Mutex<WatchService>>>> = OnceLock::new();
    let mut shared = SERVICE.get_or_init(Mutex::default).lock().unwrap();
    if let Some(service) = shared.upgrade() {
        return service;
    }
    let service = Arc::new(Mutex::new(WatchService::new()));
    *shared = Arc::downgrade(&service);
    service
}

pub struct FileWatcher {
    service: Arc<Mutex<WatchService>>,
    path: PathBuf,
    sender: async_channel::Sender<FileState>,
    events: async_channel::Receiver<FileState>,
}

impl FileWatcher {
    pub fn new(path: PathBuf, initial: FileState) -> io::Result<Self> {
        Self::subscribe(path, initial, service())
    }

    fn subscribe(path: PathBuf, initial: FileState, service: Arc<Mutex<WatchService>>) -> io::Result<Self> {
        let path = resolve(&path).map_err(io::Error::other)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a file path is required"))?;
        let (sender, events) = async_channel::bounded(1);
        {
            let mut service = service.lock().unwrap();
            service.watch(parent)?;
            let mut subscriptions = service.subscriptions.lock().unwrap();
            let signature = Signature::of(&path).ok();
            let current = FileState::read(&path);
            if current != initial {
                let _ = sender.try_send(current.clone());
            }
            let watched = subscriptions.entry(path.clone()).or_default();
            watched.signature = signature;
            for subscriber in &mut watched.subscribers {
                if current != subscriber.previous {
                    subscriber.previous = current.clone();
                    let _ = subscriber.sender.force_send(subscriber.previous.clone());
                }
            }
            watched.subscribers.push(Subscription {
                previous: current,
                sender: sender.clone(),
            });
        }
        Ok(Self {
            service,
            path,
            sender,
            events,
        })
    }

    pub fn events(&self) -> async_channel::Receiver<FileState> {
        self.events.clone()
    }
}

impl Drop for FileWatcher {
    fn drop(&mut self) {
        self.events.close();
        let mut service = self.service.lock().unwrap();
        {
            let mut subscriptions = service.subscriptions.lock().unwrap();
            let watched = subscriptions.get_mut(&self.path).unwrap();
            watched.subscribers.retain(|subscriber| !subscriber.sender.same_channel(&self.sender));
            if watched.subscribers.is_empty() {
                subscriptions.remove(&self.path);
            }
        }
        service.unwatch(self.path.parent().unwrap());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Instant};

    #[test]
    fn periodic_reconciliation_detects_changes_without_native_notifications() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("watched.tex");
        fs::write(&path, "initial").unwrap();
        let mut service = WatchService::new();
        service.native = Some(notify::recommended_watcher(|_| {}).unwrap());
        let watcher = FileWatcher::subscribe(path.clone(), FileState::read(&path), Arc::new(Mutex::new(service))).unwrap();
        assert!(watcher.service.lock().unwrap().directories.values().all(|watch| watch.native));
        let events = watcher.events();
        for expected in [
            FileState::Present(b"replacement".to_vec()),
            FileState::Missing,
            FileState::Present(b"recreated".to_vec()),
        ] {
            match &expected {
                FileState::Present(bytes) => {
                    crate::write_atomic(&path, std::str::from_utf8(bytes).unwrap()).unwrap();
                }
                FileState::Missing => fs::remove_file(&path).unwrap(),
                FileState::Unreadable(_) => unreachable!(),
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Ok(event) = events.try_recv() {
                    assert_eq!(event, expected);
                    break;
                }
                assert!(Instant::now() < deadline, "periodic reconciliation did not report a change");
                thread::sleep(Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn slow_subscribers_receive_the_latest_state_without_queuing_old_contents() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("watched.tex");
        fs::write(&path, "initial").unwrap();
        let watcher = FileWatcher::new(path.clone(), FileState::Missing).unwrap();
        let subscriptions = Arc::clone(&watcher.service.lock().unwrap().subscriptions);
        for text in ["first", "second", "latest"] {
            fs::write(&path, text).unwrap();
            reconcile(&subscriptions, Ok(Event::new(EventKind::Any)));
        }
        let events = watcher.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events.try_recv().unwrap(), FileState::Present(b"latest".to_vec()));
        assert!(events.is_empty());
    }

    #[test]
    fn polling_fallback_reports_replacements_deletion_and_recreation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("watched.tex");
        fs::write(&path, "initial").unwrap();
        let mut service = WatchService::new();
        service.native = None;
        let watcher = FileWatcher::subscribe(path.clone(), FileState::read(&path), Arc::new(Mutex::new(service))).unwrap();
        let events = watcher.events();
        for expected in [
            FileState::Present(b"replacement".to_vec()),
            FileState::Missing,
            FileState::Present(b"recreated".to_vec()),
        ] {
            match &expected {
                FileState::Present(bytes) => {
                    crate::write_atomic(&path, std::str::from_utf8(bytes).unwrap()).unwrap();
                }
                FileState::Missing => fs::remove_file(&path).unwrap(),
                FileState::Unreadable(_) => unreachable!(),
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Ok(event) = events.try_recv() {
                    assert_eq!(event, expected);
                    break;
                }
                assert!(Instant::now() < deadline, "polling watcher did not report a change");
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
