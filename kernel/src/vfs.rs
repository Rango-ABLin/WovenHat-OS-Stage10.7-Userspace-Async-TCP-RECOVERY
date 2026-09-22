use crate::irq_lock::IrqMutex as Mutex;
use unicode_normalization::UnicodeNormalization;

use crate::config::{
    MAX_OPEN_FILES, MAX_PATH_SIZE as PATH_CAPACITY, MAX_VFS_NODES as MAX_NODES, VFS_NODE_CAPACITY,
};

pub const NODE_CAPACITY: usize = VFS_NODE_CAPACITY;

/// Maximum bytes returned for a single directory entry name (excluding NUL).
pub const MAX_DIR_NAME: usize = 255;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    File,
    Directory,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct DiskPath {
    bytes: [u8; PATH_CAPACITY],
    length: usize,
}
impl DiskPath {
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.length]).unwrap_or("")
    }
}

#[derive(Clone, Copy)]
struct Node {
    path: [u8; PATH_CAPACITY],
    path_length: usize,
    data: [u8; NODE_CAPACITY],
    backing: Option<DiskPath>,
    length: usize,
    writable: bool,
    uid: u32,
    gid: u32,
    mode: u16,
    kind: NodeKind,
    occupied: bool,
}

impl Node {
    const fn empty() -> Self {
        Self {
            path: [0; PATH_CAPACITY],
            path_length: 0,
            data: [0; NODE_CAPACITY],
            backing: None,
            length: 0,
            writable: false,
            uid: 0,
            gid: 0,
            mode: 0,
            kind: NodeKind::File,
            occupied: false,
        }
    }

    const fn with_data(path: &[u8], data: &[u8], writable: bool) -> Self {
        let mut node = Self::empty();
        let mut index = 0;
        while index < path.len() {
            node.path[index] = path[index];
            index += 1;
        }
        index = 0;
        while index < data.len() {
            node.data[index] = data[index];
            index += 1;
        }
        node.path_length = path.len();
        node.length = data.len();
        node.writable = writable;
        node.mode = if writable { 0o666 } else { 0o444 };
        node.kind = NodeKind::File;
        node.occupied = true;
        node
    }

    const fn directory(path: &[u8]) -> Self {
        let mut node = Self::empty();
        let mut index = 0;
        while index < path.len() {
            node.path[index] = path[index];
            index += 1;
        }
        node.path_length = path.len();
        node.kind = NodeKind::Directory;
        node.writable = true;
        node.mode = 0o755;
        node.occupied = true;
        node
    }

    fn matches(&self, path: &str) -> bool {
        self.occupied && &self.path[..self.path_length] == path.as_bytes()
    }

    fn path_str(&self) -> &str {
        core::str::from_utf8(&self.path[..self.path_length]).unwrap_or("")
    }
}

/// Metadata returned by `stat`.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct Stat {
    pub kind: NodeKind,
    pub size: usize,
    pub writable: bool,
    pub uid: u32,
    pub gid: u32,
    pub mode: u16,
}

/// One directory entry returned by `readdir`.
#[derive(Clone, Copy)]
pub struct DirEntry {
    pub name: [u8; MAX_DIR_NAME],
    pub name_length: usize,
    pub kind: NodeKind,
}

impl DirEntry {
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_length]).unwrap_or("")
    }
}

struct Registry {
    versions: [u64; MAX_NODES],
    generations: [u64; MAX_NODES],
    nodes: [Node; MAX_NODES],
}

impl Registry {
    const fn boot() -> Self {
        let mut nodes = [Node::empty(); MAX_NODES];
        // Directories first so path walking can find parents.
        nodes[0] = Node::directory(b"/");
        nodes[1] = Node::directory(b"/etc");
        nodes[2] = Node::directory(b"/tmp");
        nodes[2].mode = 0o1777;
        nodes[3] = Node::directory(b"/mnt");
        nodes[4] = Node::directory(b"/bin");
        nodes[5] = Node::with_data(b"/etc/motd", b"Welcome to WovenHat OS.\n", false);
        nodes[6] = Node::with_data(
            b"/etc/version",
            b"WovenHat kernel 0.8.0 Multicore Foundation\n",
            false,
        );
        nodes[7] = Node::with_data(b"/tmp/vfs-self-test", b"", true);
        Self {
            nodes,
            versions: [0; MAX_NODES],
            generations: [0; MAX_NODES],
        }
    }

    const fn empty() -> Self {
        Self {
            nodes: [Node::empty(); MAX_NODES],
            versions: [0; MAX_NODES],
            generations: [0; MAX_NODES],
        }
    }

    fn insert(&mut self, path: &str, data: &[u8], writable: bool) -> Result<usize, Error> {
        validate_absolute_path(path)?;
        if data.len() > NODE_CAPACITY {
            return Err(Error::Full);
        }
        if self.nodes.iter().any(|node| node.matches(path)) {
            return Err(Error::AlreadyExists);
        }
        // Parent directory must exist (except for root itself).
        if path != "/" {
            let parent = parent_path(path).ok_or(Error::InvalidPath)?;
            let Some(parent_node) = self
                .nodes
                .iter()
                .find(|n| n.matches(parent) && n.kind == NodeKind::Directory)
            else {
                return Err(Error::NotFound);
            };
            if !access_allowed(parent_node, true) {
                return Err(Error::ReadOnly);
            }
        }
        let index = self
            .nodes
            .iter()
            .position(|node| !node.occupied)
            .ok_or(Error::Full)?;
        let node = &mut self.nodes[index];
        node.path[..path.len()].copy_from_slice(path.as_bytes());
        node.path_length = path.len();
        node.data[..data.len()].copy_from_slice(data);
        node.length = data.len();
        node.writable = writable;
        let credentials =
            crate::task::current_credentials_if_running().unwrap_or(crate::task::Credentials::ROOT);
        node.uid = credentials.uid;
        node.gid = credentials.gid;
        node.mode = if writable { 0o666 } else { 0o444 };
        node.kind = NodeKind::File;
        node.occupied = true;
        Ok(index)
    }

    fn mkdir(&mut self, path: &str) -> Result<usize, Error> {
        validate_absolute_path(path)?;
        if path == "/" {
            return Err(Error::AlreadyExists);
        }
        if self.nodes.iter().any(|node| node.matches(path)) {
            return Err(Error::AlreadyExists);
        }
        let parent = parent_path(path).ok_or(Error::InvalidPath)?;
        let Some(parent_node) = self
            .nodes
            .iter()
            .find(|n| n.matches(parent) && n.kind == NodeKind::Directory)
        else {
            return Err(Error::NotFound);
        };
        if !access_allowed(parent_node, true) {
            return Err(Error::ReadOnly);
        }
        let index = self
            .nodes
            .iter()
            .position(|node| !node.occupied)
            .ok_or(Error::Full)?;
        let node = &mut self.nodes[index];
        *node = Node::directory(path.as_bytes());
        // directory() copies from a slice; path may be longer than what const fn saw.
        node.path[..path.len()].copy_from_slice(path.as_bytes());
        node.path_length = path.len();
        let credentials =
            crate::task::current_credentials_if_running().unwrap_or(crate::task::Credentials::ROOT);
        node.uid = credentials.uid;
        node.gid = credentials.gid;
        Ok(index)
    }

    fn count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.occupied && node.path_length != 0)
            .count()
    }

    fn removable_index(&self, path: &str) -> Result<usize, Error> {
        validate_absolute_path(path)?;
        if path == "/" {
            return Err(Error::ReadOnly);
        }
        let index = self
            .nodes
            .iter()
            .position(|node| node.matches(path))
            .ok_or(Error::NotFound)?;
        let parent = parent_path(path).ok_or(Error::InvalidPath)?;
        let parent_node = self
            .nodes
            .iter()
            .find(|node| node.matches(parent) && node.kind == NodeKind::Directory)
            .ok_or(Error::NotFound)?;
        if !access_allowed(parent_node, true) {
            return Err(Error::ReadOnly);
        }
        let node = &self.nodes[index];
        // Refuse to remove non-empty directories.
        if node.kind == NodeKind::Directory {
            let dir_path = node.path_str();
            for other in self.nodes.iter().filter(|n| n.occupied) {
                if immediate_child_name(dir_path, other.path_str()).is_some() {
                    return Err(Error::NotEmpty);
                }
            }
        }
        Ok(index)
    }

    fn can_remove(&self, path: &str) -> Result<(), Error> {
        self.removable_index(path).map(|_| ())
    }

    fn remove(&mut self, path: &str) -> Result<(), Error> {
        let index = self.removable_index(path)?;
        self.generations[index] = self.generations[index].checked_add(1).ok_or(Error::Full)?;
        let node = &mut self.nodes[index];
        node.occupied = false;
        node.path_length = 0;
        node.length = 0;
        node.backing = None;
        node.writable = false;
        node.data.fill(0);
        node.path.fill(0);
        Ok(())
    }

    fn validate_rename(&self, old: &str, new: &str) -> Result<(), Error> {
        validate_absolute_path(old)?;
        validate_absolute_path(new)?;
        if old == "/" || new == "/" {
            return Err(Error::ReadOnly);
        }
        let index = self
            .nodes
            .iter()
            .position(|n| n.matches(old))
            .ok_or(Error::NotFound)?;
        if old == new {
            return Ok(());
        }
        if self.nodes.iter().any(|n| n.matches(new)) {
            return Err(Error::AlreadyExists);
        }
        if descendant_suffix(old, new).is_some() {
            return Err(Error::InvalidPath);
        }
        let parent = parent_path(new).ok_or(Error::InvalidPath)?;
        if !self
            .nodes
            .iter()
            .any(|n| n.matches(parent) && n.kind == NodeKind::Directory)
        {
            return Err(Error::NotFound);
        }
        let parent_node = self
            .nodes
            .iter()
            .find(|node| node.matches(parent) && node.kind == NodeKind::Directory)
            .ok_or(Error::NotFound)?;
        if !access_allowed(parent_node, true) {
            return Err(Error::ReadOnly);
        }
        let directory = self.nodes[index].kind == NodeKind::Directory;
        // Preflight every resulting path before changing any node. Keep node indices
        // stable so existing open-file descriptions continue to reference their data.
        for node in self.nodes.iter().filter(|n| n.occupied) {
            if directory {
                if let Some(suffix) = descendant_suffix(old, node.path_str()) {
                    if new.len() + suffix.len() > PATH_CAPACITY {
                        return Err(Error::InvalidPath);
                    }
                }
            }
            if let Some(backing) = node.backing {
                if directory {
                    if let Some(suffix) = descendant_suffix(old, backing.as_str()) {
                        if new.len() + suffix.len() > PATH_CAPACITY {
                            return Err(Error::InvalidPath);
                        }
                    }
                } else if backing.as_str() == old && new.len() > PATH_CAPACITY {
                    return Err(Error::InvalidPath);
                }
            }
        }
        Ok(())
    }

    fn rename(&mut self, old: &str, new: &str) -> Result<(), Error> {
        self.validate_rename(old, new)?;
        if old == new {
            return Ok(());
        }
        let index = self
            .nodes
            .iter()
            .position(|n| n.matches(old))
            .ok_or(Error::NotFound)?;
        let directory = self.nodes[index].kind == NodeKind::Directory;
        for (index, node) in self.nodes.iter_mut().enumerate() {
            if !node.occupied {
                continue;
            }
            let mut changed = false;
            let suffix_start = if node.matches(old)
                || (directory && descendant_suffix(old, node.path_str()).is_some())
            {
                Some(old.len())
            } else {
                None
            };
            if let Some(start) = suffix_start {
                changed = true;
                let suffix_len = node.path_length - start;
                node.path.copy_within(start..node.path_length, new.len());
                node.path[..new.len()].copy_from_slice(new.as_bytes());
                node.path_length = new.len() + suffix_len;
                node.path[node.path_length..].fill(0);
            }

            let backing_suffix_start = node.backing.as_ref().and_then(|backing| {
                let backing_path = backing.as_str();
                if backing_path == old
                    || (directory && descendant_suffix(old, backing_path).is_some())
                {
                    Some(old.len())
                } else {
                    None
                }
            });
            if let (Some(backing), Some(start)) = (node.backing.as_mut(), backing_suffix_start) {
                changed = true;
                let suffix_len = backing.length - start;
                backing.bytes.copy_within(start..backing.length, new.len());
                backing.bytes[..new.len()].copy_from_slice(new.as_bytes());
                backing.length = new.len() + suffix_len;
                backing.bytes[backing.length..].fill(0);
            }
            if changed {
                self.versions[index] = self.versions[index].wrapping_add(1);
            }
        }
        Ok(())
    }

    fn write_file(&mut self, path: &str, data: &[u8]) -> Result<(), Error> {
        validate_absolute_path(path)?;
        if data.len() > NODE_CAPACITY {
            return Err(Error::Full);
        }
        if let Some(index) = self.nodes.iter().position(|n| n.matches(path)) {
            let node = &mut self.nodes[index];
            if node.kind != NodeKind::File {
                return Err(Error::AlreadyExists);
            }
            if !node.writable || !access_allowed(node, true) {
                return Err(Error::ReadOnly);
            }
            node.backing = None;
            node.data.fill(0);
            node.data[..data.len()].copy_from_slice(data);
            node.length = data.len();
            self.versions[index] = self.versions[index].wrapping_add(1);
            crate::file_frames::replace(index, self.generations[index], data);
            return Ok(());
        }
        // Create new writable file (parent must exist).
        self.insert(path, data, true).map(|_| ())
    }

    fn stat(&self, path: &str) -> Result<Stat, Error> {
        validate_absolute_path(path)?;
        let node = self
            .nodes
            .iter()
            .find(|node| node.matches(path))
            .ok_or(Error::NotFound)?;
        if !access_allowed(node, false) {
            return Err(Error::ReadOnly);
        }
        Ok(Stat {
            kind: node.kind,
            size: node.length,
            writable: node.writable,
            uid: node.uid,
            gid: node.gid,
            mode: node.mode,
        })
    }

    /// Return the `index`-th direct child of `dir_path` (0-based).
    fn readdir(&self, dir_path: &str, index: usize) -> Result<DirEntry, Error> {
        validate_absolute_path(dir_path)?;
        let dir = self
            .nodes
            .iter()
            .find(|node| node.matches(dir_path) && node.kind == NodeKind::Directory)
            .ok_or(Error::NotFound)?;
        if !access_allowed(dir, false) {
            return Err(Error::ReadOnly);
        }
        let dir_path = dir.path_str();

        let mut seen = 0usize;
        for node in self.nodes.iter().filter(|n| n.occupied) {
            let child_path = node.path_str();
            let Some(name) = immediate_child_name(dir_path, child_path) else {
                continue;
            };
            if seen == index {
                let mut entry = DirEntry {
                    name: [0; MAX_DIR_NAME],
                    name_length: 0,
                    kind: node.kind,
                };
                let len = core::cmp::min(name.len(), MAX_DIR_NAME);
                entry.name[..len].copy_from_slice(name.as_bytes());
                entry.name_length = len;
                return Ok(entry);
            }
            seen += 1;
        }
        Err(Error::NotFound)
    }
}

fn access_allowed(node: &Node, write: bool) -> bool {
    let credentials =
        crate::task::current_credentials_if_running().unwrap_or(crate::task::Credentials::ROOT);
    if credentials.is_root() {
        return true;
    }
    let shift = if credentials.uid == node.uid {
        6
    } else if credentials.gid == node.gid {
        3
    } else {
        0
    };
    let mask = if write { 0o2 } else { 0o4 };
    ((node.mode >> shift) & mask) != 0
}

fn validate_absolute_path(path: &str) -> Result<(), Error> {
    if path.is_empty() || !path.starts_with('/') || path.len() > PATH_CAPACITY {
        return Err(Error::InvalidPath);
    }
    if path.as_bytes().contains(&0) {
        return Err(Error::InvalidPath);
    }
    // Store one canonical UTF-8 spelling for every name. FAT32 lookup also
    // normalizes names, while this boundary rejects decomposed input before it
    // can create duplicate VFS entries.
    if !is_nfc_path(path) {
        return Err(Error::InvalidPath);
    }
    // The VFS accepts canonical absolute paths. Shell/task resolvers handle
    // relative paths and dot components before entering this layer. Limit each
    // component to the directory-entry ABI so names are never truncated.
    if path != "/"
        && path[1..]
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == ".." || part.len() > MAX_DIR_NAME)
    {
        return Err(Error::InvalidPath);
    }
    Ok(())
}

fn is_nfc_path(path: &str) -> bool {
    let mut normalized = [0u8; PATH_CAPACITY];
    let mut length = 0usize;
    for ch in path.nfc() {
        let mut encoded = [0u8; 4];
        let bytes = ch.encode_utf8(&mut encoded).as_bytes();
        if length + bytes.len() > normalized.len() {
            return false;
        }
        normalized[length..length + bytes.len()].copy_from_slice(bytes);
        length += bytes.len();
    }
    normalized[..length] == *path.as_bytes()
}

/// Return the slash-prefixed suffix only at a directory boundary.
fn descendant_suffix<'a>(directory: &str, path: &'a str) -> Option<&'a str> {
    path.strip_prefix(directory)
        .filter(|suffix| suffix.starts_with('/'))
}

fn parent_path(path: &str) -> Option<&str> {
    if path == "/" {
        return None;
    }
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => Some("/"),
        Some(pos) => Some(&trimmed[..pos]),
        None => None,
    }
}

/// If `child` is an immediate child of `dir`, return the final path component.
fn immediate_child_name<'a>(dir: &str, child: &'a str) -> Option<&'a str> {
    if child == dir {
        return None;
    }
    if dir == "/" {
        if !child.starts_with('/') || child.len() < 2 {
            return None;
        }
        let rest = &child[1..];
        if rest.contains('/') {
            return None;
        }
        return Some(rest);
    }
    if !child.starts_with(dir) {
        return None;
    }
    let rest = &child[dir.len()..];
    if !rest.starts_with('/') {
        return None;
    }
    let name = &rest[1..];
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some(name)
}

static REGISTRY: Mutex<Registry> = Mutex::with_rank(Registry::boot(), 10);

/// Handle to a shared open-file description.
///
/// Process file-descriptor tables store these IDs. Multiple descriptors
/// (including across fork) can refer to the same description so that the
/// file offset is shared, matching POSIX semantics.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OpenFileId {
    slot: usize,
    epoch: u64,
}

struct OpenFileDescription {
    node: usize,
    generation: u64,
    epoch: u64,
    offset: usize,
    refcount: u32,
    occupied: bool,
}

impl OpenFileDescription {
    const fn empty() -> Self {
        Self {
            node: 0,
            generation: 0,
            epoch: 0,
            offset: 0,
            refcount: 0,
            occupied: false,
        }
    }
}

struct OpenFileTable {
    entries: [OpenFileDescription; MAX_OPEN_FILES],
    generations: [u64; MAX_OPEN_FILES],
}

impl OpenFileTable {
    const fn empty() -> Self {
        Self {
            entries: [const { OpenFileDescription::empty() }; MAX_OPEN_FILES],
            generations: [0; MAX_OPEN_FILES],
        }
    }

    fn alloc(&mut self, node: usize, generation: u64) -> Result<OpenFileId, Error> {
        let slot = self
            .entries
            .iter()
            .enumerate()
            .position(|(slot, entry)| !entry.occupied && self.generations[slot] < u64::MAX)
            .ok_or(Error::Full)?;
        // Keep slot reuse distinguishable from stale file-table snapshots.
        // Exhausted generations retire the slot rather than wrapping.
        let epoch = self.generations[slot].checked_add(1).ok_or(Error::Full)?;
        self.generations[slot] = epoch;
        self.entries[slot] = OpenFileDescription {
            node,
            generation,
            epoch,
            offset: 0,
            refcount: 1,
            occupied: true,
        };
        Ok(OpenFileId { slot, epoch })
    }

    fn get_mut(&mut self, id: OpenFileId) -> Result<&mut OpenFileDescription, Error> {
        self.entries
            .get_mut(id.slot)
            .filter(|entry| entry.occupied && entry.refcount > 0 && entry.epoch == id.epoch)
            .ok_or(Error::InvalidDescriptor)
    }

    fn clone_id(&mut self, id: OpenFileId) -> Result<OpenFileId, Error> {
        let entry = self.get_mut(id)?;
        entry.refcount = entry.refcount.checked_add(1).ok_or(Error::Full)?;
        Ok(id)
    }

    fn drop_id(&mut self, id: OpenFileId) -> Result<(), Error> {
        let entry = self
            .entries
            .get_mut(id.slot)
            .filter(|entry| entry.occupied && entry.epoch == id.epoch)
            .ok_or(Error::InvalidDescriptor)?;
        if entry.refcount == 0 {
            return Err(Error::InvalidDescriptor);
        }
        entry.refcount -= 1;
        if entry.refcount == 0 {
            *entry = OpenFileDescription::empty();
        }
        Ok(())
    }

    fn live_count(&self) -> usize {
        self.entries.iter().filter(|entry| entry.occupied).count()
    }
}

static OPEN_FILES: Mutex<OpenFileTable> = Mutex::with_rank(OpenFileTable::empty(), 10);

/// Backwards-compatible alias used by older call sites during the transition.
/// Prefer `OpenFileId` for new code.
#[allow(dead_code)]
pub type OpenFile = OpenFileId;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Error {
    NotFound,
    InvalidDescriptor,
    InvalidPath,
    AlreadyExists,
    ReadOnly,
    Full,
    NotEmpty,
    Io,
}

pub fn create_read_only(path: &str, data: &[u8]) -> Result<(), Error> {
    REGISTRY.lock().insert(path, data, false).map(|_| ())
}

/// Import disk metadata without copying the file payload into VFS RAM storage.
pub fn create_disk_file(path: &str, length: usize) -> Result<(), Error> {
    create_disk_file_with_writable(path, length, false)
}

/// Import disk metadata and set whether the VFS copy may be edited before
/// persistence. The payload stays lazy until first read or partial write.
pub fn create_disk_file_with_writable(
    path: &str,
    length: usize,
    writable: bool,
) -> Result<(), Error> {
    if length > NODE_CAPACITY {
        return Err(Error::Full);
    }
    let mut registry = REGISTRY.lock();
    let index = registry.insert(path, &[], writable)?;
    let mut backing = DiskPath {
        bytes: [0; PATH_CAPACITY],
        length: path.len(),
    };
    backing.bytes[..path.len()].copy_from_slice(path.as_bytes());
    registry.nodes[index].backing = Some(backing);
    registry.nodes[index].length = length;
    Ok(())
}

/// Apply persisted ownership metadata to a disk-backed VFS node.
pub fn set_metadata(path: &str, uid: u32, gid: u32, mode: u16) -> Result<(), Error> {
    validate_absolute_path(path)?;
    let mut registry = REGISTRY.lock();
    let node = registry
        .nodes
        .iter_mut()
        .find(|node| node.matches(path))
        .ok_or(Error::NotFound)?;
    node.uid = uid;
    node.gid = gid;
    node.mode = mode;
    node.writable = mode & 0o222 != 0;
    Ok(())
}

/// Create or overwrite a writable file.
pub fn write_file(path: &str, data: &[u8]) -> Result<(), Error> {
    let result = REGISTRY.lock().write_file(path, data);
    if result.is_ok() && path.starts_with("/mnt/") {
        crate::storage::mark_mnt_dirty();
    }
    result
}

/// Validate removal without mutating the registry.
pub fn can_remove(path: &str) -> Result<(), Error> {
    REGISTRY.lock().can_remove(path)
}

/// Preserve disk-backed bytes for an open file before its directory entry is
/// removed from the mounted disk. This keeps POSIX-style unlink semantics for
/// open descriptors even when the backing FAT32 name is about to disappear.
pub fn prepare_remove(path: &str) -> Result<(), Error> {
    validate_absolute_path(path)?;
    for _ in 0..8 {
        let pending = {
            let files = OPEN_FILES.lock();
            let registry = REGISTRY.lock();
            let index = registry.removable_index(path)?;
            let open_file = registry.nodes[index].kind == NodeKind::File
                && files
                    .entries
                    .iter()
                    .any(|entry| entry.occupied && entry.node == index);
            if !open_file || registry.nodes[index].backing.is_none() {
                return Ok(());
            }
            (index, registry.generations[index])
        };
        materialize_disk_node(pending.0, pending.1)?;
    }
    Err(Error::Io)
}

/// Remove a file or empty directory.
pub fn remove(path: &str) -> Result<(), Error> {
    validate_absolute_path(path)?;
    for _ in 0..8 {
        let pending = {
            let files = OPEN_FILES.lock();
            let mut registry = REGISTRY.lock();
            let index = registry
                .nodes
                .iter()
                .position(|node| node.matches(path))
                .ok_or(Error::NotFound)?;
            let open_file = registry.nodes[index].kind == NodeKind::File
                && files
                    .entries
                    .iter()
                    .any(|entry| entry.occupied && entry.node == index);
            if open_file && registry.nodes[index].backing.is_some() {
                Some((index, registry.generations[index]))
            } else if open_file {
                // Open descriptors retain the unnamed inode after unlink.
                registry.nodes[index].path_length = 0;
                registry.nodes[index].path.fill(0);
                return Ok(());
            } else {
                let result = registry.remove(path);
                if result.is_ok() && path.starts_with("/mnt/") {
                    crate::storage::mark_mnt_dirty();
                }
                return result;
            }
        };
        if let Some((index, generation)) = pending {
            // FAT32 backing is path-based; preserve its bytes before releasing
            // the name so a replacement cannot redirect the old descriptor.
            materialize_disk_node(index, generation)?;
        }
    }
    Err(Error::Io)
}

pub fn can_rename(old: &str, new: &str) -> Result<(), Error> {
    REGISTRY.lock().validate_rename(old, new)
}

pub fn rename(old: &str, new: &str) -> Result<(), Error> {
    let result = REGISTRY.lock().rename(old, new);
    if result.is_ok() && (old.starts_with("/mnt/") || new.starts_with("/mnt/")) {
        crate::storage::mark_mnt_dirty();
    }
    result
}

/// Create a directory. Parent must already exist.
pub fn mkdir(path: &str) -> Result<(), Error> {
    let result = REGISTRY.lock().mkdir(path).map(|_| ());
    if result.is_ok() && path.starts_with("/mnt/") {
        crate::storage::mark_mnt_dirty();
    }
    result
}

/// Query metadata for a path.
pub fn stat(path: &str) -> Result<Stat, Error> {
    REGISTRY.lock().stat(path)
}

/// Read the `index`-th entry of a directory (0-based). Returns `NotFound` when exhausted.
pub fn readdir(path: &str, index: usize) -> Result<DirEntry, Error> {
    REGISTRY.lock().readdir(path, index)
}

/// Open a **file** path and return a new open-file description (refcount = 1).
/// Directories cannot be opened for read/write; use `readdir` / `stat` instead.
pub fn open(path: &str) -> Result<OpenFileId, Error> {
    validate_absolute_path(path)?;
    let mut open_files = OPEN_FILES.lock();
    let registry = REGISTRY.lock();
    let node = registry
        .nodes
        .iter()
        .position(|node| node.matches(path) && node.kind == NodeKind::File)
        .ok_or(Error::NotFound)?;
    let generation = registry.generations[node];
    drop(registry);
    open_files.alloc(node, generation)
}

/// Increase the reference count of an existing open-file description.
/// Used by fork (and future dup) so parent and child share the offset.
pub fn clone_open_file(id: OpenFileId) -> Result<OpenFileId, Error> {
    OPEN_FILES.lock().clone_id(id)
}

/// Decrease the reference count. Frees the description when it reaches zero.
pub fn close_open_file(id: OpenFileId) -> Result<(), Error> {
    let mut files = OPEN_FILES.lock();
    let node = files.get_mut(id)?.node;
    files.drop_id(id)?;
    if !files
        .entries
        .iter()
        .any(|entry| entry.occupied && entry.node == node)
    {
        let mut registry = REGISTRY.lock();
        if registry.nodes[node].occupied && registry.nodes[node].path_length == 0 {
            registry.generations[node] = registry.generations[node]
                .checked_add(1)
                .ok_or(Error::Full)?;
            registry.nodes[node].occupied = false;
            registry.nodes[node].length = 0;
            registry.nodes[node].backing = None;
            registry.nodes[node].data.fill(0);
        }
    }
    Ok(())
}

/// Capture node identity so a lazy fault cannot read a reused VFS slot.
pub fn file_identity(id: OpenFileId) -> Result<(usize, u64, u64), Error> {
    let mut table = OPEN_FILES.lock();
    let entry = table.get_mut(id)?;
    let registry = REGISTRY.lock();
    if !registry.nodes[entry.node].occupied || registry.generations[entry.node] != entry.generation
    {
        return Err(Error::InvalidDescriptor);
    }
    Ok((entry.node, entry.generation, registry.versions[entry.node]))
}
fn materialize_disk_node(index: usize, generation: u64) -> Result<(), Error> {
    // Preserve the old disk image for unlink/write without holding VFS locks
    // through ATA I/O. A conflicting rename or write retries from a fresh
    // snapshot; a reused node slot is rejected by its generation.
    let mut bytes = [0u8; NODE_CAPACITY];
    for _ in 0..8 {
        let (backing, length, version) = {
            let registry = REGISTRY.lock();
            let node = registry.nodes.get(index).ok_or(Error::InvalidDescriptor)?;
            if !node.occupied || registry.generations[index] != generation {
                return Err(Error::InvalidDescriptor);
            }
            let Some(backing) = node.backing else {
                return Ok(());
            };
            (backing, node.length, registry.versions[index])
        };
        let read = crate::task::file_fault_io(|| {
            crate::storage::read_disk_file(backing.as_str(), 0, &mut bytes[..length])
        });
        let mut registry = REGISTRY.lock();
        let node = &registry.nodes[index];
        if !node.occupied || registry.generations[index] != generation {
            return Err(Error::InvalidDescriptor);
        }
        if node.backing != Some(backing)
            || node.length != length
            || registry.versions[index] != version
        {
            continue;
        }
        if read.map_err(|_| Error::Io)? != length {
            return Err(Error::Io);
        }
        registry.nodes[index].data[..length].copy_from_slice(&bytes[..length]);
        registry.nodes[index].backing = None;
        return Ok(());
    }
    Err(Error::Io)
}
/// Writable shared disk mappings keep a RAM inode image for bounded writeback.
/// The capability check is performed by the syscall before reaching this API.
pub fn prepare_shared_file(id: OpenFileId) -> Result<(), Error> {
    for _ in 0..8 {
        let (index, generation) = {
            let mut table = OPEN_FILES.lock();
            let entry = table.get_mut(id)?;
            let registry = REGISTRY.lock();
            let node = &registry.nodes[entry.node];
            if !node.occupied || registry.generations[entry.node] != entry.generation {
                return Err(Error::InvalidDescriptor);
            }
            if node.writable && access_allowed(node, true) {
                return Ok(());
            }
            if node.backing.is_none() {
                return Err(Error::ReadOnly);
            }
            (entry.node, entry.generation)
        };
        materialize_disk_node(index, generation)?;
        let mut table = OPEN_FILES.lock();
        let entry = table.get_mut(id)?;
        if entry.node != index || entry.generation != generation {
            return Err(Error::InvalidDescriptor);
        }
        let mut registry = REGISTRY.lock();
        if !registry.nodes[index].occupied || registry.generations[index] != generation {
            return Err(Error::InvalidDescriptor);
        }
        if registry.nodes[index].backing.is_some() {
            continue;
        }
        registry.nodes[index].writable = true;
        return Ok(());
    }
    Err(Error::Io)
}
pub fn write_mapping_at(id: OpenFileId, offset: usize, bytes: &[u8]) -> Result<(), Error> {
    let mut table = OPEN_FILES.lock();
    let entry = table.get_mut(id)?;
    let mut registry = REGISTRY.lock();
    let node = &mut registry.nodes[entry.node];
    if !node.occupied || !node.writable || !access_allowed(node, true) {
        return Err(Error::ReadOnly);
    }
    if offset
        .checked_add(bytes.len())
        .is_none_or(|end| end > node.length)
    {
        return Err(Error::Full);
    }
    node.data[offset..offset + bytes.len()].copy_from_slice(bytes);
    registry.versions[entry.node] = registry.versions[entry.node].wrapping_add(1);
    crate::file_frames::update(entry.node, entry.generation, offset, bytes);
    Ok(())
}
pub fn persist_mapping(id: OpenFileId) -> Result<(), Error> {
    let mut path = [0; PATH_CAPACITY];
    let length = {
        let mut files = OPEN_FILES.lock();
        let entry = files.get_mut(id)?;
        let registry = REGISTRY.lock();
        let node = &registry.nodes[entry.node];
        path[..node.path_length].copy_from_slice(&node.path[..node.path_length]);
        node.path_length
    };
    let path = core::str::from_utf8(&path[..length]).map_err(|_| Error::InvalidPath)?;
    if path.starts_with("/mnt/") {
        crate::storage::persist_path(path).map_err(|_| Error::Io)?;
    }
    Ok(())
}

pub fn file_generation(id: OpenFileId) -> Result<u64, Error> {
    let mut table = OPEN_FILES.lock();
    let entry = table.get_mut(id)?;
    let registry = REGISTRY.lock();
    if !registry.nodes[entry.node].occupied || registry.generations[entry.node] != entry.generation
    {
        return Err(Error::InvalidDescriptor);
    }
    Ok(entry.generation)
}
pub fn read_mapping_at(
    id: OpenFileId,
    generation: u64,
    offset: usize,
    buffer: &mut [u8],
) -> Result<usize, Error> {
    read_from(id, Some(offset), buffer, Some(generation))
}

pub fn file_size(id: OpenFileId) -> Result<usize, Error> {
    let mut table = OPEN_FILES.lock();
    let index = table.get_mut(id)?.node;
    // Keep the inner guard in a named binding. A chained temporary in the
    // return expression outlives `table` during function teardown.
    let registry = REGISTRY.lock();
    registry
        .nodes
        .get(index)
        .filter(|node| node.occupied && node.kind == NodeKind::File)
        .map(|node| node.length)
        .ok_or(Error::InvalidDescriptor)
}

pub fn open_file_path_starts_with(id: OpenFileId, prefix: &str) -> Result<bool, Error> {
    let mut table = OPEN_FILES.lock();
    let entry = table.get_mut(id)?;
    let registry = REGISTRY.lock();
    let node = registry
        .nodes
        .get(entry.node)
        .filter(|node| node.occupied)
        .ok_or(Error::InvalidDescriptor)?;
    Ok(node.path_str().starts_with(prefix))
}
/// Positional reads do not alter the shared open-file offset.
pub fn read_at(id: OpenFileId, offset: usize, buffer: &mut [u8]) -> Result<usize, Error> {
    read_from(id, Some(offset), buffer, None)
}

pub fn read(id: OpenFileId, buffer: &mut [u8]) -> Result<usize, Error> {
    read_from(id, None, buffer, None)
}

fn read_from(
    id: OpenFileId,
    position: Option<usize>,
    buffer: &mut [u8],
    generation: Option<u64>,
) -> Result<usize, Error> {
    // Disk I/O must not retain the open-description or node registry lock.
    // Retry when another CPU changes the node or shared seek position while
    // the read is in flight, so the returned bytes match the committed offset.
    for _ in 0..8 {
        let (node_index, node_generation, offset, count, backing, version, length) = {
            let mut table = OPEN_FILES.lock();
            let entry = table.get_mut(id)?;
            let node_index = entry.node;
            let node_generation = entry.generation;
            let offset = position.unwrap_or(entry.offset);
            let registry = REGISTRY.lock();
            if registry.generations[node_index] != node_generation
                || generation.is_some_and(|value| value != node_generation)
            {
                return Err(Error::InvalidDescriptor);
            }
            let node = registry
                .nodes
                .get(node_index)
                .filter(|node| node.occupied)
                .ok_or(Error::InvalidDescriptor)?;
            if !access_allowed(node, false) {
                return Err(Error::ReadOnly);
            }
            if offset > node.length {
                return Err(Error::InvalidDescriptor);
            }
            let count = core::cmp::min(node.length - offset, buffer.len());
            if node.backing.is_none() || count == 0 {
                buffer[..count].copy_from_slice(&node.data[offset..offset + count]);
                crate::file_frames::overlay(
                    node_index,
                    node_generation,
                    offset,
                    &mut buffer[..count],
                );
                if position.is_none() {
                    entry.offset = offset + count;
                }
                return Ok(count);
            }
            (
                node_index,
                node_generation,
                offset,
                count,
                node.backing.unwrap(),
                registry.versions[node_index],
                node.length,
            )
        };

        let read = crate::task::file_fault_io(|| {
            crate::storage::read_disk_file(backing.as_str(), offset, &mut buffer[..count])
        });
        let mut table = OPEN_FILES.lock();
        let entry = table.get_mut(id)?;
        if entry.node != node_index || entry.generation != node_generation {
            return Err(Error::InvalidDescriptor);
        }
        let registry = REGISTRY.lock();
        let node = &registry.nodes[node_index];
        if !node.occupied || registry.generations[node_index] != node_generation {
            return Err(Error::InvalidDescriptor);
        }
        if !access_allowed(node, false) {
            return Err(Error::ReadOnly);
        }
        if registry.versions[node_index] != version
            || node.backing != Some(backing)
            || node.length != length
            || (position.is_none() && entry.offset != offset)
        {
            continue;
        }
        if read.map_err(|_| Error::Io)? != count {
            return Err(Error::Io);
        }
        crate::file_frames::overlay(node_index, node_generation, offset, &mut buffer[..count]);
        if position.is_none() {
            entry.offset = offset + count;
        }
        return Ok(count);
    }
    Err(Error::Io)
}

pub fn read_all(path: &str, buffer: &mut [u8]) -> Result<usize, Error> {
    let file = open(path)?;
    let result = (|| {
        let size = file_size(file)?;
        if size > buffer.len() {
            return Err(Error::Full);
        }
        read_at(file, 0, &mut buffer[..size])
    })();
    let _ = close_open_file(file);
    result
}

pub fn seek(id: OpenFileId, offset: usize) -> Result<usize, Error> {
    let mut table = OPEN_FILES.lock();
    let entry = table.get_mut(id)?;
    let registry = REGISTRY.lock();
    let node = registry
        .nodes
        .get(entry.node)
        .filter(|n| n.occupied)
        .ok_or(Error::NotFound)?;
    let max = node.length;
    drop(registry);
    // Allow seek to end (offset == length) for append; not past EOF beyond length for simplicity.
    let pos = core::cmp::min(offset, max);
    entry.offset = pos;
    Ok(pos)
}

/// Positional writes do not alter the shared open-file offset.
/// Stage 10.5 uses this for asynchronous file I/O so concurrent requests do
/// not race through the descriptor's mutable seek position.
pub fn write_at(id: OpenFileId, offset: usize, buffer: &[u8]) -> Result<usize, Error> {
    for _ in 0..8 {
        let (node_index, generation) = {
            let mut table = OPEN_FILES.lock();
            let entry = table.get_mut(id)?;
            let registry = REGISTRY.lock();
            let node = &registry.nodes[entry.node];
            if !node.occupied || registry.generations[entry.node] != entry.generation {
                return Err(Error::InvalidDescriptor);
            }
            if !node.writable || !access_allowed(node, true) {
                return Err(Error::ReadOnly);
            }
            (entry.node, entry.generation)
        };
        materialize_disk_node(node_index, generation)?;
        let mut table = OPEN_FILES.lock();
        let entry = table.get_mut(id)?;
        if entry.node != node_index || entry.generation != generation {
            return Err(Error::InvalidDescriptor);
        }
        let mut registry = REGISTRY.lock();
        if registry.generations[node_index] != generation {
            return Err(Error::InvalidDescriptor);
        }
        let node = registry
            .nodes
            .get_mut(node_index)
            .filter(|node| node.occupied)
            .ok_or(Error::InvalidDescriptor)?;
        if !node.writable || !access_allowed(node, true) {
            return Err(Error::ReadOnly);
        }
        if node.backing.is_some() {
            continue;
        }
        if offset > node.length {
            return Err(Error::InvalidDescriptor);
        }
        let count = core::cmp::min(buffer.len(), NODE_CAPACITY - offset);
        node.data[offset..offset + count].copy_from_slice(&buffer[..count]);
        let new_end = offset + count;
        node.length = core::cmp::max(node.length, new_end);
        let short_write = count < buffer.len();
        registry.versions[node_index] = registry.versions[node_index].wrapping_add(1);
        crate::file_frames::update(node_index, generation, offset, &buffer[..count]);
        if short_write {
            return Err(Error::Full);
        }
        return Ok(count);
    }
    Err(Error::Io)
}

pub fn write(id: OpenFileId, buffer: &[u8]) -> Result<usize, Error> {
    for _ in 0..8 {
        let (node_index, generation) = {
            let mut table = OPEN_FILES.lock();
            let entry = table.get_mut(id)?;
            let registry = REGISTRY.lock();
            let node = &registry.nodes[entry.node];
            if !node.occupied || registry.generations[entry.node] != entry.generation {
                return Err(Error::InvalidDescriptor);
            }
            if !node.writable || !access_allowed(node, true) {
                return Err(Error::ReadOnly);
            }
            (entry.node, entry.generation)
        };
        materialize_disk_node(node_index, generation)?;
        let mut table = OPEN_FILES.lock();
        let entry = table.get_mut(id)?;
        if entry.node != node_index || entry.generation != generation {
            return Err(Error::InvalidDescriptor);
        }
        let offset = entry.offset;
        let mut registry = REGISTRY.lock();
        if registry.generations[node_index] != generation {
            return Err(Error::InvalidDescriptor);
        }
        let node = registry
            .nodes
            .get_mut(node_index)
            .filter(|node| node.occupied)
            .ok_or(Error::InvalidDescriptor)?;
        if !node.writable || !access_allowed(node, true) {
            return Err(Error::ReadOnly);
        }
        if node.backing.is_some() {
            continue;
        }
        if offset > node.length {
            return Err(Error::InvalidDescriptor);
        }
        let count = core::cmp::min(buffer.len(), NODE_CAPACITY - offset);
        node.data[offset..offset + count].copy_from_slice(&buffer[..count]);
        let new_offset = offset + count;
        node.length = core::cmp::max(node.length, new_offset);
        let short_write = count < buffer.len();
        registry.versions[node_index] = registry.versions[node_index].wrapping_add(1);
        crate::file_frames::update(node_index, generation, offset, &buffer[..count]);
        entry.offset = new_offset;
        if short_write {
            return Err(Error::Full);
        }
        return Ok(count);
    }
    Err(Error::Io)
}

/// Invoke `f` for every occupied file node whose path starts with `prefix`.
/// Directories are skipped. Intended for storage layer discovery under `/mnt`.
pub fn for_each_file_with_prefix<F>(prefix: &str, mut f: F)
where
    F: FnMut(&str),
{
    // Callers may allocate or persist in the callback. Snapshot bounded names
    // first so no external code runs while the registry guard is held.
    let mut paths = [[0u8; PATH_CAPACITY]; MAX_NODES];
    let mut lengths = [0usize; MAX_NODES];
    let mut count = 0usize;
    {
        let registry = REGISTRY.lock();
        for node in registry.nodes.iter() {
            if node.occupied && node.kind == NodeKind::File && node.path_str().starts_with(prefix) {
                paths[count][..node.path_length].copy_from_slice(&node.path[..node.path_length]);
                lengths[count] = node.path_length;
                count += 1;
            }
        }
    }
    for index in 0..count {
        if let Ok(path) = core::str::from_utf8(&paths[index][..lengths[index]]) {
            f(path);
        }
    }
}

pub fn node_count() -> usize {
    REGISTRY.lock().count()
}

pub fn open_file_description_count() -> usize {
    OPEN_FILES.lock().live_count()
}

pub fn self_test() -> bool {
    // The registry exceeds the 1 MiB boot stack. Reserve scratch storage
    // statically and reset one node at a time so repeated self-tests are safe.
    static SCRATCH: Mutex<Registry> = Mutex::with_rank(Registry::empty(), 10);
    let mut scratch = SCRATCH.lock();
    for node in &mut scratch.nodes {
        *node = Node::empty();
    }
    // Parent directories required before inserting files.
    let root_ok = scratch.mkdir("/").is_err(); // root cannot be created via mkdir on empty
    scratch.nodes[0] = Node::directory(b"/");
    let mnt_ok = scratch.mkdir("/mnt") == Ok(1);
    let inserted = scratch.insert("/mnt/test.txt", b"mounted", false).is_ok();
    let duplicate = scratch.insert("/mnt/test.txt", b"again", false) == Err(Error::AlreadyExists);
    let invalid_path = scratch.insert("relative", b"bad", false) == Err(Error::InvalidPath);
    let no_parent = scratch.insert("/missing/file", b"x", false) == Err(Error::NotFound);

    let path_semantics = path_semantics_self_test(&mut scratch);
    let disk_write_semantics = disk_write_semantics_self_test();
    let backing_rename_semantics = backing_rename_semantics_self_test();

    // Directory listing on the live registry.
    let root_stat = matches!(
        stat("/"),
        Ok(Stat {
            kind: NodeKind::Directory,
            ..
        })
    );
    let etc_stat = matches!(
        stat("/etc"),
        Ok(Stat {
            kind: NodeKind::Directory,
            ..
        })
    );
    let file_stat = matches!(
        stat("/etc/motd"),
        Ok(Stat {
            kind: NodeKind::File,
            size: 24,
            writable: false,
            uid: 0,
            gid: 0,
            mode: 0o444,
            ..
        })
    );

    // / should contain at least etc and tmp.
    let mut found_etc = false;
    let mut found_tmp = false;
    let mut index = 0;
    while let Ok(entry) = readdir("/", index) {
        if entry.name_str() == "etc" {
            found_etc = true;
        }
        if entry.name_str() == "tmp" {
            found_tmp = true;
        }
        index += 1;
        if index > 16 {
            break;
        }
    }
    let readdir_ok = found_etc && found_tmp;

    let Ok(writer) = open("/tmp/vfs-self-test") else {
        return false;
    };
    let payload = b"wovenhat-vfs";
    if write(writer, payload) != Ok(payload.len()) {
        let _ = close_open_file(writer);
        return false;
    }

    // Shared offset: cloning the description must observe the advanced offset.
    let Ok(shared) = clone_open_file(writer) else {
        let _ = close_open_file(writer);
        return false;
    };
    let mut probe = [0; 4];
    let shared_offset_advanced = read(shared, &mut probe) == Ok(0);

    // Independent open starts at offset 0 again.
    let Ok(reader) = open("/tmp/vfs-self-test") else {
        let _ = close_open_file(shared);
        let _ = close_open_file(writer);
        return false;
    };
    let mut buffer = [0; 12];
    let round_trip = read(reader, &mut buffer) == Ok(payload.len()) && buffer == *payload;

    let Ok(protected) = open("/etc/motd") else {
        let _ = close_open_file(reader);
        let _ = close_open_file(shared);
        let _ = close_open_file(writer);
        return false;
    };
    let mut complete = [0; 32];
    let read_only_ok = write(protected, b"x") == Err(Error::ReadOnly)
        && read_all("/etc/motd", &mut complete) == Ok(24)
        && &complete[..24] == b"Welcome to WovenHat OS.\n";

    // Directories cannot be opened as files.
    let dir_open_rejected = open("/etc").is_err();

    // A descriptor opened before a directory move still reads the same file.
    let moved = rename("/tmp", "/vfs-moved").is_ok();
    let handle_survives = moved
        && seek(reader, 0) == Ok(0)
        && read(reader, &mut buffer) == Ok(payload.len())
        && buffer == *payload
        && stat("/tmp/vfs-self-test").is_err()
        && stat("/vfs-moved/vfs-self-test").is_ok();
    let restored = moved && rename("/vfs-moved", "/tmp").is_ok();

    let _ = close_open_file(protected);
    let _ = close_open_file(reader);
    let _ = close_open_file(shared);
    let _ = close_open_file(writer);

    // Reusing an open-description slot must not revive an older kernel handle.
    let stale_handle_rejected = match open("/tmp/vfs-self-test") {
        Ok(reused) => {
            let rejected = reused != writer
                && clone_open_file(writer) == Err(Error::InvalidDescriptor)
                && file_size(writer) == Err(Error::InvalidDescriptor);
            let _ = close_open_file(reused);
            rejected
        }
        Err(_) => false,
    };

    // After closing all clones the description table should release the slots.
    let cleaned = open_file_description_count() == 0;

    // Boot registry: /, /etc, /tmp, motd, version, vfs-self-test
    let boot_nodes = node_count() == 8;

    root_ok
        && mnt_ok
        && inserted
        && duplicate
        && invalid_path
        && no_parent
        && path_semantics
        && disk_write_semantics
        && backing_rename_semantics
        && root_stat
        && etc_stat
        && file_stat
        && readdir_ok
        && shared_offset_advanced
        && round_trip
        && read_only_ok
        && dir_open_rejected
        && handle_survives
        && restored
        && stale_handle_rejected
        && cleaned
        && boot_nodes
}

fn backing_rename_semantics_self_test() -> bool {
    static SCRATCH: Mutex<Registry> = Mutex::with_rank(Registry::empty(), 10);
    let mut fs = SCRATCH.lock();
    for node in &mut fs.nodes {
        *node = Node::empty();
    }
    fs.nodes[0] = Node::directory(b"/");
    if fs.mkdir("/mnt").is_err()
        || fs.mkdir("/mnt/docs").is_err()
        || fs.insert("/mnt/docs/a.txt", &[], true).is_err()
    {
        return false;
    }
    let Some(index) = fs
        .nodes
        .iter()
        .position(|node| node.matches("/mnt/docs/a.txt"))
    else {
        return false;
    };
    set_scratch_backing(&mut fs.nodes[index], "/mnt/docs/a.txt", 7);
    fs.rename("/mnt/docs", "/mnt/archive").is_ok()
        && fs.nodes[index].matches("/mnt/archive/a.txt")
        && fs.nodes[index]
            .backing
            .is_some_and(|backing| backing.as_str() == "/mnt/archive/a.txt")
}

fn disk_write_semantics_self_test() -> bool {
    static SCRATCH: Mutex<Registry> = Mutex::with_rank(Registry::empty(), 10);
    let mut fs = SCRATCH.lock();
    for node in &mut fs.nodes {
        *node = Node::empty();
    }
    fs.nodes[0] = Node::directory(b"/");
    if fs.mkdir("/mnt").is_err() {
        return false;
    }

    let Ok(ro_index) = fs.insert("/mnt/ro.txt", &[], false) else {
        return false;
    };
    set_scratch_backing(&mut fs.nodes[ro_index], "/mnt/ro.txt", 5);
    let ro_protected = fs.write_file("/mnt/ro.txt", b"new") == Err(Error::ReadOnly)
        && fs.nodes[ro_index].backing.is_some()
        && fs.nodes[ro_index].length == 5;

    let Ok(rw_index) = fs.insert("/mnt/rw.txt", &[], true) else {
        return false;
    };
    set_scratch_backing(&mut fs.nodes[rw_index], "/mnt/rw.txt", 5);
    let rw_overwritten = fs.write_file("/mnt/rw.txt", b"new").is_ok()
        && fs.nodes[rw_index].backing.is_none()
        && fs.nodes[rw_index].length == 3
        && &fs.nodes[rw_index].data[..3] == b"new";

    ro_protected && rw_overwritten
}

fn set_scratch_backing(node: &mut Node, path: &str, length: usize) {
    let mut backing = DiskPath {
        bytes: [0; PATH_CAPACITY],
        length: path.len(),
    };
    backing.bytes[..path.len()].copy_from_slice(path.as_bytes());
    node.backing = Some(backing);
    node.length = length;
}

/// Exercise path semantics on the statically allocated scratch registry.
fn path_semantics_self_test(fs: &mut Registry) -> bool {
    if !is_nfc_path("/caf\u{e9}") || is_nfc_path("/cafe\u{301}") {
        return false;
    }
    for path in [
        "",
        "relative",
        "//home",
        "/home/",
        "/home//x",
        "/home/./x",
        "/home/../x",
        "/nul\0x",
    ] {
        if fs.mkdir(path) != Err(Error::InvalidPath)
            || fs.remove(path) != Err(Error::InvalidPath)
            || !matches!(fs.stat(path), Err(Error::InvalidPath))
            || !matches!(fs.readdir(path, 0), Err(Error::InvalidPath))
            || fs.rename(path, "/valid") != Err(Error::InvalidPath)
            || fs.rename("/mnt", path) != Err(Error::InvalidPath)
            || fs.write_file(path, b"x") != Err(Error::InvalidPath)
            || open(path) != Err(Error::InvalidPath)
            || read_all(path, &mut [0; 1]) != Err(Error::InvalidPath)
        {
            return false;
        }
    }
    for path in [
        "/home",
        "/home/anthony",
        "/home/anthony/docs",
        "/home/anthony2",
    ] {
        if fs.mkdir(path).is_err() {
            return false;
        }
    }
    let Ok(file_index) = fs.insert("/home/anthony/docs/a.txt", b"payload", true) else {
        return false;
    };
    let count = fs.count();
    if fs.remove("/home/anthony") != Err(Error::NotEmpty)
        || fs.rename("/home/anthony", "/missing/user") != Err(Error::NotFound)
        || fs.rename("/home/anthony", "/mnt/test.txt/user") != Err(Error::NotFound)
        || fs.rename("/home/anthony", "/home/anthony/docs/user") != Err(Error::InvalidPath)
        || fs.rename("/home/anthony", "/home/anthony2") != Err(Error::AlreadyExists)
        || fs.rename("/", "/root") != Err(Error::ReadOnly)
        || fs.rename("/home", "/") != Err(Error::ReadOnly)
        || fs.rename("/missing", "/missing") != Err(Error::NotFound)
        || fs.rename("/home/anthony", "/home/anthony").is_err()
        || !fs.nodes[file_index].matches("/home/anthony/docs/a.txt")
        || fs.count() != count
    {
        return false;
    }
    if fs.rename("/home/anthony", "/home/user").is_err()
        || fs.stat("/home/anthony").is_ok()
        || fs.stat("/home/anthony/docs").is_ok()
        || fs.stat("/home/anthony/docs/a.txt").is_ok()
        || fs.stat("/home/user").is_err()
        || fs.stat("/home/user/docs").is_err()
        || fs.stat("/home/anthony2").is_err()
        || !fs.nodes[file_index].matches("/home/user/docs/a.txt")
        || &fs.nodes[file_index].data[..7] != b"payload"
        || !fs.nodes[file_index].writable
        || fs.count() != count
        || !matches!(fs.readdir("/home/user", 0), Ok(entry) if entry.name_str() == "docs")
        || !matches!(fs.readdir("/home/user/docs", 0), Ok(entry) if entry.name_str() == "a.txt")
    {
        return false;
    }
    // Destination itself fits, but its descendants would exceed path capacity.
    let parent = alloc::format!("/{}", "q".repeat(MAX_DIR_NAME));
    let destination = alloc::format!("{}/{}", parent, "d".repeat(MAX_DIR_NAME));
    if fs.mkdir(&parent).is_err()
        || fs.rename("/home/user", &destination) != Err(Error::InvalidPath)
        || fs.stat(&destination).is_ok()
        || fs.stat("/home/user/docs").is_err()
        || !fs.nodes[file_index].matches("/home/user/docs/a.txt")
    {
        return false;
    }
    // Exactly PATH_CAPACITY bytes is valid for both insertion and rename.
    let parent = alloc::format!("/{}", "p".repeat(MAX_DIR_NAME));
    let child_len = PATH_CAPACITY - parent.len() - 1;
    let boundary = alloc::format!("{}/{}", parent, "a".repeat(child_len));
    let renamed = alloc::format!("{}/{}", parent, "b".repeat(child_len));
    let oversized = alloc::format!("{}/{}", parent, "c".repeat(child_len + 1));
    let long_component = alloc::format!("/{}", "x".repeat(MAX_DIR_NAME + 1));
    if fs.mkdir(&parent).is_err()
        || fs.mkdir(&boundary).is_err()
        || fs.rename(&boundary, &renamed).is_err()
        || fs.mkdir(&oversized) != Err(Error::InvalidPath)
        || fs.mkdir(&long_component) != Err(Error::InvalidPath)
        || fs.rename(&renamed, &oversized) != Err(Error::InvalidPath)
        || fs.stat(&renamed).is_err()
    {
        return false;
    }
    // Move to another parent, then shorten the prefix, retaining file identity.
    fs.rename("/home/user/docs", "/mnt/docs").is_ok()
        && fs.rename("/mnt/docs/a.txt", "/mnt/docs/b.txt").is_ok()
        && fs.rename("/mnt/docs", "/d").is_ok()
        && fs.nodes[file_index].matches("/d/b.txt")
        && fs.remove("/d") == Err(Error::NotEmpty)
        && fs.remove("/d/b.txt").is_ok()
        && fs.remove("/d").is_ok()
        && fs.remove("/home/user").is_ok()
}
