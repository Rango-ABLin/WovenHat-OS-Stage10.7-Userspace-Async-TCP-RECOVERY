// Standalone host harness: rustc --test tests/buffer_cache.rs -o target/buffer-cache-tests.exe
#![allow(dead_code)]
#[path = "../kernel/src/block.rs"]
mod block;
#[path = "../kernel/src/block_cache.rs"]
mod block_cache;
#[path = "../kernel/src/partition.rs"]
mod partition;

#[test]
fn partition_flush_reaches_cache_and_device() {
    use block::BlockDevice;
    let mut disk = block::RamDisk::<4>::new();
    {
        let mut cache = block_cache::CachedDevice::<_, 2>::new(&mut disk);
        {
            let part = partition::Partition {
                start_lba: 1,
                sectors: 2,
                kind: 0x0b,
            };
            let mut view = partition::PartitionDevice::new(&mut cache, part)
                .ok()
                .unwrap();
            assert!(view.write_sector(0, &[4; block::SECTOR_SIZE]).is_ok());
            assert!(view.flush().is_ok());
        }
        assert_eq!(cache.stats().dirty, 0);
    }
    let mut out = [0; block::SECTOR_SIZE];
    assert!(disk.read_sector(1, &mut out).is_ok());
    assert_eq!(out, [4; block::SECTOR_SIZE]);
}
