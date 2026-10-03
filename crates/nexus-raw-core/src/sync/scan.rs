//! Directory scanning for plain-mode transfers: files are artifacts.

use std::path::Path;

use crate::error::Error;
use crate::model::name::ArtifactName;

/// Recursively list the artifact names in `dir`.
///
/// Every regular file becomes a candidate name (relative path, `/` separated).
/// Hidden path segments (starting with `.`) and `*.sha256` siblings are skipped: a sibling is the marker of its bytes, never an artifact of its own.
/// Names that fail the grammar are a misuse error (exit 2), never silently skipped.
///
/// # Errors
///
/// Returns [`Error::Io`] when a directory cannot be listed and [`Error::Misuse`] (or [`Error::UnsafeName`]) when a file path violates the name grammar.
pub fn scan_dir(dir: &Path) -> Result<Vec<ArtifactName>, Error> {
    let mut names = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        let entries = std::fs::read_dir(&d).map_err(|e| Error::io(&d, e))?;
        let mut items: Vec<_> = entries
            .filter_map(std::result::Result::ok)
            .map(|e| (e.file_name(), e.path()))
            .collect();
        items.sort();
        for (fname, path) in items {
            let fname = fname.to_string_lossy().into_owned();
            if fname.starts_with('.') {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path).map_err(|e| Error::io(&path, e))?;
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            if fname.ends_with(".sha256") {
                continue;
            }
            let rel = path
                .strip_prefix(dir)
                .map_err(|_| Error::misuse(format!("scan root mismatch: {}", path.display())))?
                .to_string_lossy()
                .replace('\\', "/");
            names.push(ArtifactName::parse(&rel)?);
        }
    }
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn scan_skips_hidden_and_siblings() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.zip"), b"x").unwrap();
        fs::write(root.join("a.zip.sha256"), b"x").unwrap();
        fs::write(root.join("sub").join("b.json"), b"y").unwrap();
        fs::write(root.join(".hidden"), b"z").unwrap();
        let names = scan_dir(root).unwrap();
        let pretty: Vec<_> = names.iter().map(std::string::ToString::to_string).collect();
        assert_eq!(pretty, ["a.zip", "sub/b.json"]);
    }

    #[test]
    fn scan_rejects_unsafe_names() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("bad*name")).unwrap();
        fs::write(root.join("bad*name").join("x"), b"y").unwrap();
        assert!(scan_dir(root).is_err());
    }
}
