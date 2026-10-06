//! The delta report: a local directory against a storage enumeration (`nxr diff`).
//!
//! Facts, not refusals: every scanned or enumerated name lands in exactly one
//! section, and a divergence never aborts the walk.
//! The inputs are [`LocalStatus`] and [`RemoteStatus`] facts, the same probes
//! `up` and `down` see, so no transport type leaks upward.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::model::digest::{sha256_file, Digest};
use crate::model::name::ArtifactName;
use crate::model::state::{bytes_path, LocalStatus, RemoteStatus};

/// The content facts one side holds for a name.
/// `None` means the fact is unknown on this side, never that it differs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Side {
    /// Byte size, when the side reports one.
    pub size: Option<u64>,
    /// The sha256 digest, when the side carries one.
    pub digest: Option<Digest>,
}

/// One name's standing between the local directory and the storage enumeration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delta {
    /// Both sides carry the same digest.
    Same { name: ArtifactName, digest: Digest },
    /// The storage holds (or lists) the name, the local directory does not.
    MissingLocal {
        name: ArtifactName,
        /// The storage-side facts, empty when the enumeration listed a name the server has lost.
        remote: Side,
    },
    /// The local directory holds the name, the storage enumeration does not.
    MissingRemote {
        name: ArtifactName,
        /// The local-side facts.
        local: Side,
    },
    /// Both sides hold the name and the copies are not proven equal.
    /// `size` and `sha` name the dimensions that provably differ.
    /// Both `false` means unverifiable: no digest pair and no size contrast anywhere.
    Diverged {
        name: ArtifactName,
        /// The local-side facts.
        local: Side,
        /// The storage-side facts.
        remote: Side,
        /// The sizes are known on both sides and differ.
        size: bool,
        /// The digests are known on both sides and differ.
        sha: bool,
    },
}

/// The delta of a local scan against the remote enumeration, in name order.
///
/// The name set is the union: a scanned-only name is [`Delta::MissingRemote`],
/// an enumerated-only name is [`Delta::MissingLocal`].
/// An enumerated name the server answers 404 for counts as not held, exactly like an unenumerated one.
/// A markerless local file is hashed on the fly, so an unfinished local copy
/// still compares by digest against a marked remote one.
pub fn compare(
    dir: &Path,
    locals: Vec<(ArtifactName, LocalStatus)>,
    remotes: BTreeMap<ArtifactName, RemoteStatus>,
) -> Vec<Delta> {
    let locals: BTreeMap<ArtifactName, LocalStatus> = locals.into_iter().collect();
    let mut names: BTreeSet<&ArtifactName> = locals.keys().collect();
    names.extend(remotes.keys());
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        let remote = remotes
            .get(name)
            .filter(|s| !matches!(s, RemoteStatus::Absent));
        let entry = match (locals.get(name), remote) {
            // Found nowhere, but the enumeration named it: the phantom stays visible, with empty facts.
            (None, None) => Delta::MissingLocal {
                name: name.clone(),
                remote: Side::default(),
            },
            (None, Some(remote)) => Delta::MissingLocal {
                name: name.clone(),
                remote: remote_side(remote),
            },
            (Some(local), None) => Delta::MissingRemote {
                name: name.clone(),
                local: local_side(dir, name, local),
            },
            (Some(local), Some(remote)) => {
                let local = local_side(dir, name, local);
                let remote = remote_side(remote);
                let same = matches!(
                    (&local.digest, &remote.digest),
                    (Some(a), Some(b)) if a == b
                );
                if same {
                    // The digest is the completion truth: equal digests are equal copies,
                    // whatever the size headers claim.
                    let digest = local.digest.expect("same requires two digests");
                    Delta::Same {
                        name: name.clone(),
                        digest,
                    }
                } else {
                    let size = matches!((local.size, remote.size), (Some(a), Some(b)) if a != b);
                    let sha = matches!(
                        (&local.digest, &remote.digest),
                        (Some(a), Some(b)) if a != b
                    );
                    Delta::Diverged {
                        name: name.clone(),
                        local,
                        remote,
                        size,
                        sha,
                    }
                }
            }
        };
        out.push(entry);
    }
    out
}

/// The local facts of one name: the marker digest, or the bytes hashed on the fly.
fn local_side(dir: &Path, name: &ArtifactName, status: &LocalStatus) -> Side {
    let bytes = bytes_path(dir, name);
    let size = std::fs::metadata(&bytes).ok().map(|m| m.len());
    let digest = match status {
        LocalStatus::Complete(d) => Some(d.clone()),
        LocalStatus::Markerless => sha256_file(&bytes).ok(),
        LocalStatus::Broken(_) | LocalStatus::Absent => None,
    };
    Side { size, digest }
}

/// The storage facts of one name: the sibling digest and the HEAD size.
fn remote_side(status: &RemoteStatus) -> Side {
    match status {
        RemoteStatus::Complete { digest, size } => Side {
            size: *size,
            digest: Some(digest.clone()),
        },
        RemoteStatus::Markerless { size } => Side {
            size: *size,
            digest: None,
        },
        RemoteStatus::Absent | RemoteStatus::Broken(_) => Side::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::state::{local_status, sibling_path};
    use std::fs;

    fn name(s: &str) -> ArtifactName {
        ArtifactName::parse(s).expect("test name parses")
    }

    /// Seed bytes plus an optional marker, then classify the name locally.
    fn local(dir: &Path, file: &str, bytes: &[u8], marker: bool) -> (ArtifactName, LocalStatus) {
        fs::write(dir.join(file), bytes).expect("fixture write");
        if marker {
            let line = format!("{}  {file}\n", Digest::of_bytes(bytes).as_str());
            fs::write(dir.join(format!("{file}.sha256")), line).expect("marker write");
        }
        let n = name(file);
        let st = local_status(dir, &n);
        (n, st)
    }

    fn remote_ok(bytes: &[u8], size: Option<u64>) -> RemoteStatus {
        RemoteStatus::Complete {
            digest: Digest::of_bytes(bytes),
            size,
        }
    }

    #[test]
    fn sections_cover_same_missing_local_missing_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let locals = vec![
            local(dir, "a.zip", b"alpha", true),
            local(dir, "c.zip", b"gamma", true),
        ];
        let mut remotes = BTreeMap::new();
        remotes.insert(name("a.zip"), remote_ok(b"alpha", Some(5)));
        remotes.insert(name("b.zip"), remote_ok(b"beta", Some(4)));

        let out = compare(dir, locals, remotes);
        assert_eq!(
            out,
            vec![
                Delta::Same {
                    name: name("a.zip"),
                    digest: Digest::of_bytes(b"alpha"),
                },
                Delta::MissingLocal {
                    name: name("b.zip"),
                    remote: Side {
                        size: Some(4),
                        digest: Some(Digest::of_bytes(b"beta")),
                    },
                },
                Delta::MissingRemote {
                    name: name("c.zip"),
                    local: Side {
                        size: Some(5),
                        digest: Some(Digest::of_bytes(b"gamma")),
                    },
                },
            ]
        );
    }

    #[test]
    fn markerless_locals_are_hashed_for_the_comparison() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let locals = vec![local(dir, "m.bin", b"same bytes", false)];
        let mut remotes = BTreeMap::new();
        remotes.insert(name("m.bin"), remote_ok(b"same bytes", None));
        assert_eq!(
            compare(dir, locals, remotes),
            vec![Delta::Same {
                name: name("m.bin"),
                digest: Digest::of_bytes(b"same bytes"),
            }]
        );

        // A different content diverges by sha even without a local marker.
        let locals = vec![local(dir, "m.bin", b"other bytes", false)];
        let mut remotes = BTreeMap::new();
        remotes.insert(name("m.bin"), remote_ok(b"same bytes", None));
        let out = compare(dir, locals, remotes);
        match &out[..] {
            [Delta::Diverged { size, sha, .. }] => {
                assert!(*sha, "two digests known and different");
                assert!(!*size, "the remote reports no size here");
            }
            other => panic!("unexpected delta: {other:?}"),
        }
    }

    #[test]
    fn unverifiable_and_size_only_divergences() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // Same size, no digest on either side: unverifiable.
        let locals = vec![local(dir, "u.bin", b"ten bytes", false)];
        let mut remotes = BTreeMap::new();
        remotes.insert(
            name("u.bin"),
            RemoteStatus::Markerless {
                size: Some(9), // "ten bytes".len()
            },
        );
        match &compare(dir, locals, remotes)[..] {
            [Delta::Diverged { size, sha, .. }] => {
                assert!(!*size && !*sha, "no comparable dimension");
            }
            other => panic!("unexpected delta: {other:?}"),
        }

        // Different sizes, no digest on the remote: size-only.
        let locals = vec![local(dir, "u.bin", b"totally different", false)];
        let mut remotes = BTreeMap::new();
        remotes.insert(name("u.bin"), RemoteStatus::Markerless { size: Some(9) });
        match &compare(dir, locals, remotes)[..] {
            [Delta::Diverged { size, sha, .. }] => {
                assert!(*size, "the sizes contrast");
                assert!(!*sha, "the remote carries no digest");
            }
            other => panic!("unexpected delta: {other:?}"),
        }
    }

    #[test]
    fn sha_divergence_with_equal_sizes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let locals = vec![local(dir, "d.zip", b"bbbbbbbbbb", true)];
        let mut remotes = BTreeMap::new();
        remotes.insert(name("d.zip"), remote_ok(b"cccccccccc", Some(10)));
        match &compare(dir, locals, remotes)[..] {
            [Delta::Diverged { size, sha, .. }] => {
                assert!(*sha && !*size, "same size, different digest");
            }
            other => panic!("unexpected delta: {other:?}"),
        }
    }

    #[test]
    fn phantom_enumerated_name_is_missing_local_with_empty_facts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let locals = vec![];
        let mut remotes = BTreeMap::new();
        remotes.insert(name("ghost.bin"), RemoteStatus::Absent);
        assert_eq!(
            compare(dir, locals, remotes),
            vec![Delta::MissingLocal {
                name: name("ghost.bin"),
                remote: Side::default(),
            }]
        );
    }

    #[test]
    fn enumerated_but_absent_remote_counts_as_missing_remote() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let locals = vec![local(dir, "c.zip", b"gamma", true)];
        let mut remotes = BTreeMap::new();
        remotes.insert(name("c.zip"), RemoteStatus::Absent);
        assert_eq!(
            compare(dir, locals, remotes),
            vec![Delta::MissingRemote {
                name: name("c.zip"),
                local: Side {
                    size: Some(5),
                    digest: Some(Digest::of_bytes(b"gamma")),
                },
            }]
        );
    }

    #[test]
    fn broken_local_diverges_without_a_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        // Bytes with a stale marker: a broken local copy.
        let (_, _) = local(dir, "x.zip", b"fresh bytes", true);
        fs::write(dir.join("x.zip"), b"rewritten bytes").expect("rewrite");
        fs::write(sibling_path(dir, &name("x.zip")), "garbage").expect("stale marker");
        let locals = vec![(name("x.zip"), local_status(dir, &name("x.zip")))];
        assert!(matches!(locals[0].1, LocalStatus::Broken(_)));
        let mut remotes = BTreeMap::new();
        remotes.insert(name("x.zip"), remote_ok(b"fresh bytes", Some(11)));
        match &compare(dir, locals, remotes)[..] {
            [Delta::Diverged {
                local, size, sha, ..
            }] => {
                assert!(local.digest.is_none(), "a broken copy carries no digest");
                assert!(local.size.is_some());
                assert!(!*sha, "only one digest is known");
                assert!(*size, "15 rewritten bytes vs 11 remote bytes differ");
            }
            other => panic!("unexpected delta: {other:?}"),
        }
    }
}
