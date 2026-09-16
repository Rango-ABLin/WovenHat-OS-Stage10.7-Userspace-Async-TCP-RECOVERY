//! Physical file pages shared by mappings; cache ownership participates in COW refs.
use crate::{paging, vfs};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
const CAPACITY: usize = 64;
#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    node: usize,
    generation: u64,
    version: u64,
    offset: usize,
    count: usize,
    shared: bool,
    snapshot: u64,
}
#[derive(Clone, Copy)]
struct Entry {
    key: Key,
    frame: u64,
    age: u64,
}
static SNAPSHOTS: AtomicU64 = AtomicU64::new(1);
static CACHE: Mutex<[Option<Entry>; CAPACITY]> = Mutex::new([None; CAPACITY]);

pub fn reclaim_unused() -> usize {
    let mut cache = CACHE.lock();
    let mut released = 0;
    for slot in cache.iter_mut() {
        if let Some(entry) = *slot {
            if paging::file_frame_references(entry.frame) == 1
                && paging::release_file_frame(entry.frame)
            {
                *slot = None;
                released += 1;
            }
        }
    }
    released
}

pub fn map(
    space: paging::AddressSpace,
    address: u64,
    file: vfs::OpenFileId,
    generation: u64,
    offset: usize,
    count: usize,
    shared: bool,
) -> bool {
    let Ok((node, actual_generation, version)) = vfs::file_identity(file) else {
        return false;
    };
    if generation != actual_generation {
        return false;
    }
    let changing = !shared
        && CACHE
            .lock()
            .iter()
            .flatten()
            .any(|e| e.key.shared && e.key.node == node && e.key.generation == generation);
    let key = Key {
        node,
        generation,
        version: if shared { 0 } else { version },
        offset,
        count,
        shared,
        snapshot: if changing {
            SNAPSHOTS.fetch_add(1, Ordering::Relaxed)
        } else {
            0
        },
    };
    {
        let mut cache = CACHE.lock();
        for entry in cache.iter_mut().flatten() {
            entry.age = entry.age.saturating_add(1);
        }
        if let Some(entry) = cache
            .iter_mut()
            .flatten()
            .find(|entry| same_page(entry.key, key))
        {
            entry.age = 0;
            return paging::map_file_frame(space, address, entry.frame, shared);
        }
    }
    // No cache or process lock is held during file I/O.
    let mut bytes = [0; 4096];
    if crate::task::file_fault_io(|| {
        vfs::read_mapping_at(file, generation, offset, &mut bytes[..count])
    }) != Ok(count)
    {
        return false;
    }
    let mut cache = CACHE.lock();
    // Recheck after I/O before publishing, allowing a future concurrent loader.
    if let Some(entry) = cache
        .iter_mut()
        .flatten()
        .find(|entry| same_page(entry.key, key))
    {
        entry.age = 0;
        return paging::map_file_frame(space, address, entry.frame, shared);
    }
    let index = cache.iter().position(Option::is_none).or_else(|| {
        cache
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.is_some_and(|e| paging::file_frame_references(e.frame) == 1))
            .max_by_key(|(_, entry)| entry.unwrap().age)
            .map(|(index, _)| index)
    });
    let Some(index) = index else {
        return false;
    };
    if let Some(old) = cache[index].take() {
        if !paging::release_file_frame(old.frame) {
            return false;
        }
    }
    let frame = match paging::allocate_file_frame(&bytes) {
        Some(frame) => frame,
        None => {
            // Physical pressure: release unpinned cache-owned pages and retry.
            for slot in cache.iter_mut() {
                if slot.is_some_and(|e| paging::file_frame_references(e.frame) == 1) {
                    let entry = slot.take().unwrap();
                    let _ = paging::release_file_frame(entry.frame);
                }
            }
            drop(cache);
            let frame = paging::allocate_file_frame(&bytes).or_else(|| {
                if crate::task::evict_one_mapped_file_page() {
                    paging::allocate_file_frame(&bytes)
                } else {
                    None
                }
            });
            let Some(frame) = frame else {
                return false;
            };
            cache = CACHE.lock();
            frame
        }
    };
    if !paging::map_file_frame(space, address, frame, shared) {
        let _ = paging::release_file_frame(frame);
        return false;
    }
    cache[index] = Some(Entry { key, frame, age: 0 });
    true
}

pub fn overlay(node: usize, generation: u64, offset: usize, bytes: &mut [u8]) {
    let cache = CACHE.lock();
    for entry in cache
        .iter()
        .flatten()
        .filter(|e| e.key.shared && e.key.node == node && e.key.generation == generation)
    {
        let start = offset.max(entry.key.offset);
        let end = (offset + bytes.len()).min(entry.key.offset + entry.key.count);
        if start < end {
            paging::read_file_frame(
                entry.frame,
                start - entry.key.offset,
                &mut bytes[start - offset..end - offset],
            );
        }
    }
}
pub fn update(node: usize, generation: u64, offset: usize, bytes: &[u8]) {
    let mut cache = CACHE.lock();
    for entry in cache
        .iter_mut()
        .flatten()
        .filter(|e| e.key.shared && e.key.node == node && e.key.generation == generation)
    {
        if offset < entry.key.offset + 4096 && offset + bytes.len() > entry.key.offset {
            entry.key.count = entry.key.count.max(
                (offset + bytes.len())
                    .saturating_sub(entry.key.offset)
                    .min(4096),
            );
        }
        let start = offset.max(entry.key.offset);
        let end = (offset + bytes.len()).min(entry.key.offset + entry.key.count);
        if start < end {
            paging::write_file_frame(
                entry.frame,
                start - entry.key.offset,
                &bytes[start - offset..end - offset],
            );
        }
    }
}
pub fn replace(node: usize, generation: u64, bytes: &[u8]) {
    let mut cache = CACHE.lock();
    for entry in cache
        .iter_mut()
        .flatten()
        .filter(|e| e.key.shared && e.key.node == node && e.key.generation == generation)
    {
        let mut page = [0; 4096];
        let count = bytes.len().saturating_sub(entry.key.offset).min(4096);
        entry.key.count = count;
        if count > 0 {
            page[..count].copy_from_slice(&bytes[entry.key.offset..entry.key.offset + count]);
        }
        paging::write_file_frame(entry.frame, 0, &page);
    }
}

fn same_page(a: Key, b: Key) -> bool {
    a.node == b.node
        && a.generation == b.generation
        && a.version == b.version
        && a.offset == b.offset
        && a.shared == b.shared
        && a.snapshot == b.snapshot
        && (a.shared || a.count == b.count)
}

pub fn self_test() -> bool {
    const PATH: &str = "/tmp/frame-cache-test";
    const BASE: u64 = 0x4000_000e_0000;
    let frames = crate::memory::stats().allocated_frames;
    let descriptors = vfs::open_file_description_count();
    if vfs::stat(PATH).is_ok() || vfs::write_file(PATH, &[0; 4096]).is_err() {
        return false;
    }
    let Ok(file) = vfs::open(PATH) else {
        return false;
    };
    let Ok(generation) = vfs::file_generation(file) else {
        return false;
    };
    let Some(root) = paging::create_user_address_space(BASE) else {
        return false;
    };
    if paging::map_user_range_in(root, BASE + 8192, 4096, false, false).is_err() {
        return false;
    }
    let mut passed = map(root, BASE, file, generation, 0, 4096, false);
    let pinned = paging::user_frame_in(root, BASE);
    for version in 1..=CAPACITY {
        passed &= vfs::write_file(PATH, &[version as u8; 4096]).is_ok();
        passed &= map(root, BASE + 4096, file, generation, 0, 4096, false);
        let mut byte = [0];
        passed &= paging::read_user_bytes_in(root, BASE + 4096, &mut byte).is_ok()
            && byte == [version as u8];
        passed &= paging::unmap_user_range_in(root, BASE + 4096, 4096).is_ok();
    }
    let mut byte = [9];
    passed &= CACHE.lock().iter().flatten().count() == CAPACITY
        && paging::user_frame_in(root, BASE) == pinned
        && paging::read_user_bytes_in(root, BASE, &mut byte).is_ok()
        && byte == [0];
    passed &= reclaim_unused() == CAPACITY - 1;
    passed &= paging::unmap_user_range_in(root, BASE, 4096).is_ok() && reclaim_unused() == 1;
    passed &= paging::destroy_user_address_space(root, &[(BASE + 8192, 4096)]).is_ok();
    passed &= vfs::close_open_file(file).is_ok() && vfs::remove(PATH).is_ok();
    passed
        && crate::memory::stats().allocated_frames == frames
        && vfs::open_file_description_count() == descriptors
}
