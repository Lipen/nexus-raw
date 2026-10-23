//! Directory scanning for plain-mode transfers: files are artifacts.

use std::path::Path;

use crate::error::Error;
use crate::model::name::ArtifactName;

/// Recursively list the artifact names in `dir`.
///
/// Every regular file becomes a candidate name (relative path, `/` separated).
/// Hidden path segments (starting with `.`) and `*.sha256` siblings are skipped: a sibling is the marker of its bytes, never an artifact of its own.
/// A symlink or any other non-regular entry is a misuse error (exit 2), never a silent skip.
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
            .map(|e| {
                // A failed entry is a real read error, never a silent skip: an
                // upload over a half-readable tree must fail, not under-report.
                let e = e.map_err(|e| Error::io(&d, e))?;
                Ok::<_, Error>((e.file_name(), e.path()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        items.sort();
        for (fname, path) in items {
            let fname = fname.to_string_lossy().into_owned();
            if fname.starts_with('.') {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path).map_err(|e| Error::io(&path, e))?;
            // The relative name is computed before the type checks: a refused
            // entry must carry the name the user can see in the tree.
            let rel = path
                .strip_prefix(dir)
                .map_err(|_| Error::misuse(format!("scan root mismatch: {}", path.display())))?
                .to_string_lossy()
                .into_owned();
            // A backslash separates path components only on Windows. On Unix
            // it is a legal filename byte, and rewriting it would silently
            // turn `a\b.zip` into the remote name `a/b.zip`. The grammar then
            // rejects either the phantom or the collision.
            #[cfg(windows)]
            let rel = rel.replace('\\', "/");
            if meta.file_type().is_symlink() {
                // A marker is local state, never an artifact: the marker
                // writer's NOFOLLOW guard refuses it before any transfer.
                if fname.ends_with(".sha256") {
                    continue;
                }
                return Err(Error::UnsafeName {
                    name: rel,
                    reason: "symlinked entry: upload the target, not the link".to_owned(),
                });
            }
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            if !meta.is_file() {
                return Err(Error::UnsafeName {
                    name: rel,
                    reason: "non-regular entry: only files and directories are artifacts"
                        .to_owned(),
                });
            }
            if fname.ends_with(".sha256") {
                continue;
            }
            // A `.part` file is local download staging, not an artifact. The
            // name grammar rejects it, so a stale or foreign part fails loudly
            // here instead of vanishing from the upload.
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
    #[cfg(unix)]
    fn backslash_is_not_a_separator_on_unix() {
        // `a\b.zip` is one legal filename on unix. Rewriting the backslash
        // would upload it as the two-segment name `a/b.zip` and could collide
        // with a real `a/b.zip`. The grammar rejects the literal name instead.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("a\\b.zip"), b"x").unwrap();
        let err = scan_dir(root).unwrap_err();
        assert!(
            matches!(err, Error::UnsafeName { .. }),
            "expected the grammar to refuse the literal backslash: {err:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn scan_rejects_a_symlinked_entry() {
        // A `latest -> 1.4.0` link inside the tree used to vanish from the
        // upload with no warning. The scan now carries the name and refuses.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir(root.join("1.4.0")).unwrap();
        std::os::unix::fs::symlink(root.join("1.4.0"), root.join("latest")).unwrap();
        let err = scan_dir(root).unwrap_err();
        assert!(
            matches!(err, Error::UnsafeName { .. }),
            "a symlink must be refused, not silently dropped: {err:?}"
        );
    }

    #[test]
    fn scan_rejects_a_part_staging_file() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::write(root.join("data.part"), b"x").unwrap();
        let err = scan_dir(root).unwrap_err();
        assert!(
            matches!(err, Error::UnsafeName { .. }),
            "a part staging file must be refused, not silently dropped: {err:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn scan_rejects_unsafe_names() {
        // The fixture needs a name windows cannot create at all (`*` is
        // reserved there): the scan's own grammar is exercised on unix, and
        // on windows the filesystem is the first line of the same defense.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        fs::create_dir_all(root.join("bad*name")).unwrap();
        fs::write(root.join("bad*name").join("x"), b"y").unwrap();
        assert!(scan_dir(root).is_err());
    }
}
