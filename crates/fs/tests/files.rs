use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};

use ligature_fs::{FileState, FileWatcher, read, write_atomic, write_atomic_bytes, write_atomic_checked};

#[test]
fn atomic_binary_writes_preserve_bytes_and_replace_existing_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("export.pdf");
    let bytes = b"%PDF-1.7\n\0\xff\xfe\n%%EOF\n";
    write_atomic_bytes(&path, bytes).unwrap();
    assert_eq!(fs::read(&path).unwrap(), bytes);
    write_atomic_bytes(&path, b"replacement\0\x80").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"replacement\0\x80");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn utf8_lf_crlf_and_bom_round_trip_without_conversion() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document with spaces.tex");
    for text in ["a\nb\n", "a\r\nb\r\n", "", "猫😀\r\n", "\u{feff}a\n", "mixed\r\nlines\n"] {
        write_atomic(&path, text).unwrap();
        let loaded = read(&path).unwrap();
        assert_eq!(loaded.text, text);
        assert_eq!(loaded.path, fs::canonicalize(&path).unwrap());
        assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn invalid_encoding_and_read_failures_include_path_and_reason() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("invalid.tex");
    fs::write(&path, [0xff, 0xfe, 0x61, 0]).unwrap();
    let error = read(&path).err().unwrap().to_string();
    assert!(error.contains("unsupported encoding"));
    assert!(error.contains("invalid.tex"));
    assert!(read(&directory.path().join("missing.tex")).is_err());
    assert!(read(directory.path()).is_err());
}

#[test]
fn failed_saves_preserve_destinations_and_remove_temporary_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("original.tex");
    fs::write(&path, "original").unwrap();
    let expected = FileState::Present(b"older".to_vec());
    assert!(write_atomic_checked(&path, "replacement", Some(&expected)).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    assert!(write_atomic(directory.path(), "replacement").is_err());
    assert!(write_atomic(&directory.path().join("missing/child.tex"), "replacement").is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
}

#[test]
fn read_only_save_is_reported_without_changing_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("readonly.tex");
    fs::write(&path, "original").unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    assert!(write_atomic(&path, "replacement").unwrap_err().to_string().contains("read-only"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "original");
}

#[cfg(unix)]
#[test]
fn atomic_save_preserves_permissions_and_symbolic_links() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("original.tex");
    let alias = directory.path().join("alias.tex");
    fs::write(&path, "original").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&path, &alias).unwrap();
    write_atomic(&alias, "replacement").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "replacement");
    assert!(fs::symlink_metadata(&alias).unwrap().file_type().is_symlink());
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
}

#[cfg(unix)]
#[test]
fn saving_through_a_dangling_symbolic_link_preserves_the_link() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("missing.tex");
    let alias = directory.path().join("alias.tex");
    symlink(&target, &alias).unwrap();
    let error = write_atomic(&alias, "local").unwrap_err().to_string();
    assert!(error.contains("symbolic link"));
    assert_eq!(fs::read_link(&alias).unwrap(), target);
    assert!(!target.exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn watching_a_filesystem_root_returns_an_error() {
    assert!(FileWatcher::new(Path::new("/").to_owned(), FileState::Missing).is_err());
}

#[track_caller]
fn wait_for_event(events: &async_channel::Receiver<FileState>) -> FileState {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(event) = events.try_recv() {
            return event;
        }
        assert!(Instant::now() < deadline, "file watcher did not report a change");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn watcher_reports_atomic_replacements_deletion_and_recreation_and_closes_on_drop() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("watched.tex");
    fs::write(&path, "initial").unwrap();
    let watcher = FileWatcher::new(path.clone(), FileState::read(&path)).unwrap();
    let events = watcher.events();
    write_atomic(&path, "external").unwrap();
    assert_eq!(wait_for_event(&events), FileState::Present(b"external".to_vec()));
    fs::rename(&path, directory.path().join("moved.tex")).unwrap();
    assert_eq!(wait_for_event(&events), FileState::Missing);
    write_atomic(&path, "recreated").unwrap();
    assert_eq!(wait_for_event(&events), FileState::Present(b"recreated".to_vec()));
    drop(watcher);
    assert!(events.is_closed());
}

#[test]
fn checked_save_can_recreate_a_missing_document() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("new.tex");
    write_atomic_checked(&path, "restored", Some(&FileState::Missing)).unwrap();
    assert_eq!(read(&path).unwrap().text, "restored");
    assert!(matches!(
        FileState::read(Path::new("/definitely-missing-ligature-document")),
        FileState::Missing
    ));
}

#[test]
fn watchers_share_a_directory_and_duplicate_subscriptions_survive_drop() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.tex");
    let second = directory.path().join("second.tex");
    fs::write(&first, "initial").unwrap();
    fs::write(&second, "initial").unwrap();
    let watcher = FileWatcher::new(first.clone(), FileState::read(&first)).unwrap();
    let duplicate = FileWatcher::new(first.clone(), FileState::read(&first)).unwrap();
    let sibling = FileWatcher::new(second.clone(), FileState::read(&second)).unwrap();
    write_atomic(&first, "updated").unwrap();
    assert_eq!(wait_for_event(&watcher.events()), FileState::Present(b"updated".to_vec()));
    assert_eq!(wait_for_event(&duplicate.events()), FileState::Present(b"updated".to_vec()));
    assert!(sibling.events().is_empty());
    drop(watcher);
    write_atomic(&first, "again").unwrap();
    assert_eq!(wait_for_event(&duplicate.events()), FileState::Present(b"again".to_vec()));
    drop(duplicate);
    write_atomic(&second, "sibling").unwrap();
    assert_eq!(wait_for_event(&sibling.events()), FileState::Present(b"sibling".to_vec()));
}

#[test]
fn watcher_reconciles_changes_since_the_initial_snapshot_and_watches_missing_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("watched.tex");
    let initial = FileState::Present(b"initial".to_vec());
    fs::write(&path, "updated").unwrap();
    let watcher = FileWatcher::new(path.clone(), initial).unwrap();
    assert_eq!(wait_for_event(&watcher.events()), FileState::Present(b"updated".to_vec()));
    drop(watcher);
    fs::remove_file(&path).unwrap();
    let watcher = FileWatcher::new(path.clone(), FileState::Missing).unwrap();
    write_atomic(&path, "created").unwrap();
    assert_eq!(wait_for_event(&watcher.events()), FileState::Present(b"created".to_vec()));
}

#[test]
fn checked_save_rejects_a_file_created_since_the_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("new.tex");
    fs::write(&path, "external").unwrap();
    assert!(write_atomic_checked(&path, "local", Some(&FileState::Missing)).is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "external");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn checked_save_rejects_a_file_deleted_since_the_snapshot() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("deleted.tex");
    let expected = FileState::Present(b"original".to_vec());
    assert!(write_atomic_checked(&path, "local", Some(&expected)).is_err());
    assert!(!path.exists());
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
}

#[test]
fn checked_save_with_matching_contents_replaces_the_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("document.tex");
    fs::write(&path, "猫\r\n").unwrap();
    let expected = FileState::read(&path);
    let resolved = write_atomic_checked(&path, "updated\n", Some(&expected)).unwrap();
    assert_eq!(resolved, fs::canonicalize(&path).unwrap());
    assert_eq!(fs::read_to_string(&path).unwrap(), "updated\n");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn directory_listing_sorts_directories_before_files() {
    let directory = tempfile::tempdir().unwrap();
    for name in ["z-folder", "b-folder"] {
        fs::create_dir(directory.path().join(name)).unwrap();
    }
    for name in ["c.tex", "a.tex"] {
        fs::write(directory.path().join(name), "").unwrap();
    }
    let entries = ligature_fs::read_directory(directory.path()).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| (entry.path.file_name().unwrap().to_str().unwrap(), entry.is_directory))
            .collect::<Vec<_>>(),
        vec![("b-folder", true), ("z-folder", true), ("a.tex", false), ("c.tex", false)]
    );
}
