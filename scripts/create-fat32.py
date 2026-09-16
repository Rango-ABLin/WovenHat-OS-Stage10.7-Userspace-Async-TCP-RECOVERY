"""Create a FAT32 data image for WovenHat; never overwrite an existing disk."""
import pathlib
import struct
import sys

path = pathlib.Path(sys.argv[1])
if path.exists():
    print(f"Keeping existing disk: {path}")
    sys.exit(0)

sector_size = 512
total_sectors = 70000
swap_sectors = 32 * 8
reserved = 32
fat_sectors = 600
first_data = reserved + 2 * fat_sectors
clusters = total_sectors - first_data
boot = bytearray(sector_size)
boot[:3] = b"\xeb\x58\x90"
boot[3:11] = b"WOVENHAT"
struct.pack_into("<H", boot, 11, sector_size)
boot[13] = 1
struct.pack_into("<H", boot, 14, reserved)
boot[16] = 2
boot[21] = 0xF8
struct.pack_into("<H", boot, 24, 63)
struct.pack_into("<H", boot, 26, 255)
struct.pack_into("<I", boot, 32, total_sectors)
struct.pack_into("<I", boot, 36, fat_sectors)
struct.pack_into("<I", boot, 44, 2)
struct.pack_into("<H", boot, 48, 1)
struct.pack_into("<H", boot, 50, 6)
boot[64] = 0x80
boot[66] = 0x29
struct.pack_into("<I", boot, 67, 0x574F564E)
boot[71:82] = b"WOVENHAT   "
boot[82:90] = b"FAT32   "
boot[510:512] = b"\x55\xaa"
info = bytearray(sector_size)
struct.pack_into("<I", info, 0, 0x41615252)
struct.pack_into("<I", info, 484, 0x61417272)
struct.pack_into("<I", info, 488, clusters - 1)
struct.pack_into("<I", info, 492, 3)
struct.pack_into("<I", info, 508, 0xAA550000)
fat = bytearray(sector_size)
struct.pack_into("<III", fat, 0, 0x0FFFFFF8, 0xFFFFFFFF, 0x0FFFFFFF)
# Exclusive creation prevents accidental replacement, including concurrent runs.
with path.open("xb") as disk:
    disk.truncate((total_sectors + swap_sectors) * sector_size)
    for lba, data in ((0, boot), (1, info), (6, boot), (7, info),
                      (reserved, fat), (reserved + fat_sectors, fat)):
        disk.seek(lba * sector_size)
        disk.write(data)
print(f"Created FAT32 disk: {path} ({total_sectors * sector_size} FAT32 bytes + {swap_sectors * sector_size} swap bytes)")
