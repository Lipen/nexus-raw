//! The read-only FUSE filesystem over a Nexus raw repository.
//!
//! The repository has no directories: a path is a directory while any object sits under it.
//! A node is therefore a file when HEAD answers 200 and a directory when the search listing is
//! non-empty. An absent object with an empty listing is `ENOENT`.
//!
//! Reads are block-cached, listings and attributes live for [`TTL`], and no lock is held across a
//! network call: the cache mutex only guards map lookups and inserts.

use fuser::{
    AccessFlags, Errno, FileAttr, FileHandle, FileType, Filesystem, FopenFlags, Generation,
    INodeNo, LockOwner, OpenFlags, ReplyAttr, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, Request,
};
use nexus_raw_core::{EntryKind, Error, Nxr};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime};
use tokio::runtime::Handle;

/// How long the kernel may cache an attribute or a listing.
const TTL: Duration = Duration::from_secs(2);

/// The root inode: FUSE reserves it.
const ROOT: u64 = 1;

/// One known node.
struct Node {
    path: String,
}

/// A cached directory listing.
struct Listing {
    at: Instant,
    entries: Vec<(u64, FileType, String)>,
}

/// Cached attributes of one node.
struct Attrs {
    at: Instant,
    attr: FileAttr,
}

struct State {
    nodes: HashMap<u64, Node>,
    paths: HashMap<String, u64>,
    next_ino: u64,
    blocks: HashMap<(u64, u64), Arc<Vec<u8>>>,
    block_bytes: usize,
    listings: HashMap<u64, Listing>,
    attrs: HashMap<u64, Attrs>,
    lookups: HashMap<u64, u64>,
}

impl State {
    /// Drop every cached block of one inode and keep `block_bytes` exact.
    fn evict_blocks(&mut self, ino: u64) {
        let mut dropped = 0usize;
        self.blocks.retain(|(key, _), value| {
            if *key == ino {
                dropped += value.len();
                false
            } else {
                true
            }
        });
        self.block_bytes = self.block_bytes.saturating_sub(dropped);
    }

    /// Drop every cache of one inode and its node after the kernel's last lookup.
    fn forget(&mut self, ino: u64, nlookup: u64) {
        if ino == ROOT {
            return;
        }
        let Some(count) = self.lookups.get_mut(&ino) else {
            return;
        };
        *count = count.saturating_sub(nlookup);
        if *count > 0 {
            return;
        }
        self.lookups.remove(&ino);
        self.attrs.remove(&ino);
        self.listings.remove(&ino);
        self.evict_blocks(ino);
        if let Some(node) = self.nodes.remove(&ino) {
            self.paths.remove(&node.path);
        }
    }
}

/// A read-only view of one repository, mounted as a local directory.
pub struct NexusFs {
    nxr: Arc<Nxr>,
    rt: Handle,
    /// The repository root URL, without a trailing slash.
    base: String,
    /// One read block in bytes.
    block: u64,
    /// The read-block cache budget in bytes.
    cache_cap: usize,
    uid: u32,
    gid: u32,
    state: Mutex<State>,
}

/// What a path resolves to.
enum Resolved {
    Dir,
    File(u64),
}

impl NexusFs {
    pub fn new(nxr: Arc<Nxr>, rt: Handle, base: String, block: u64, cache_cap: usize) -> Self {
        let mut nodes = HashMap::new();
        let mut paths = HashMap::new();
        nodes.insert(
            ROOT,
            Node {
                path: "/".to_owned(),
            },
        );
        paths.insert("/".to_owned(), ROOT);
        // SAFETY: `getuid`/`getgid` have no preconditions and cannot fail.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        Self {
            nxr,
            rt,
            base: base.trim_end_matches('/').to_owned(),
            block,
            cache_cap,
            uid,
            gid,
            state: Mutex::new(State {
                nodes,
                paths,
                next_ino: ROOT + 1,
                blocks: HashMap::new(),
                block_bytes: 0,
                listings: HashMap::new(),
                attrs: HashMap::new(),
                lookups: HashMap::new(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The repository URL of one path.
    fn url_for(&self, path: &str) -> String {
        repo_url(&self.base, path)
    }

    fn path_of(&self, ino: u64) -> Option<String> {
        self.state().nodes.get(&ino).map(|node| node.path.clone())
    }

    /// The inode of a path when the kernel has already looked it up.
    fn known_ino(&self, path: &str) -> Option<u64> {
        self.state().paths.get(path).copied()
    }

    /// The inode of a path, allocated on first sight.
    fn ino_for_path(&self, path: &str) -> u64 {
        let mut state = self.state();
        if let Some(&ino) = state.paths.get(path) {
            return ino;
        }
        let ino = state.next_ino;
        state.next_ino += 1;
        state.paths.insert(path.to_owned(), ino);
        state.nodes.insert(
            ino,
            Node {
                path: path.to_owned(),
            },
        );
        ino
    }

    /// File or directory: HEAD first, the search listing second.
    fn resolve(&self, path: &str) -> Result<Resolved, Errno> {
        if path == "/" {
            return Ok(Resolved::Dir);
        }
        let url = self.url_for(path);
        let head = self.rt.block_on(self.nxr.head(&url)).map_err(map_error)?;
        if (200..300).contains(&head.status) {
            return Ok(Resolved::File(head.size.unwrap_or(0)));
        }
        if head.status != 404 {
            return Err(Errno::EIO);
        }
        let entries = self.rt.block_on(self.nxr.ls_at(&url)).map_err(map_error)?;
        if entries.is_empty() {
            Err(Errno::ENOENT)
        } else {
            Ok(Resolved::Dir)
        }
    }

    /// The attributes of one path, cached for [`TTL`].
    fn attr_for_path(&self, path: &str) -> Result<FileAttr, Errno> {
        let ino = self.ino_for_path(path);
        if let Some(attr) = self.cached_attr(ino) {
            return Ok(attr);
        }
        let (kind, size) = match self.resolve(path)? {
            Resolved::Dir => (FileType::Directory, 0),
            Resolved::File(size) => (FileType::RegularFile, size),
        };
        let attr = self.make_attr(ino, kind, size);
        self.state().attrs.insert(
            ino,
            Attrs {
                at: Instant::now(),
                attr,
            },
        );
        Ok(attr)
    }

    fn cached_attr(&self, ino: u64) -> Option<FileAttr> {
        let state = self.state();
        let cached = state.attrs.get(&ino)?;
        if cached.at.elapsed() < TTL {
            Some(cached.attr)
        } else {
            None
        }
    }

    fn make_attr(&self, ino: u64, kind: FileType, size: u64) -> FileAttr {
        let now = SystemTime::now();
        let (perm, nlink) = match kind {
            FileType::Directory => (0o555, 2),
            _ => (0o444, 1),
        };
        FileAttr {
            ino: INodeNo(ino),
            size,
            blocks: size.div_ceil(512),
            atime: now,
            mtime: now,
            ctime: now,
            crtime: now,
            kind,
            perm,
            nlink,
            uid: self.uid,
            gid: self.gid,
            rdev: 0,
            blksize: 4096,
            flags: 0,
        }
    }

    /// The immediate children of one directory, cached for [`TTL`].
    fn list_dir(&self, ino: u64, path: &str) -> Result<Vec<(u64, FileType, String)>, Errno> {
        {
            let state = self.state();
            if let Some(listing) = state.listings.get(&ino) {
                if listing.at.elapsed() < TTL {
                    return Ok(listing.entries.clone());
                }
            }
        }
        let entries = self
            .rt
            .block_on(self.nxr.ls_at(&self.url_for(path)))
            .map_err(map_error)?;
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            let child = join(path, &entry.name);
            let kind = match entry.kind {
                EntryKind::Dir => FileType::Directory,
                EntryKind::File => FileType::RegularFile,
            };
            let child_ino = self
                .known_ino(&child)
                .unwrap_or_else(|| synthetic_ino(&child));
            out.push((child_ino, kind, entry.name));
        }
        self.state().listings.insert(
            ino,
            Listing {
                at: Instant::now(),
                entries: out.clone(),
            },
        );
        Ok(out)
    }

    /// One read block, fetched on demand and cached.
    ///
    /// The whole map is dropped when the budget is exceeded: one block is a bounded overshoot.
    fn block(&self, ino: u64, url: &str, index: u64) -> Result<Arc<Vec<u8>>, Errno> {
        if let Some(bytes) = self.state().blocks.get(&(ino, index)).cloned() {
            return Ok(bytes);
        }
        let start = index * self.block;
        let bytes = self
            .rt
            .block_on(self.nxr.read_range(url, start, self.block))
            .map_err(map_error)?;
        let Some(bytes) = bytes else {
            return Err(Errno::ENOENT);
        };
        let bytes = Arc::new(bytes);
        let mut state = self.state();
        state.block_bytes += bytes.len();
        if state.block_bytes > self.cache_cap {
            state.evict_blocks(ino);
        }
        state.blocks.insert((ino, index), Arc::clone(&bytes));
        drop(state);
        Ok(bytes)
    }
}

impl Filesystem for NexusFs {
    fn lookup(&self, _req: &Request, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let Some(parent_path) = self.path_of(parent.0) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let Some(name) = name.to_str() else {
            reply.error(Errno::ENOENT);
            return;
        };
        let path = match name {
            "." => parent_path,
            ".." => parent_of(&parent_path).to_owned(),
            _ if name.is_empty() || name.contains('/') => {
                reply.error(Errno::ENOENT);
                return;
            }
            _ => join(&parent_path, name),
        };
        match self.attr_for_path(&path) {
            Ok(attr) => {
                self.state()
                    .lookups
                    .entry(attr.ino.0)
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
                reply.entry(&TTL, &attr, Generation(0));
            }
            Err(err) => reply.error(err),
        }
    }

    fn forget(&self, _req: &Request, ino: INodeNo, nlookup: u64) {
        self.state().forget(ino.0, nlookup);
    }

    fn getattr(&self, _req: &Request, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let Some(path) = self.path_of(ino.0) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.attr_for_path(&path) {
            Ok(attr) => reply.attr(&TTL, &attr),
            Err(err) => reply.error(err),
        }
    }

    fn open(&self, _req: &Request, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        if self.path_of(ino.0).is_none() {
            reply.error(Errno::ENOENT);
            return;
        }
        reply.opened(FileHandle(ino.0), FopenFlags::empty());
    }

    fn read(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let Some(path) = self.path_of(ino.0) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let attr = match self.attr_for_path(&path) {
            Ok(attr) => attr,
            Err(err) => {
                reply.error(err);
                return;
            }
        };
        if attr.kind == FileType::Directory {
            reply.error(Errno::EISDIR);
            return;
        }
        if offset >= attr.size {
            reply.data(&[]);
            return;
        }
        let end = (offset + u64::from(size)).min(attr.size);
        let url = self.url_for(&path);
        let mut out = Vec::with_capacity((end - offset) as usize);
        let first = offset / self.block;
        let last = (end - 1) / self.block;
        for index in first..=last {
            let bytes = match self.block(ino.0, &url, index) {
                Ok(bytes) => bytes,
                Err(err) => {
                    reply.error(err);
                    return;
                }
            };
            let block_start = index * self.block;
            let from = offset.max(block_start);
            let to = end.min(block_start + bytes.len() as u64);
            if to > from {
                let lo = (from - block_start) as usize;
                let hi = (to - block_start) as usize;
                out.extend_from_slice(&bytes[lo..hi]);
            }
        }
        reply.data(&out);
    }

    fn release(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        {
            let mut state = self.state();
            state.evict_blocks(ino.0);
        }
        reply.ok();
    }

    fn readdir(
        &self,
        _req: &Request,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let Some(path) = self.path_of(ino.0) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let entries = match self.list_dir(ino.0, &path) {
            Ok(entries) => entries,
            Err(err) => {
                reply.error(err);
                return;
            }
        };
        let parent_ino = self.ino_for_path(parent_of(&path));
        let mut items = Vec::with_capacity(entries.len() + 2);
        items.push((ino.0, FileType::Directory, ".".to_owned()));
        items.push((parent_ino, FileType::Directory, "..".to_owned()));
        items.extend(entries);
        for (index, (child_ino, kind, name)) in items.into_iter().enumerate().skip(offset as usize)
        {
            if reply.add(INodeNo(child_ino), (index + 1) as u64, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn statfs(&self, _req: &Request, _ino: INodeNo, reply: ReplyStatfs) {
        let files = self.state().nodes.len() as u64;
        reply.statfs(1 << 20, 1 << 20, 1 << 20, files, 1 << 20, 4096, 255, 4096);
    }

    fn access(&self, _req: &Request, _ino: INodeNo, _mask: AccessFlags, reply: ReplyEmpty) {
        reply.ok();
    }
}

/// The error a mount callback reports: a bad URL is `ENOENT`, everything else is `EIO`.
fn map_error(error: Error) -> Errno {
    match error {
        Error::Misuse(_) => Errno::ENOENT,
        _ => Errno::EIO,
    }
}

/// The repository URL of one FUSE path: the base plus each path segment percent-encoded.
///
/// A foreign object name may carry spaces, `?`, `#`, `%` or unicode; the search API
/// returns it decoded, so every segment is encoded before it becomes a URL.
fn repo_url(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    let rel = path.trim_start_matches('/');
    if rel.is_empty() {
        format!("{base}/")
    } else {
        format!("{base}/{}", encode_path(rel))
    }
}

fn encode_path(path: &str) -> String {
    path.split('/')
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_segment(segment: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(segment.len());
    for &byte in segment.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out
}

/// A stable synthetic inode for a listing entry the kernel has not looked up.
///
/// The high bit keeps it outside the real inode range, so a readdir entry can report
/// an inode before its lookup without colliding with an allocated one.
fn synthetic_ino(path: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in path.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash | (1 << 63)
}

/// `parent` plus one segment.
fn join(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

/// The parent of a path. The root is its own parent.
fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) | None => "/",
        Some(index) => &path[..index],
    }
}

#[cfg(test)]
mod tests {
    use super::{encode_path, join, parent_of, repo_url, synthetic_ino, Node, State};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn empty_state() -> State {
        State {
            nodes: HashMap::new(),
            paths: HashMap::new(),
            next_ino: 2,
            blocks: HashMap::new(),
            block_bytes: 0,
            listings: HashMap::new(),
            attrs: HashMap::new(),
            lookups: HashMap::new(),
        }
    }

    #[test]
    fn repo_url_encodes_foreign_names() {
        assert_eq!(repo_url("http://h/repo", "/"), "http://h/repo/");
        assert_eq!(repo_url("http://h/repo/", "/"), "http://h/repo/");
        assert_eq!(
            repo_url("http://h/repo/", "/a b/c?d#e%f"),
            "http://h/repo/a%20b/c%3Fd%23e%25f"
        );
    }

    #[test]
    fn path_encoding_keeps_unreserved_segments() {
        assert_eq!(encode_path("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(encode_path("a/b"), "a/b");
        assert_eq!(encode_path("é"), "%C3%A9");
    }

    #[test]
    fn join_keeps_one_slash() {
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/a", "b"), "/a/b");
    }

    #[test]
    fn parent_of_walks_up() {
        assert_eq!(parent_of("/"), "/");
        assert_eq!(parent_of("/a"), "/");
        assert_eq!(parent_of("/a/b"), "/a");
    }

    #[test]
    fn eviction_is_per_ino_and_keeps_the_byte_count() {
        let mut state = empty_state();
        state.blocks.insert((1, 0), Arc::new(vec![0; 10]));
        state.blocks.insert((1, 1), Arc::new(vec![0; 3]));
        state.blocks.insert((2, 0), Arc::new(vec![0; 7]));
        state.block_bytes = 20;
        state.evict_blocks(1);
        assert_eq!(state.blocks.len(), 1, "the other inode stays cached");
        assert!(state.blocks.contains_key(&(2, 0)));
        assert_eq!(
            state.block_bytes, 7,
            "the short block is subtracted exactly"
        );
    }

    #[test]
    fn forget_drops_a_node_after_the_last_lookup() {
        let mut state = empty_state();
        state.nodes.insert(
            2,
            Node {
                path: "/a".to_owned(),
            },
        );
        state.paths.insert("/a".to_owned(), 2);
        state.lookups.insert(2, 2);
        state.forget(2, 1);
        assert!(state.paths.contains_key("/a"), "one lookup remains");
        state.forget(2, 1);
        assert!(!state.paths.contains_key("/a"));
        assert!(!state.nodes.contains_key(&2));
    }

    #[test]
    fn synthetic_inodes_are_stable_and_high() {
        let a = synthetic_ino("/a/b");
        assert_eq!(a, synthetic_ino("/a/b"));
        assert_ne!(a, synthetic_ino("/a/c"));
        assert!(a > u64::MAX / 2);
    }
}
