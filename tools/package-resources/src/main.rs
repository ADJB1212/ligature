use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Write, path::Path, process::Command};
use zip::{CompressionMethod, ZipWriter, write::FileOptions};

const UPSTREAM: &str = "https://data1b.fullyjustified.net/tlextras-2022.0r0.tar";
const UPSTREAM_DIGEST: &str = "6ffe055852f8faf66c0acbe1a7fb27f87b869a90bad1204f3bf4d9683f597c7c";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = std::env::args_os().nth(1).ok_or("Pass the staged Tectonic 0.17.0 executable")?;
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize()?;
    let staging = root.join("target/resource-packaging");
    let cache = staging.join("cache");
    fs::create_dir_all(&staging)?;
    fs::write(
        staging.join("references.bib"),
        "@book{knuth,author={Donald Knuth},title={The TeXbook},year={1984},publisher={Addison-Wesley}}\n",
    )?;
    for class in ["article", "book", "report", "letter"] {
        let input = staging.join(format!("{class}.tex"));
        let document = if class == "letter" {
            "\\documentclass{letter}\n\\begin{document}\\begin{letter}{Reader}\\opening{Hello}Letter.\\closing{Regards}\\end{letter}\\end{document}\n"
                .to_string()
        } else {
            format!("\\documentclass{{{class}}}\n{}", include_str!("../smoke.tex"))
        };
        fs::write(&input, document)?;
        let status = Command::new(&engine)
            .current_dir(&staging)
            .env("TECTONIC_CACHE_DIR", &cache)
            .args(["--untrusted", "--bundle", UPSTREAM, "--keep-logs"])
            .arg(&input)
            .status()?;
        if !status.success() {
            return Err(format!("Resource collection failed for {class}").into());
        }
    }
    let mut files = BTreeMap::new();
    let data = cache.join("bundles/data");
    let directories: Vec<_> = fs::read_dir(&data)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .collect();
    if directories.len() != 1 || directories[0].file_name() != UPSTREAM_DIGEST {
        return Err("Expected exactly one upstream bundle in the isolated collection cache".into());
    }
    collect(&directories[0].path(), &mut files)?;
    collect(&root.join("newcomputermodern/otf"), &mut files)?;
    files.insert(
        "NewComputerModern-LICENSE.txt".into(),
        fs::read(root.join("OS-LICENSES/NewComputerModern_License.txt"))?,
    );
    files.remove("SHA256SUM");
    let mut manifest = String::new();
    for (name, bytes) in &files {
        manifest.push_str(&format!("{}  {name}\n", hex::encode(Sha256::digest(bytes))));
    }
    let digest = hex::encode(Sha256::digest(manifest.as_bytes()));
    files.insert("SHA256SUM".into(), digest.into_bytes());
    files.insert("ligature-files.sha256".into(), manifest.into_bytes());
    let resources = root.join("crates/toolchain/resources");
    fs::create_dir_all(&resources)?;
    let archive = resources.join("ligature-bundle-1.zip");
    let mut zip = ZipWriter::new(fs::File::create(&archive)?);
    let options = FileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (name, bytes) in &files {
        zip.start_file(name, options)?;
        zip.write_all(bytes)?;
    }
    zip.finish()?;
    fs::write(
        resources.join("ligature-bundle-1.sha256"),
        format!("{}\n", hex::encode(Sha256::digest(fs::read(&archive)?))),
    )?;
    fs::write(
        resources.join("ligature-bundle-1.fonts"),
        names_text(files.keys().filter(|name| name.ends_with(".otf"))),
    )?;
    fs::write(resources.join("ligature-bundle-1.files"), names_text(files.keys()))?;
    println!("Packaged {} entries in {}", files.len(), archive.display());
    Ok(())
}

fn names_text<'a>(names: impl Iterator<Item = &'a String>) -> String {
    let mut text = String::new();
    for name in names {
        text.push_str(name);
        text.push('\n');
    }
    text
}

fn collect(directory: &Path, files: &mut BTreeMap<String, Vec<u8>>) -> std::io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            return Err(std::io::Error::other("Bundle resources must be flat regular files"));
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| std::io::Error::other("Non-UTF-8 resource name"))?;
        files.insert(name, fs::read(entry.path())?);
    }
    Ok(())
}
