#![allow(dead_code)]
#[path = "../kernel/src/block.rs"]
mod block;
#[path = "../kernel/src/fat32.rs"]
mod fat32;
#[path = "../kernel/src/page_cache.rs"]
mod page_cache;

#[test]
fn fat32_pages_cross_boundaries_and_reach_64k_eof() {
    use block::{BlockDevice, Error, SECTOR_SIZE};
    struct Disk {
        reads: usize,
    }
    impl BlockDevice for Disk {
        fn sector_count(&self) -> u64 {
            70000
        }
        fn read_sector(&mut self, lba: u64, out: &mut [u8]) -> Result<(), Error> {
            self.reads += 1;
            out.fill(0);
            if lba == 0 {
                out[11..13].copy_from_slice(&512u16.to_le_bytes());
                out[13] = 1;
                out[14..16].copy_from_slice(&32u16.to_le_bytes());
                out[16] = 2;
                out[32..36].copy_from_slice(&70000u32.to_le_bytes());
                out[36..40].copy_from_slice(&600u32.to_le_bytes());
                out[44..48].copy_from_slice(&2u32.to_le_bytes());
                out[510] = 0x55;
                out[511] = 0xaa;
            } else if (32..632).contains(&lba) {
                for i in 0..128 {
                    let cluster = ((lba - 32) * 128 + i) as u32;
                    let next = if (3..130).contains(&cluster) {
                        cluster + 1
                    } else {
                        0x0fffffff
                    };
                    let offset = i as usize * 4;
                    out[offset..offset + 4].copy_from_slice(&next.to_le_bytes());
                }
            } else if (1233..1361).contains(&lba) {
                for (i, byte) in out.iter_mut().enumerate() {
                    *byte = (((lba - 1233) as usize * SECTOR_SIZE + i) % 251) as u8;
                }
            } else {
                return Err(Error::OutOfBounds);
            }
            Ok(())
        }
        fn write_sector(&mut self, _: u64, _: &[u8]) -> Result<(), Error> {
            Err(Error::ReadOnly)
        }
    }
    let mut disk = Disk { reads: 0 };
    let volume = fat32::mount(&mut disk).ok().unwrap();
    let entry = fat32::DirectoryEntry {
        short_name: *b"LARGE   BIN",
        first_cluster: 3,
        size: 65536,
        attributes: 0x20,
    };
    let mut cache = page_cache::PageCache::<3>::new();
    let mut output = [0; 4109];
    for iteration in 0..2 {
        let before = disk.reads;
        assert_eq!(
            cache
                .read("large", 32761, &mut output, |offset, page| {
                    fat32::read_file_at(&mut disk, volume, entry, offset, page)
                        .map_err(|_| Error::DeviceFault)
                })
                .ok(),
            Some(4109)
        );
        for (i, byte) in output.iter().enumerate() {
            assert_eq!(*byte, ((32761 + i) % 251) as u8);
        }
        if iteration == 1 {
            assert_eq!(disk.reads, before);
        }
    }
    assert_eq!(
        cache
            .read("large", 65530, &mut output, |offset, page| {
                fat32::read_file_at(&mut disk, volume, entry, offset, page)
                    .map_err(|_| Error::DeviceFault)
            })
            .ok(),
        Some(6)
    );
    for (i, byte) in output[..6].iter().enumerate() {
        assert_eq!(*byte, ((65530 + i) % 251) as u8);
    }
}
