#[path = "../kernel/src/wifi_firmware_tlv.rs"]
mod tlv;
use tlv::*;

fn header() -> Vec<u8> {
    let mut bytes = vec![0; INTEL_TLV_UCODE_HEADER_SIZE];
    bytes[4..8].copy_from_slice(&INTEL_TLV_UCODE_MAGIC.to_le_bytes());
    bytes
}

fn record(bytes: &mut Vec<u8>, kind: u32, payload: &[u8]) {
    bytes.extend_from_slice(&kind.to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes.resize(bytes.len().next_multiple_of(4), 0);
}

fn section(bytes: &mut Vec<u8>, kind: u32, address: u32, payload: &[u8]) {
    let mut data = address.to_le_bytes().to_vec();
    data.extend_from_slice(payload);
    record(bytes, kind, &data);
}

fn error(bytes: &[u8], expected: IntelTlvError) {
    assert_eq!(IntelTlvFirmware::parse(bytes).err(), Some(expected));
}

#[test]
fn upstream_ax200_image_preserves_all_groups_without_staging_markers() {
    let bytes = include_bytes!("fixtures/iwlwifi/iwlwifi-cc-a0-77.ucode");
    let fw = IntelTlvFirmware::parse(bytes).unwrap();
    assert_eq!(fw.bytes(), bytes);
    assert_eq!((fw.version(), fw.build()), (77, 0x8dba_fb52));
    assert_eq!(fw.paging_size(), Some(0x8f000));
    assert_eq!(fw.section_count(), 48);
    let mut counts = [0; 3];
    for i in 0..fw.section_count() {
        let section = fw.section(i).unwrap();
        assert_eq!(section.image, IntelFirmwareImageKind::Runtime);
        assert!(!section.bytes.is_empty());
        assert!(!matches!(
            section.device_offset,
            INTEL_CPU_SEPARATOR | INTEL_PAGING_SEPARATOR
        ));
        let group = match fw.section_group(i).unwrap() {
            IntelFirmwareSectionGroup::Lmac => 0,
            IntelFirmwareSectionGroup::Umac => 1,
            IntelFirmwareSectionGroup::Paging => 2,
        };
        counts[group] += 1;
    }
    assert_eq!(counts, [14, 15, 19]);
    assert_eq!(fw.section(48), None);
    assert_eq!(fw.section_group(usize::MAX), None);
    // The generic image contract is deliberately unchanged.
    assert_eq!(MAX_FIRMWARE_SECTIONS, 16);
}

#[test]
fn secure_and_plain_sections_keep_independent_image_groups() {
    let mut bytes = header();
    section(&mut bytes, INTEL_TLV_SECURE_SEC_RT, 0x1000, &[1]);
    section(&mut bytes, INTEL_TLV_SECURE_SEC_INIT, 0x2000, &[2]);
    section(&mut bytes, INTEL_TLV_SEC_RT, INTEL_CPU_SEPARATOR, &[]);
    section(&mut bytes, INTEL_TLV_SEC_INIT, 0x3000, &[3]);
    section(&mut bytes, INTEL_TLV_SECURE_SEC_RT, 0x4000, &[4]);
    let fw = IntelTlvFirmware::parse(&bytes).unwrap();
    assert_eq!(fw.section_count(), 4);
    assert_eq!(fw.section(2).unwrap().image, IntelFirmwareImageKind::Init);
    assert_eq!(fw.section_group(2), Some(IntelFirmwareSectionGroup::Lmac));
    assert_eq!(fw.section_group(3), Some(IntelFirmwareSectionGroup::Umac));
    assert_eq!(fw.paging_size(), None);
}

#[test]
fn paging_is_exact_bounded_metadata_not_a_section() {
    for size in [0u32, 4096, 1024 * 1024] {
        let mut bytes = header();
        record(&mut bytes, INTEL_TLV_PAGING, &size.to_le_bytes());
        section(&mut bytes, INTEL_TLV_SEC_RT, 0, &[1]);
        let fw = IntelTlvFirmware::parse(&bytes).unwrap();
        assert_eq!(fw.section_count(), 1);
        assert_eq!(fw.paging_size(), Some(size));
        record(&mut bytes, INTEL_TLV_PAGING, &size.to_le_bytes());
        error(&bytes, IntelTlvError::DuplicatePagingSize);
    }
    for payload in [
        vec![],
        vec![0; 3],
        vec![0; 8],
        4097u32.to_le_bytes().to_vec(),
        0x101000u32.to_le_bytes().to_vec(),
    ] {
        let mut bytes = header();
        record(&mut bytes, INTEL_TLV_PAGING, &payload);
        error(&bytes, IntelTlvError::InvalidPagingSize);
    }
}

#[test]
fn malformed_delimiters_cannot_drop_data_or_change_groups() {
    for delimiter in [INTEL_CPU_SEPARATOR, INTEL_PAGING_SEPARATOR] {
        let mut bytes = header();
        section(&mut bytes, INTEL_TLV_SEC_RT, delimiter, &[]);
        error(&bytes, IntelTlvError::InvalidSeparator);
    }
    for payload in [&[][..], &[0, 0, 0, 0][..], &[1][..]] {
        let mut bytes = header();
        section(&mut bytes, INTEL_TLV_SEC_RT, 0x1000, &[1]);
        section(&mut bytes, INTEL_TLV_SEC_RT, INTEL_CPU_SEPARATOR, payload);
        error(&bytes, IntelTlvError::InvalidSeparator); // dangling or malformed
    }
    let mut bytes = header();
    section(&mut bytes, INTEL_TLV_SEC_RT, 0x1000, &[1]);
    section(&mut bytes, INTEL_TLV_SEC_RT, INTEL_CPU_SEPARATOR, &[]);
    section(&mut bytes, INTEL_TLV_SEC_RT, 0x2000, &[2]);
    section(&mut bytes, INTEL_TLV_SEC_RT, INTEL_CPU_SEPARATOR, &[]);
    error(&bytes, IntelTlvError::InvalidSeparator);
}

#[test]
fn input_bounds_and_device_range_overflow_fail_closed() {
    error(
        &vec![0; MAX_FIRMWARE_IMAGE_SIZE + 1],
        IntelTlvError::ImageTooLarge,
    );
    let mut bytes = header();
    section(
        &mut bytes,
        INTEL_TLV_SEC_RT,
        0,
        &vec![0; MAX_FIRMWARE_SECTION_SIZE + 1],
    );
    error(&bytes, IntelTlvError::SectionTooLarge);
    let mut bytes = header();
    section(&mut bytes, INTEL_TLV_SEC_RT, u32::MAX, &[1]);
    error(&bytes, IntelTlvError::AddressOverflow);
    let mut bytes = header();
    for _ in 0..MAX_INTEL_TLV_SECTIONS {
        section(&mut bytes, INTEL_TLV_SEC_RT, 0, &[1]);
    }
    assert_eq!(
        IntelTlvFirmware::parse(&bytes).unwrap().section_count(),
        MAX_INTEL_TLV_SECTIONS
    );
    section(&mut bytes, INTEL_TLV_SEC_RT, 0, &[1]);
    error(&bytes, IntelTlvError::TooManySections);
}

#[test]
fn truncation_empty_sections_and_unknown_records_are_checked() {
    error(&header(), IntelTlvError::NoSections);
    let mut bytes = header();
    section(&mut bytes, INTEL_TLV_SEC_RT, 0, &[]);
    error(&bytes, IntelTlvError::EmptySection);
    let mut bytes = header();
    record(&mut bytes, INTEL_TLV_SECURE_SEC_INIT, &[1, 2, 3]);
    error(&bytes, IntelTlvError::SectionTooShort);
    let mut bytes = header();
    record(&mut bytes, 0xfeed, &[1, 2, 3]);
    section(&mut bytes, INTEL_TLV_SEC_RT, 0, &[1]);
    for length in 0..bytes.len() {
        assert!(
            IntelTlvFirmware::parse(&bytes[..length]).is_err(),
            "length {length}"
        );
    }
    assert!(IntelTlvFirmware::parse(&bytes).is_ok());
    bytes[92..96].copy_from_slice(&u32::MAX.to_le_bytes());
    error(&bytes, IntelTlvError::TruncatedTlv);
}

#[test]
fn bad_header_and_missing_alignment_padding_are_rejected() {
    let mut bytes = header();
    bytes[0] = 1;
    error(&bytes, IntelTlvError::BadZeroPrefix);
    bytes[0] = 0;
    bytes[4] = 0;
    error(&bytes, IntelTlvError::BadMagic);
    let mut bytes = header();
    record(&mut bytes, 0xbeef, &[1]);
    bytes.pop();
    error(&bytes, IntelTlvError::InvalidPadding);
}
