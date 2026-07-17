use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const ALLOWED_EXTENSIONS: &[&str] = &["json", "md", "toml", "txt", "yaml", "yml"];

fn main() -> io::Result<()> {
    let manifest_dir =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "CARGO_MANIFEST_DIR is not set")
        })?);
    let source = manifest_dir.join("../../knowledge");
    let destination = PathBuf::from(
        std::env::var_os("OUT_DIR")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "OUT_DIR is not set"))?,
    )
    .join("embedded-knowledge");

    println!("cargo:rerun-if-changed={}", source.display());
    if destination.exists() {
        fs::remove_dir_all(&destination)?;
    }
    fs::create_dir_all(&destination)?;
    copy_curated_tree(&source, &destination)
}

fn copy_curated_tree(source: &Path, destination: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }

        let file_type = entry.file_type()?;
        let target = destination.join(&name);
        if file_type.is_dir() {
            fs::create_dir_all(&target)?;
            copy_curated_tree(&entry.path(), &target)?;
        } else if file_type.is_file() && is_curated_source(&entry.path()) {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn is_curated_source(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            ALLOWED_EXTENSIONS
                .iter()
                .any(|allowed| extension.eq_ignore_ascii_case(allowed))
        })
}
