use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const ENGINE_VERSION: &str = "0.17.0";
const TEXLAB_REVISION: &str = "45c3ced6790b508a12a2aab1938e58023173017e";

mod build_storage;

#[path = "src/bundle.rs"]
mod bundle;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!("cargo::rerun-if-changed=build_storage.rs");
    println!("cargo::rerun-if-changed=src/bundle.rs");
    println!("cargo::rerun-if-changed=resources");
    for name in ["PKG_CONFIG_PATH", "CC", "CXX"] {
        println!("cargo::rerun-if-env-changed={name}");
    }
    if let Err(error) = build_engine() {
        panic!("Could not stage bundled tools and TeX resources: {error}");
    }
}

fn build_engine() -> Result<(), String> {
    let target = env::var("TARGET").map_err(|error| error.to_string())?;
    let host = env::var("HOST").map_err(|error| error.to_string())?;
    if !target.ends_with("-apple-darwin") || target != host {
        return Err("Bundled Tectonic currently requires a native macOS build".into());
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("Missing OUT_DIR")?);
    let build_directory = output.ancestors().nth(2).ok_or("Invalid Cargo output directory")?;
    let profile_directory = build_directory.parent().ok_or("Missing Cargo profile directory")?;
    let storage = profile_directory.join("ligature-toolchain");
    fs::create_dir_all(&storage).map_err(|error| error.to_string())?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(storage.join("build.lock"))
        .map_err(|error| error.to_string())?;
    lock.lock().map_err(|error| error.to_string())?;
    let legacy_outputs = build_storage::legacy_outputs(build_directory).map_err(|error| error.to_string())?;
    build_storage::migrate_cache(&legacy_outputs, &storage).map_err(|error| error.to_string())?;
    let install = storage.join("tectonic-install");
    let staging = tempfile::Builder::new()
        .prefix("resources-")
        .tempdir_in(&storage)
        .map_err(|error| error.to_string())?;
    let resources = staging.path().join("resources");
    let libraries = resources.join("lib");
    fs::create_dir_all(&install).map_err(|error| error.to_string())?;
    fs::create_dir_all(resources.join("bin")).map_err(|error| error.to_string())?;
    fs::create_dir_all(&libraries).map_err(|error| error.to_string())?;

    let icu = run(Command::new("brew").args(["--prefix", "icu4c"]))?;
    let icu = PathBuf::from(String::from_utf8(icu.stdout).map_err(|error| error.to_string())?.trim());
    let mut pkg_config_paths = vec![icu.join("lib/pkgconfig")];
    if let Some(paths) = env::var_os("PKG_CONFIG_PATH") {
        pkg_config_paths.extend(env::split_paths(&paths));
    }
    let mut cargo = Command::new(env::var_os("CARGO").ok_or("Missing CARGO")?);
    cargo
        .current_dir(&install)
        .args([
            "install",
            "tectonic",
            "--git",
            "https://github.com/ADJB1212/tectonic.git",
            "--branch",
            "master",
            "--locked",
            "--force",
            "--config",
            "profile.release.lto=\"fat\"",
            "--config",
            "profile.release.codegen-units=1",
            "--config",
            "profile.release.panic=\"abort\"",
            "--target",
        ])
        .arg(&target)
        .arg("--root")
        .arg(&install)
        .arg("--target-dir")
        .arg(storage.join("tectonic-build"))
        .env("TECTONIC_DEP_BACKEND", "pkg-config")
        .env("TECTONIC_PKGCONFIG_FORCE_SEMI_STATIC", "1")
        .env("PKG_CONFIG_PATH", env::join_paths(pkg_config_paths).map_err(|error| error.to_string())?)
        .env("CFLAGS", "-O3 -std=gnu17 -fuse-ld=lld -march=native -w")
        .env("CXXFLAGS", "-O3 -std=c++17 -fuse-ld=lld -march=native -w");

    for name in [
        "CARGO_MAKEFLAGS",
        "MAKEFLAGS",
        "MFLAGS",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WORKSPACE_WRAPPER",
        "CLIPPY_ARGS",
    ] {
        cargo.env_remove(name);
    }
    let tty = fs::OpenOptions::new().write(true).open("/dev/tty").map_err(|error| error.to_string())?;

    let status = cargo
        .stdout(tty.try_clone().map_err(|error| error.to_string())?)
        .stderr(tty)
        .status()
        .map_err(|error| error.to_string())?;

    if !status.success() {
        return Err(format!("cargo install tectonic failed: {status}"));
    }

    let executable = resources.join("bin/tectonic");
    copy_executable(&install.join("bin/tectonic"), &executable)?;
    let mut bundled = BTreeMap::new();
    bundle_libraries(
        &executable,
        &install.join("bin/tectonic"),
        &libraries,
        "@executable_path/../lib",
        &mut bundled,
    )?;
    sign(&executable)?;
    run(Command::new(&executable).arg("--version"))?;
    stage_language_server(&storage, &resources, &target)?;
    stage_resources(&resources)?;
    build_storage::publish_resources(&resources, &storage).map_err(|error| error.to_string())?;
    let resources = storage.join("resources");
    build_storage::prune_legacy_outputs(&legacy_outputs, &resources).map_err(|error| error.to_string())?;

    for source in bundled.values() {
        println!("cargo::rerun-if-changed={}", source.display());
    }
    println!("cargo::rustc-env=LIGATURE_TECTONIC_VERSION={ENGINE_VERSION}");
    println!("cargo::rustc-env=LIGATURE_TECTONIC_RESOURCES={}", resources.display());
    Ok(())
}

fn stage_language_server(storage: &Path, resources: &Path, target: &str) -> Result<(), String> {
    let source = storage.join("texlab-5.26.0-source");
    let install = storage.join("texlab-5.26.0-install");
    if !source.join(".git").exists() {
        fs::create_dir_all(&source).map_err(|error| error.to_string())?;
        run(Command::new("git").arg("init").arg(&source))?;
    }
    if !source.join("Cargo.toml").exists() {
        run(Command::new("git")
            .arg("-C")
            .arg(&source)
            .args(["fetch", "--depth=1", "https://github.com/latex-lsp/texlab.git", TEXLAB_REVISION]))?;
        run(Command::new("git").arg("-C").arg(&source).args(["checkout", "--detach", TEXLAB_REVISION]))?;
    }
    let executable = install.join("bin/texlab");
    if !executable.is_file() {
        let mut cargo = Command::new(env::var_os("CARGO").ok_or("Missing CARGO")?);
        cargo
            .current_dir(&source)
            .args(["install", "--path"])
            .arg(source.join("crates/texlab"))
            .args(["--locked", "--target", target, "--root"])
            .arg(&install)
            .arg("--target-dir")
            .arg(storage.join("texlab-build"))
            .env("CFLAGS", "-fuse-ld=lld -march=native")
            .env("CXXFLAGS", "-fuse-ld=lld -march=native");
        for name in [
            "CARGO_MAKEFLAGS",
            "MAKEFLAGS",
            "MFLAGS",
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "RUSTC_WORKSPACE_WRAPPER",
            "CLIPPY_ARGS",
        ] {
            cargo.env_remove(name);
        }
        run(&mut cargo)?;
    }
    let staged = resources.join("bin/texlab");
    copy_executable(&executable, &staged)?;
    sign(&staged)?;
    run(Command::new(&staged).arg("--version"))?;
    let licenses = resources.join("licenses");
    fs::create_dir_all(&licenses).map_err(|error| error.to_string())?;
    fs::copy(source.join("LICENSE"), licenses.join("Texlab-GPL-3.0.txt")).map_err(|error| error.to_string())?;
    run(Command::new("git")
        .arg("-C")
        .arg(&source)
        .args(["archive", "--format=tar", "--output"])
        .arg(licenses.join("Texlab-source.tar"))
        .arg(TEXLAB_REVISION))?;
    fs::write(licenses.join("Texlab-NOTICE.txt"), format!("Texlab 5.26.0\nCopyright Eric Foerster and Patrick Foerster\nGPL-3.0\nSource: https://github.com/latex-lsp/texlab\nRevision: {TEXLAB_REVISION}\n")).map_err(|error| error.to_string())?;
    Ok(())
}

fn stage_resources(resources: &Path) -> Result<(), String> {
    let source = Path::new(&env::var("CARGO_MANIFEST_DIR").map_err(|error| error.to_string())?).join("resources/ligature-bundle-1.zip");
    let mut bundle = bundle::LocalBundle::open(&source)?;
    let tex = resources.join("tex");
    let fonts = resources.join("newcomputermodern/otf");
    fs::create_dir_all(&tex).map_err(|error| error.to_string())?;
    fs::create_dir_all(&fonts).map_err(|error| error.to_string())?;
    fs::copy(&source, tex.join("ligature-bundle-1.zip")).map_err(|error| error.to_string())?;
    let names: Vec<_> = bundle.names()?.into_iter().filter(|name| name.ends_with(".otf")).collect();
    for name in names {
        if Path::new(&name).file_name().and_then(|name| name.to_str()) != Some(name.as_str()) {
            return Err("Packaged font names must be plain filenames".into());
        }
        fs::write(fonts.join(&name), bundle.read(&name)?.ok_or("Missing packaged font")?).map_err(|error| error.to_string())?;
    }
    fs::write(
        resources.join("newcomputermodern/LICENSE.txt"),
        bundle.read("NewComputerModern-LICENSE.txt")?.ok_or("Missing font license")?,
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn bundle_libraries(
    binary: &Path, source: &Path, libraries: &Path, relative_directory: &str, bundled: &mut BTreeMap<OsString, PathBuf>,
) -> Result<(), String> {
    let output = run(Command::new("otool").arg("-L").arg(binary))?;
    let output = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
    for line in output.lines().skip(1) {
        let dependency = line.trim().split_once(" (compatibility version ").ok_or("Invalid otool output")?.0;
        if dependency.starts_with("/usr/lib/") || dependency.starts_with("/System/Library/") {
            continue;
        }
        if dependency
            .strip_prefix("@loader_path/")
            .is_some_and(|name| Some(Path::new(name)) == binary.file_name().map(Path::new))
        {
            continue;
        }
        let original = if let Some(relative) = dependency.strip_prefix("@loader_path/") {
            source.parent().ok_or("Library has no parent directory")?.join(relative)
        } else if Path::new(dependency).is_absolute() {
            PathBuf::from(dependency)
        } else {
            return Err(format!("Cannot resolve native library '{dependency}'"));
        };
        let name = original.file_name().ok_or("Library has no filename")?.to_os_string();
        let original = fs::canonicalize(&original).map_err(|error| error.to_string())?;
        if let Some(previous) = bundled.get(&name) {
            if previous != &original {
                return Err(format!("Conflicting bundled library filename '{}'", name.to_string_lossy()));
            }
        } else {
            bundled.insert(name.clone(), original.clone());
            let destination = libraries.join(&name);
            copy_executable(&original, &destination)?;
            run(Command::new("install_name_tool")
                .arg("-id")
                .arg(format!("@loader_path/{}", name.to_string_lossy()))
                .arg(&destination))?;
            bundle_libraries(&destination, &original, libraries, "@loader_path", bundled)?;
            sign(&destination)?;
        }
        run(Command::new("install_name_tool")
            .arg("-change")
            .arg(dependency)
            .arg(format!("{relative_directory}/{}", name.to_string_lossy()))
            .arg(binary))?;
    }
    Ok(())
}

fn copy_executable(source: &Path, destination: &Path) -> Result<(), String> {
    fs::copy(source, destination).map_err(|error| error.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o755)).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn sign(binary: &Path) -> Result<(), String> {
    run(Command::new("codesign").args(["--force", "--sign", "-"]).arg(binary)).map(|_| ())
}

fn run(command: &mut Command) -> Result<Output, String> {
    let output = command.output().map_err(|error| format!("{command:?}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{command:?} failed ({}):\n{}{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(output)
}
