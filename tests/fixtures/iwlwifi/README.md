# AX200 container fixture

`iwlwifi-cc-a0-77.ucode` is an unmodified upstream Intel binary, used only by
host parser tests. It is not embedded in the kernel or activated on hardware.
Redistribution terms and copyright are reproduced in `LICENCE.iwlwifi_firmware`.

- Repository: https://kernel.googlesource.com/pub/scm/linux/kernel/git/iwlwifi/linux-firmware/
- Commit: `85ea23f877042c8e7d4c3b8ac131a946c8f42703`
- Upstream path: `intel/iwlwifi/iwlwifi-cc-a0-77.ucode`
- Size: 1,368,100 bytes
- SHA-256: `94f5fb915f2074f98059365e11cca313e25a6fa3b7cd6772e80b3ba5edfb966b`
- License path at that commit: `LICENSES/LICENCE.iwlwifi_firmware`

Tests inspect the documented TLV container, not firmware instructions. The
observed header is version 77/build `0x8dbafb52`. There are 48 runtime data
sections: 14 LMAC, 15 UMAC and 19 paging, separated by two marker records.
Paging metadata declares `0x8f000` bytes. Parsing this fixture does not establish
firmware ABI compatibility, signature verification or successful device boot.
