// SPDX-License-Identifier: Apache-2.0
use crate::{lifecycle::Database, types::*};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Implementations must serialize writers and atomically commit or roll back the
/// entire callback, including request receipts. Hosted stores can use a DB transaction.
pub trait Store {
    fn transaction<T>(&self, action: impl FnOnce(&mut Database) -> Result<T>) -> Result<T>;
}
#[derive(Default)]
pub struct MemoryStore(Mutex<Database>);
impl Store for MemoryStore {
    fn transaction<T>(&self, action: impl FnOnce(&mut Database) -> Result<T>) -> Result<T> {
        let mut guard = self
            .0
            .lock()
            .map_err(|_| Error::Invalid("poisoned store"))?;
        let mut next = guard.clone();
        let result = action(&mut next)?;
        next.validate()?;
        *guard = next;
        Ok(result)
    }
}
#[derive(Serialize, Deserialize)]
struct Snapshot {
    sha256: Fingerprint,
    database: Database,
}
/// Durable local state. Use a Git-excluded .perfine directory owned by the caller.
/// The lock file is never renamed. OS locks release on process death.
pub struct FileStore {
    directory: PathBuf,
}
impl FileStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(path.as_ref())?;
        private_directory(path.as_ref())?;
        Ok(Self {
            directory: path.as_ref().to_path_buf(),
        })
    }
}
impl Store for FileStore {
    fn transaction<T>(&self, action: impl FnOnce(&mut Database) -> Result<T>) -> Result<T> {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.directory.join("state.lock"))?;
        lock.lock_exclusive()?;
        let path = self.directory.join("state-v1.json");
        let mut db = match fs::read(&path) {
            Ok(bytes) => {
                let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
                if fingerprint(&snapshot.database)? != snapshot.sha256 {
                    return Err(Error::Mismatch("durable snapshot checksum"));
                }
                snapshot.database
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Database::default(),
            Err(e) => return Err(e.into()),
        };
        db.validate()?;
        let result = action(&mut db)?;
        db.validate()?;
        let snapshot = Snapshot {
            sha256: fingerprint(&db)?,
            database: db,
        };
        let temporary = self
            .directory
            .join(format!("state-{}.tmp", ResourceId::generate().as_str()));
        let write = || -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec(&snapshot)?)?;
            file.sync_all()?;
            fs::rename(&temporary, &path)?;
            sync_directory(&self.directory)?;
            Ok(())
        };
        if let Err(error) = write() {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(result)
    }
}

/// Content-addressed local evidence. Artifact names never become filesystem paths.
/// Hosted adapters can implement ArtifactReader over their authorized object store.
pub struct EvidenceDirectory {
    directory: PathBuf,
}
impl EvidenceDirectory {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(path.as_ref())?;
        private_directory(path.as_ref())?;
        Ok(Self {
            directory: path.as_ref().to_path_buf(),
        })
    }
    pub fn put(&self, bytes: &[u8]) -> Result<Fingerprint> {
        if bytes.is_empty() {
            return Err(Error::Incomplete("empty artifact"));
        }
        let hash = digest(bytes);
        let path = self.directory.join(hash.as_str());
        let temporary = self
            .directory
            .join(format!("{}.tmp", ResourceId::generate().as_str()));
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        let write = || -> Result<()> {
            file.write_all(bytes)?;
            file.sync_all()?;
            // A hard link is atomic and does not overwrite an existing artifact.
            match fs::hard_link(&temporary, &path) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.read(&hash)?;
                }
                Err(e) => return Err(e.into()),
            }
            sync_directory(&self.directory)?;
            Ok(())
        }();
        let _ = fs::remove_file(&temporary);
        write?;
        Ok(hash)
    }
    pub fn read(&self, hash: &Fingerprint) -> Result<Vec<u8>> {
        let bytes = fs::read(self.directory.join(hash.as_str()))?;
        if digest(&bytes) != *hash {
            return Err(Error::Mismatch("stored artifact checksum"));
        }
        Ok(bytes)
    }
    /// Persist only a run whose artifacts were already verified. Ensure artifacts
    /// are present in this directory before publishing the manifest fingerprint.
    pub fn put_run(&self, run: &crate::evidence::VerifiedRun) -> Result<Fingerprint> {
        for artifact in run.manifest().artifacts.values() {
            self.read(&artifact.sha256)?;
        }
        self.put(&serde_json::to_vec(run.manifest())?)
    }
    pub fn load_run(
        &self,
        config: &crate::evidence::FrozenConfig,
        expected: &crate::evidence::RunIdentity,
        hash: &Fingerprint,
    ) -> Result<crate::evidence::VerifiedRun> {
        let manifest: crate::evidence::Manifest = serde_json::from_slice(&self.read(hash)?)?;
        let artifacts = manifest.artifacts.clone();
        crate::evidence::verify(config, expected, manifest, &|name: &Name| {
            let artifact = artifacts
                .get(name)
                .ok_or(Error::Incomplete("artifact manifest"))?;
            self.read(&artifact.sha256)
        })
    }
}

/// Persist directory entries where the host supports directory fsync. Windows
/// file handles are flushed before atomic rename; directory FlushFileBuffers is
/// unsupported, so do not fail an otherwise committed journal on that platform.
pub fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Restrict a caller-owned local state directory before writing private evidence.
pub fn private_directory(path: &Path) -> Result<()> {
    crate::adapters::host::private_directory(path)
}
