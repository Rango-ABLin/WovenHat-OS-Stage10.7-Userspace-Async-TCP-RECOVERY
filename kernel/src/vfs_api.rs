//! Stable VFS 2.0 boundary over the existing in-memory and disk-backed VFS.
use crate::vfs::{self, Error, NodeKind, OpenFileId};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Metadata { pub size: usize, pub directory: bool, pub generation: u64 }

pub trait FileSystem {
    fn stat(&self, path: &str) -> Result<Metadata, Error>;
    fn open(&self, path: &str) -> Result<OpenFileId, Error>;
    fn mkdir(&self, path: &str) -> Result<(), Error>;
    fn remove(&self, path: &str) -> Result<(), Error>;
}

pub struct SystemVfs;
impl FileSystem for SystemVfs {
    fn stat(&self, path: &str) -> Result<Metadata, Error> {
        let s = vfs::stat(path)?;
        Ok(Metadata { size: s.size, directory: s.kind == NodeKind::Directory, generation: 1 })
    }
    fn open(&self, path: &str) -> Result<OpenFileId, Error> { vfs::open(path) }
    fn mkdir(&self, path: &str) -> Result<(), Error> { vfs::mkdir(path) }
    fn remove(&self, path: &str) -> Result<(), Error> { vfs::remove(path) }
}

#[cfg(feature = "stage12-1-test")]
pub fn structural_self_test() -> bool {
    let fs = SystemVfs;
    let path = "/tmp/stage12-vfs.txt";
    let _ = fs.remove(path);
    if vfs::write_file(path, b"woven") .is_err() { return false }
    let Ok(meta) = fs.stat(path) else { return false };
    let Ok(file) = fs.open(path) else { return false };
    let mut bytes = [0u8; 5];
    let read_ok = vfs::read(file, &mut bytes) == Ok(5) && &bytes == b"woven";
    let close_ok = vfs::close_open_file(file).is_ok();
    let remove_ok = fs.remove(path).is_ok();
    read_ok && close_ok && remove_ok && meta.size == 5 && !meta.directory && meta.generation != 0
}
