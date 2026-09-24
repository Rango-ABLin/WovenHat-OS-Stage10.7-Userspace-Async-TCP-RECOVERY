#[path = "../kernel/src/wifi_tlv.rs"]
pub mod wifi_tlv;
use wifi_tlv::*;

fn image() -> Vec<u8> {
    let mut bytes = vec![0; INTEL_TLV_UCODE_HEADER_SIZE];
    bytes[4..8].copy_from_slice(&INTEL_TLV_UCODE_MAGIC.to_le_bytes());
    bytes
}
fn record(bytes: &mut Vec<u8>, kind: u32, payload: &[u8]) {
    bytes.extend(kind.to_le_bytes());
    bytes.extend((payload.len() as u32).to_le_bytes());
    bytes.extend(payload);
    while !bytes.len().is_multiple_of(4) {
        bytes.push(0);
    }
}
fn section(bytes: &mut Vec<u8>, kind: u32, offset: u32, data: &[u8]) {
    let mut payload = offset.to_le_bytes().to_vec();
    payload.extend(data);
    record(bytes, kind, &payload);
}
fn error(bytes: &[u8], expected: IntelTlvError) {
    assert!(matches!(IntelTlvFirmware::parse(bytes), Err(actual) if actual == expected));
}

#[test]
fn paging_metadata_is_not_downloadable_code() {
    let mut bytes = image();
    section(&mut bytes, INTEL_TLV_SEC_RT, 0x1000, &[1, 2, 3]);
    record(&mut bytes, INTEL_TLV_PAGING, &0x8f000u32.to_le_bytes());
    let fw = IntelTlvFirmware::parse(&bytes).unwrap();
    assert_eq!(fw.section_count(), 1);
    assert_eq!(fw.paging_size(), Some(0x8f000));
    assert_eq!(fw.section(0).unwrap().bytes, &[1, 2, 3]);
    assert!(fw.section(1).is_none());
    assert!(fw.section(usize::MAX).is_none());
}

#[test]
fn paging_size_and_duplicate_metadata_fail_closed() {
    for size in [1, 4095, INTEL_MAX_PAGING_SIZE + 4096, u32::MAX] {
        let mut bytes = image();
        record(&mut bytes, INTEL_TLV_PAGING, &size.to_le_bytes());
        error(&bytes, IntelTlvError::InvalidPagingSize);
    }
    for payload in [&[][..], &[0, 0, 0][..], &[0; 8][..]] {
        let mut bytes = image();
        record(&mut bytes, INTEL_TLV_PAGING, payload);
        error(&bytes, IntelTlvError::InvalidPagingSize);
    }
    let mut bytes = image();
    record(&mut bytes, INTEL_TLV_PAGING, &4096u32.to_le_bytes());
    record(&mut bytes, INTEL_TLV_PAGING, &4096u32.to_le_bytes());
    error(&bytes, IntelTlvError::DuplicatePaging);
}

#[test]
fn secure_sections_and_separator_order_are_preserved() {
    let mut bytes = image();
    section(&mut bytes, INTEL_TLV_SECURE_SEC_RT, 0x1000, &[7]);
    section(
        &mut bytes,
        INTEL_TLV_SECURE_SEC_RT,
        INTEL_CPU_SEPARATOR,
        &[],
    );
    section(&mut bytes, INTEL_TLV_SEC_RT, 0xc000_0000, &[8]);
    section(
        &mut bytes,
        INTEL_TLV_SEC_RT,
        INTEL_PAGING_SEPARATOR,
        &[0; 4],
    );
    section(&mut bytes, INTEL_TLV_SECURE_SEC_INIT, 0x2000, &[9]);
    let fw = IntelTlvFirmware::parse(&bytes).unwrap();
    let sections: Vec<_> = fw.sections().collect();
    assert_eq!(sections.len(), 5);
    assert!(!sections[0].is_separator());
    assert!(sections[1].is_separator());
    assert!(sections[3].is_separator());
    assert_eq!(sections[4].image, IntelFirmwareImageKind::Init);
    assert_eq!(sections[2].bytes, &[8]);
}

#[test]
fn real_sized_container_streams_more_than_sixteen_sections() {
    let mut bytes = image();
    for index in 0..50u32 {
        section(
            &mut bytes,
            INTEL_TLV_SEC_RT,
            index * 32768,
            &[index as u8; 16],
        );
    }
    let fw = IntelTlvFirmware::parse(&bytes).unwrap();
    assert_eq!(fw.section_count(), 50);
    assert_eq!(fw.sections().count(), 50);
    for (index, section) in fw.sections().enumerate() {
        assert_eq!(section.device_offset, index as u32 * 32768);
        assert_eq!(section.bytes, &[index as u8; 16]);
    }
    // Descriptor storage must not grow with section count on the kernel stack.
    assert!(std::mem::size_of::<IntelTlvFirmware<'_>>() < 128);
}

#[test]
fn malformed_sections_and_missing_padding_are_rejected() {
    let mut bytes = image();
    section(&mut bytes, INTEL_TLV_SEC_RT, 0x2000, &[]);
    error(&bytes, IntelTlvError::EmptySection);
    let mut bytes = image();
    record(&mut bytes, INTEL_TLV_SEC_RT, &[0; 3]);
    error(&bytes, IntelTlvError::SectionTooShort);
    let mut bytes = image();
    record(&mut bytes, 0xffff, &[1, 2, 3]);
    bytes.pop();
    error(&bytes, IntelTlvError::InvalidPadding);
}

#[test]
fn truncated_and_oversized_containers_are_rejected() {
    for size in 0..INTEL_TLV_UCODE_HEADER_SIZE {
        error(&vec![0; size], IntelTlvError::TooShort);
    }
    let mut bytes = image();
    bytes.extend([1; 7]);
    error(&bytes, IntelTlvError::TruncatedTlv);
    let mut bytes = image();
    bytes.extend(INTEL_TLV_SEC_RT.to_le_bytes());
    bytes.extend(u32::MAX.to_le_bytes());
    error(&bytes, IntelTlvError::TruncatedTlv);
    let mut bytes = image();
    bytes.resize(INTEL_MAX_IMAGE_SIZE + 1, 0);
    error(&bytes, IntelTlvError::ImageTooLarge);
}

#[test]
fn header_and_unknown_records_preserve_forward_compatibility() {
    let mut bytes = image();
    bytes[72..76].copy_from_slice(&77u32.to_le_bytes());
    bytes[76..80].copy_from_slice(&3u32.to_le_bytes());
    record(&mut bytes, 0xff00, &[1, 2, 3]);
    let fw = IntelTlvFirmware::parse(&bytes).unwrap();
    assert_eq!(fw.version(), 77);
    assert_eq!(fw.build(), 3);
    assert_eq!(fw.bytes(), bytes);
    assert_eq!(fw.paging_size(), None);
    bytes[0] = 1;
    error(&bytes, IntelTlvError::BadZeroPrefix);
    bytes[0] = 0;
    bytes[4] = 0;
    error(&bytes, IntelTlvError::BadMagic);
}
