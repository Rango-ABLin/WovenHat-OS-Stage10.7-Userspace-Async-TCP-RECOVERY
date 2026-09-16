# Partition-table discovery

WovenHat discovers FAT32 volumes in this order:

1. validate a FAT32 superfloppy beginning at device LBA 0;
2. inspect the four primary MBR entries for types 0x0B and 0x0C; and
3. when a protective MBR is present, validate the primary GPT and inspect its entries.

MBR entries require a valid boot marker, a supported boot flag, nonzero start and
length, overflow-free arithmetic, and an end within the block device.

GPT discovery requires the `EFI PART` signature, revision 1.0, a 92–512 byte header,
a valid header CRC32, a protective MBR entry starting at LBA 1, usable-LBA bounds, and
an entry array contained by the device. The entry count is capped at 128 and entry size
at 256 bytes. The complete declared entry array is CRC32-checked before any partition
is returned. EFI System and Microsoft Basic Data partitions are considered FAT-capable;
the FAT32 loader remains the final format validator.

Both schemes produce the same partition-relative block-device wrapper. It translates
logical LBAs with checked addition and rejects reads or writes at or beyond the
partition length before forwarding them to the underlying driver.

Deterministic boot fixtures validate MBR translation and bounds, a valid GPT image,
partition selection, and rejection after header corruption.

Current limitations: extended/logical MBR partitions, the backup GPT header, partition
names and attributes, hybrid MBR policy, and GUID-specific driver binding are not yet
implemented.
