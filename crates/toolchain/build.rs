use std::{
    collections::BTreeMap,
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

const ENGINE_VERSION: &str = "0.17.0";

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    for name in ["PKG_CONFIG_PATH", "CC", "CXX"] {
        println!("cargo::rerun-if-env-changed={name}");
    }
    if let Err(error) = build_engine() {
        panic!("Could not build bundled Tectonic: {error}");
    }
}

fn build_engine() -> Result<(), String> {
    let target = env::var("TARGET").map_err(|error| error.to_string())?;
    let host = env::var("HOST").map_err(|error| error.to_string())?;
    if !target.ends_with("-apple-darwin") || target != host {
        return Err("Bundled Tectonic currently requires a native macOS build".into());
    }
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("Missing OUT_DIR")?);
    let install = output.join("tectonic-install");
    let resources = output.join("resources");
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
            "--version",
            &format!("={ENGINE_VERSION}"),
            "--locked",
            "--force",
            "--target",
        ])
        .arg(&target)
        .arg("--root")
        .arg(&install)
        .arg("--target-dir")
        .arg(output.join("tectonic-build"))
        .arg("--jobs")
        .arg(env::var_os("NUM_JOBS").unwrap_or_else(|| "1".into()))
        .env("TECTONIC_DEP_BACKEND", "pkg-config")
        .env("TECTONIC_PKGCONFIG_FORCE_SEMI_STATIC", "1")
        .env("PKG_CONFIG_PATH", env::join_paths(pkg_config_paths).map_err(|error| error.to_string())?)
        .env("CFLAGS", "-std=gnu17")
        .env("CXXFLAGS", "-std=c++17");
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

    for source in bundled.values() {
        println!("cargo::rerun-if-changed={}", source.display());
    }
    println!("cargo::rustc-env=LIGATURE_TECTONIC_VERSION={ENGINE_VERSION}");
    println!("cargo::rustc-env=LIGATURE_TECTONIC_RESOURCES={}", resources.display());
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
