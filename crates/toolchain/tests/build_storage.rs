#[path = "../build_storage.rs"]
mod build_storage;

use std::fs;

#[test]
fn migrating_and_pruning_keeps_one_engine_and_preserves_legacy_resource_paths() {
    let directory = tempfile::tempdir().unwrap();
    let build = directory.path().join("build");
    let storage = directory.path().join("ligature-toolchain");
    fs::create_dir_all(&storage).unwrap();
    for hash in ["abc", "def"] {
        let output = build.join(format!("ligature-toolchain-{hash}/out"));
        for name in ["tectonic-install/bin", "tectonic-build", "resources/bin"] {
            fs::create_dir_all(output.join(name)).unwrap();
        }
        fs::write(output.join("tectonic-install/bin/tectonic"), hash).unwrap();
        let modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(if hash == "abc" { 1 } else { 2 });
        fs::File::options()
            .write(true)
            .open(output.join("tectonic-install/bin/tectonic"))
            .unwrap()
            .set_modified(modified)
            .unwrap();
        fs::write(output.join("resources/bin/tectonic"), "old").unwrap();
        fs::write(output.join("unrelated"), "keep").unwrap();
    }
    let unrelated = build.join("another-crate-abc/out/resources");
    fs::create_dir_all(&unrelated).unwrap();
    fs::write(unrelated.join("data"), "keep").unwrap();
    let outputs = build_storage::legacy_outputs(&build).unwrap();
    assert_eq!(outputs.len(), 2);
    build_storage::migrate_cache(&outputs, &storage).unwrap();
    assert_eq!(fs::read(storage.join("tectonic-install/bin/tectonic")).unwrap(), b"def");
    assert!(storage.join("tectonic-build").is_dir());
    let staged = directory.path().join("staged");
    fs::create_dir_all(staged.join("bin")).unwrap();
    fs::write(staged.join("bin/tectonic"), "new").unwrap();
    build_storage::publish_resources(&staged, &storage).unwrap();
    build_storage::prune_legacy_outputs(&outputs, &storage.join("resources")).unwrap();
    build_storage::prune_legacy_outputs(&outputs, &storage.join("resources")).unwrap();
    for output in outputs {
        assert!(!output.join("tectonic-install").exists());
        assert!(!output.join("tectonic-build").exists());
        assert_eq!(fs::read(output.join("resources/bin/tectonic")).unwrap(), b"new");
        assert_eq!(fs::read(output.join("unrelated")).unwrap(), b"keep");
    }
    assert_eq!(fs::read(unrelated.join("data")).unwrap(), b"keep");
}

#[test]
fn failed_resource_publication_preserves_the_previous_engine() {
    let directory = tempfile::tempdir().unwrap();
    let resources = directory.path().join("resources");
    fs::create_dir_all(&resources).unwrap();
    fs::write(resources.join("tectonic"), "previous").unwrap();
    assert!(build_storage::publish_resources(&directory.path().join("missing"), directory.path()).is_err());
    assert_eq!(fs::read(resources.join("tectonic")).unwrap(), b"previous");
}
