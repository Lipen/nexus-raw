//! The opt-in disk cache of read blocks.
//!
//! A block is keyed by the object's marker digest, the block size and its index, so an overwritten
//! object gets new keys and an old entry can never serve stale bytes.
//! The cache is a hint: a failed read or write falls back to the network.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use nexus_raw_core::Digest;

/// One cache entry: a block of a digest-identified object.
pub struct Key {
    digest: Digest,
    block_bytes: u64,
    index: u64,
}

impl Key {
    pub fn new(digest: Digest, block_bytes: u64, index: u64) -> Self {
        Self {
            digest,
            block_bytes,
            index,
        }
    }
}

/// A bounded directory of blocks, one file per block.
pub struct DiskCache {
    dir: PathBuf,
    cap: u64,
    total: u64,
}

impl DiskCache {
    /// Open or create the cache directory and evict down to the budget when it is already over.
    pub fn new(dir: PathBuf, cap: u64) -> std::io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let mut cache = Self { dir, cap, total: 0 };
        cache.total = cache.scan();
        if cache.total > cap {
            cache.evict();
        }
        Ok(cache)
    }

    /// The cached block, when it is present.
    pub fn get(&self, key: &Key) -> Option<Vec<u8>> {
        let bytes = fs::read(self.path(key)).ok()?;
        if bytes.is_empty() {
            None
        } else {
            Some(bytes)
        }
    }

    /// Store one block atomically.
    ///
    /// A write failure is not a read failure: the caller has the bytes already.
    pub fn put(&mut self, key: &Key, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let path = self.path(key);
        let Some(parent) = path.parent() else {
            return;
        };
        if fs::create_dir_all(parent).is_err() {
            return;
        }
        let previous = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let temp = parent.join(format!(".tmp-{}", std::process::id()));
        let Ok(mut file) = fs::File::create(&temp) else {
            return;
        };
        let written = file.write_all(bytes);
        drop(file);
        if written.is_err() {
            let _ = fs::remove_file(&temp);
            return;
        }
        if fs::rename(&temp, &path).is_err() {
            let _ = fs::remove_file(&temp);
            return;
        }
        self.total = self.total.saturating_sub(previous) + bytes.len() as u64;
        if self.total > self.cap {
            self.evict();
        }
    }

    /// The entry path: `<dir>/<shard>/<digest>/<block-bytes>-<index>`.
    fn path(&self, key: &Key) -> PathBuf {
        let hex = key.digest.as_str();
        self.dir
            .join(&hex[..2])
            .join(hex)
            .join(format!("{}-{}", key.block_bytes, key.index))
    }

    /// The blocks under the three-level cache layout.
    fn entries(&self) -> Vec<(PathBuf, u64, SystemTime)> {
        let mut out = Vec::new();
        let Ok(shards) = fs::read_dir(&self.dir) else {
            return out;
        };
        for shard in shards.flatten() {
            let Ok(objects) = fs::read_dir(shard.path()) else {
                continue;
            };
            for object in objects.flatten() {
                let Ok(files) = fs::read_dir(object.path()) else {
                    continue;
                };
                for file in files.flatten() {
                    if let Ok(meta) = file.metadata() {
                        if meta.is_file() {
                            let at = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                            out.push((file.path(), meta.len(), at));
                        }
                    }
                }
            }
        }
        out
    }

    fn scan(&self) -> u64 {
        self.entries().iter().map(|(_, len, _)| len).sum()
    }

    /// Drop the oldest entries until the total fits the budget.
    fn evict(&mut self) {
        let mut entries = self.entries();
        entries.sort_by_key(|(_, _, at)| *at);
        let mut total: u64 = entries.iter().map(|(_, len, _)| len).sum();
        for (path, len, _) in entries {
            if total <= self.cap {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(len);
            }
        }
        self.total = total;
    }
}

#[cfg(test)]
mod tests {
    use super::{DiskCache, Key};
    use nexus_raw_core::Digest;
    use std::fs;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("nxr-mount-cache-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn put_then_get_round_trips() {
        let dir = temp("round");
        let mut cache = DiskCache::new(dir.clone(), 1 << 20).unwrap();
        let key = Key::new(Digest::of_bytes(b"object"), 4096, 0);
        assert!(cache.get(&key).is_none());
        cache.put(&key, b"block");
        assert_eq!(cache.get(&key).as_deref(), Some(&b"block"[..]));
        let other = Key::new(Digest::of_bytes(b"object"), 8192, 0);
        assert!(
            cache.get(&other).is_none(),
            "the block size is part of the key"
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_budget_evicts_the_oldest_blocks() {
        let dir = temp("evict");
        let mut cache = DiskCache::new(dir.clone(), 8).unwrap();
        let a = Key::new(Digest::of_bytes(b"a"), 4096, 0);
        let b = Key::new(Digest::of_bytes(b"b"), 4096, 0);
        let c = Key::new(Digest::of_bytes(b"c"), 4096, 0);
        cache.put(&a, b"1234");
        std::thread::sleep(std::time::Duration::from_millis(20));
        cache.put(&b, b"5678");
        assert_eq!(cache.total, 8);
        cache.put(&c, b"9012");
        assert!(cache.get(&a).is_none(), "the oldest block made room");
        assert!(cache.get(&b).is_some());
        assert!(cache.get(&c).is_some());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_reopen_counts_existing_blocks() {
        let dir = temp("reopen");
        let key = Key::new(Digest::of_bytes(b"object"), 4096, 0);
        {
            let mut cache = DiskCache::new(dir.clone(), 1 << 20).unwrap();
            cache.put(&key, b"123456");
        }
        let cache = DiskCache::new(dir.clone(), 1 << 20).unwrap();
        assert_eq!(cache.total, 6);
        assert_eq!(cache.get(&key).as_deref(), Some(&b"123456"[..]));
        let _ = fs::remove_dir_all(dir);
    }
}
