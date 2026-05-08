use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::foundation::{Error, Result};

pub(crate) fn clean_manifest_files(
    dir: &Path,
    manifest_name: &str,
    planned_files: &BTreeSet<String>,
    logging_target: &'static str,
    safe_relative_path: impl Fn(&str) -> Option<PathBuf>,
) -> Result<()> {
    let mut files = read_manifest(dir, manifest_name, logging_target);
    files.extend(planned_files.iter().cloned());

    for file in files {
        let Some(relative) = safe_relative_path(&file).and_then(safe_manifest_relative_path) else {
            tracing::warn!(
                target: "forge.generated_manifest",
                area = logging_target,
                file = %file,
                "skipping unsafe generated manifest path"
            );
            continue;
        };

        match std::fs::remove_file(dir.join(relative)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::other(error)),
        }
    }

    Ok(())
}

pub(crate) fn write_manifest(
    dir: &Path,
    manifest_name: &str,
    output_files: &BTreeSet<String>,
) -> Result<()> {
    let files: Vec<&str> = output_files.iter().map(String::as_str).collect();
    let content = serde_json::to_string_pretty(&files).map_err(Error::other)?;
    std::fs::write(dir.join(manifest_name), content).map_err(Error::other)
}

pub(crate) fn safe_manifest_path_with_extension(
    file: &str,
    extension: &str,
    allow_subdirectories: bool,
) -> Option<PathBuf> {
    if file.is_empty() || file.contains('\\') || file.chars().any(char::is_control) {
        return None;
    }

    let path = Path::new(file);
    if path.is_absolute() {
        return None;
    }
    if path.extension().and_then(|ext| ext.to_str()) != Some(extension) {
        return None;
    }

    let mut normal_components = 0usize;
    for component in path.components() {
        match component {
            Component::Normal(_) => normal_components += 1,
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => return None,
        }
    }

    if normal_components == 0 || (!allow_subdirectories && normal_components != 1) {
        return None;
    }

    Some(path.to_path_buf())
}

fn read_manifest(
    dir: &Path,
    manifest_name: &str,
    logging_target: &'static str,
) -> BTreeSet<String> {
    let path = dir.join(manifest_name);
    let Ok(content) = std::fs::read_to_string(&path) else {
        return BTreeSet::new();
    };

    match serde_json::from_str::<Vec<String>>(&content) {
        Ok(files) => files.into_iter().collect(),
        Err(error) => {
            tracing::warn!(
                target: "forge.generated_manifest",
                area = logging_target,
                path = %path.display(),
                error = %error,
                "ignoring invalid generated manifest"
            );
            BTreeSet::new()
        }
    }
}

fn safe_manifest_relative_path(path: PathBuf) -> Option<PathBuf> {
    if path.is_absolute() {
        return None;
    }

    for component in path.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => return None,
        }
    }

    Some(path)
}
