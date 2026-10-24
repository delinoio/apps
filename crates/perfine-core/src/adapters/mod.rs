// SPDX-License-Identifier: Apache-2.0
//! Configured external-tool adapters. Docker images and dependency bundles are trusted
//! operator inputs, not repository-controlled configuration. No native verification is implied.
pub mod agent;
pub mod environment;
pub mod measurement;
mod process;
pub mod profile;

use crate::{evidence::ArtifactReader, types::*};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

/// Content-addressed local evidence, outside all candidate and agent mounts.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
}
impl ArtifactStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(root.as_ref())?;
        Ok(Self {
            root: root.as_ref().canonicalize()?,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn put(&self, bytes: &[u8]) -> Result<Fingerprint> {
        use std::io::Write;
        let hash = digest(bytes);
        let mut file = tempfile::NamedTempFile::new_in(&self.root)?;
        file.write_all(bytes)?;
        file.as_file().sync_all()?;
        match file.persist_noclobber(self.root.join(hash.as_str())) {
            Ok(_) => (),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                if self.get(&hash)? != bytes {
                    return Err(Error::Mismatch("artifact collision"));
                }
            }
            Err(e) => return Err(e.error.into()),
        }
        Ok(hash)
    }
    pub fn get(&self, hash: &Fingerprint) -> Result<Vec<u8>> {
        let bytes = fs::read(self.root.join(hash.as_str()))?;
        if digest(&bytes) != *hash {
            return Err(Error::Mismatch("stored artifact hash"));
        }
        Ok(bytes)
    }
}
impl ArtifactReader for ArtifactStore {
    fn read(&self, name: &Name) -> Result<Vec<u8>> {
        self.get(&Fingerprint::new(name.as_str())?)
    }
}
pub(crate) fn relative(path: &str) -> Result<&Path> {
    let p = Path::new(path);
    if p.as_os_str().is_empty()
        || p.components().any(|c| !matches!(c, Component::Normal(_)))
        || path.contains('\\')
    {
        return Err(Error::Invalid("relative regular path"));
    }
    Ok(p)
}
/// Reject symlink components, including links which currently resolve inside the root.
pub(crate) fn regular_file(root: &Path, path: &str) -> Result<PathBuf> {
    let mut full = root.to_path_buf();
    for part in relative(path)?.components() {
        full.push(part);
        if fs::symlink_metadata(&full)?.file_type().is_symlink() {
            return Err(Error::Invalid("symlink in evidence/source path"));
        }
    }
    if !full.is_file() {
        return Err(Error::Incomplete("regular file"));
    }
    Ok(full)
}
