//! Bounded, demand-loaded clean file pages. Serialized by the storage layer.
use crate::block::Error;

pub const PAGE_SIZE: usize = 4096;
const KEY_SIZE: usize = 128;

struct Slot {
    key: [u8; KEY_SIZE],
    key_len: usize,
    page: usize,
    data: [u8; PAGE_SIZE],
    length: usize,
    valid: bool,
    age: u64,
}
impl Slot {
    const fn empty() -> Self {
        Self {
            key: [0; KEY_SIZE],
            key_len: 0,
            page: 0,
            data: [0; PAGE_SIZE],
            length: 0,
            valid: false,
            age: 0,
        }
    }
}
#[derive(Clone, Copy)]
pub struct Stats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub resident: usize,
    pub capacity: usize,
}
pub struct PageCache<const N: usize> {
    slots: [Slot; N],
    hits: u64,
    misses: u64,
    evictions: u64,
}
impl<const N: usize> PageCache<N> {
    pub const fn new() -> Self {
        Self {
            slots: [const { Slot::empty() }; N],
            hits: 0,
            misses: 0,
            evictions: 0,
        }
    }
    pub fn stats(&self) -> Stats {
        Stats {
            hits: self.hits,
            misses: self.misses,
            evictions: self.evictions,
            resident: self.slots.iter().filter(|s| s.valid).count(),
            capacity: N,
        }
    }
    /// All entries are clean, so reclamation requires no writeback.
    pub fn invalidate(&mut self) {
        for slot in &mut self.slots {
            slot.valid = false;
        }
    }
    pub fn read(
        &mut self,
        key: &str,
        offset: usize,
        output: &mut [u8],
        mut load: impl FnMut(usize, &mut [u8]) -> Result<usize, Error>,
    ) -> Result<usize, Error> {
        if key.is_empty()
            || key.len() > KEY_SIZE
            || N == 0
            || offset.checked_add(output.len()).is_none()
        {
            return Err(Error::InvalidBuffer);
        }
        let mut copied = 0;
        while copied < output.len() {
            let position = offset + copied;
            let page = position / PAGE_SIZE;
            let within = position % PAGE_SIZE;
            let index = if let Some(index) = self.slots.iter().position(|s| {
                s.valid
                    && s.page == page
                    && s.key_len == key.len()
                    && &s.key[..s.key_len] == key.as_bytes()
            }) {
                self.hits = self.hits.saturating_add(1);
                index
            } else {
                self.misses = self.misses.saturating_add(1);
                let index = self.slots.iter().position(|s| !s.valid).unwrap_or_else(|| {
                    self.slots
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, s)| s.age)
                        .unwrap()
                        .0
                });
                let slot = &mut self.slots[index];
                if slot.valid {
                    self.evictions = self.evictions.saturating_add(1);
                }
                slot.valid = false;
                slot.data.fill(0);
                let length = load(page * PAGE_SIZE, &mut slot.data)?;
                if length > PAGE_SIZE {
                    return Err(Error::InvalidBuffer);
                }
                slot.key[..key.len()].copy_from_slice(key.as_bytes());
                slot.key_len = key.len();
                slot.page = page;
                slot.length = length;
                slot.valid = true;
                index
            };
            for slot in self.slots.iter_mut().filter(|s| s.valid) {
                slot.age = slot.age.saturating_add(1);
            }
            let slot = &mut self.slots[index];
            slot.age = 0;
            let count = slot
                .length
                .saturating_sub(within)
                .min(output.len() - copied);
            if count == 0 {
                break;
            }
            output[copied..copied + count].copy_from_slice(&slot.data[within..within + count]);
            copied += count;
            if slot.length < PAGE_SIZE {
                break;
            }
        }
        Ok(copied)
    }
}

fn boundary_test() -> bool {
    let mut cache = PageCache::<2>::new();
    let mut loads = 0;
    let mut output = [0; 32];
    let mut load = |offset: usize, page: &mut [u8]| {
        loads += 1;
        let length = (PAGE_SIZE + 11usize).saturating_sub(offset).min(PAGE_SIZE);
        page[..length].fill(if offset == 0 { 1 } else { 2 });
        Ok(length)
    };
    if cache.read("/file", PAGE_SIZE - 8, &mut output, &mut load) != Ok(19)
        || output[..8] != [1; 8]
        || output[8..19] != [2; 11]
        || output[19..] != [0; 13]
        || cache.read("/file", PAGE_SIZE - 8, &mut output, &mut load) != Ok(19)
        || cache.read("/file", PAGE_SIZE + 11, &mut output, &mut load) != Ok(0)
    {
        return false;
    }
    loads == 2 && cache.stats().resident == 2 && cache.stats().hits == 3
}

fn eviction_and_failure_test() -> bool {
    let mut cache = PageCache::<2>::new();
    let mut output = [0; 1];
    let load = |_: usize, page: &mut [u8]| {
        page.fill(4);
        Ok(PAGE_SIZE)
    };
    if cache.read("a", 0, &mut output, load).is_err()
        || cache.read("b", 0, &mut output, load).is_err()
        || cache.read("a", 0, &mut output, load).is_err()
        || cache.read("c", 0, &mut output, load).is_err()
        || cache.stats().evictions != 1
    {
        return false;
    }
    // a survived LRU eviction; b must load again. Failed loads never become hits.
    if cache
        .read("a", 0, &mut output, |_, _| Err(Error::DeviceFault))
        .is_err()
        || cache.read("b", 0, &mut output, |_, page| {
            page.fill(99);
            Err(Error::DeviceFault)
        }) != Err(Error::DeviceFault)
        || cache.read("b", 0, &mut output, load).is_err()
        || output != [4]
    {
        return false;
    }
    cache.invalidate();
    let changed = |_: usize, page: &mut [u8]| {
        page.fill(7);
        Ok(PAGE_SIZE)
    };
    cache.stats().resident == 0
        && cache.read("a", 0, &mut output, changed).is_ok()
        && output == [7]
        && cache.read("", 0, &mut output, changed) == Err(Error::InvalidBuffer)
        && cache.read("a", usize::MAX, &mut output, changed) == Err(Error::InvalidBuffer)
        && PageCache::<0>::new().read("a", 0, &mut output, changed) == Err(Error::InvalidBuffer)
}

pub fn self_test() -> bool {
    boundary_test() && eviction_and_failure_test()
}

#[cfg(test)]
mod tests {
    #[test]
    fn cross_page_eof_and_hits() {
        assert!(super::boundary_test());
    }
    #[test]
    fn lru_failed_load_and_invalidation() {
        assert!(super::eviction_and_failure_test());
    }
}
