//! EDID parsing as an outside consumer sees it: the dock's own bytes, damaged copies and
//! hostile descriptors.

use screendockd::protocol::edid::{Edid, EdidError, EdidMode, Millimetres, RefreshMilliHz};

const DOCK_EDID: &[u8] = include_bytes!("../fixtures/dock_edid.bin");

fn mode(width: u16, height: u16, hertz: u32) -> EdidMode {
    EdidMode {
        width,
        height,
        refresh: RefreshMilliHz(hertz * 1000),
    }
}

fn fix_checksum(block: &mut [u8]) {
    block[127] = 0;
    let sum = block.iter().fold(0u8, |s, b| s.wrapping_add(*b));
    block[127] = 0u8.wrapping_sub(sum);
}

/// A valid single block with a name descriptor holding `name` and no timings.
fn block_with_name(name: &[u8]) -> Vec<u8> {
    let mut block = vec![0u8; 128];
    block[..8].copy_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
    block[18] = 1;
    block[19] = 3;
    block[54 + 3] = 0xFC;
    let text = &mut block[54 + 5..54 + 18];
    text.fill(b' ');
    text[..name.len().min(13)].copy_from_slice(&name[..name.len().min(13)]);
    fix_checksum(&mut block);
    block
}

#[test]
fn dock_edid_parses_to_its_name_native_mode_and_mode_list() {
    let edid = Edid::parse(DOCK_EDID).unwrap();
    assert_eq!(
        edid.name.as_ref().map(|n| n.as_str()),
        Some("HDMI Extender")
    );
    assert_eq!(edid.manufacturer, "UNK");
    assert_eq!(edid.version, (1, 3));
    assert_eq!(edid.preferred, Some(mode(1920, 1080, 60)));
    assert_eq!(
        edid.physical_size,
        Some((Millimetres(620), Millimetres(340)))
    );
    assert_eq!(edid.extension_blocks, 1);
    assert_eq!(edid.trailing_bytes, 2);
    assert_eq!(
        edid.modes,
        vec![
            mode(1920, 1080, 60),
            mode(1280, 720, 60),
            mode(1280, 960, 60),
            mode(1280, 800, 60),
            mode(1280, 1024, 60),
            mode(1600, 900, 60),
        ]
    );
}

#[test]
fn trailing_bytes_after_the_declared_blocks_are_ignored() {
    let mut longer = DOCK_EDID.to_vec();
    longer.extend_from_slice(&[0xAA; 50]);
    let edid = Edid::parse(&longer).unwrap();
    assert_eq!(edid.trailing_bytes, 52);
    assert_eq!(edid.modes, Edid::parse(DOCK_EDID).unwrap().modes);

    let base_only = Edid::parse(&block_with_name(b"Tiny")).unwrap();
    assert_eq!(base_only.extension_blocks, 0);
    assert_eq!(base_only.trailing_bytes, 0);
}

#[test]
fn a_corrupted_block_fails_its_own_checksum() {
    let mut bad_base = DOCK_EDID.to_vec();
    bad_base[30] ^= 0x01;
    assert_eq!(
        Edid::parse(&bad_base),
        Err(EdidError::BadChecksum { block: 0 })
    );
    let mut bad_extension = DOCK_EDID.to_vec();
    bad_extension[200] ^= 0x01;
    assert_eq!(
        Edid::parse(&bad_extension),
        Err(EdidError::BadChecksum { block: 1 })
    );
}

#[test]
fn truncated_input_is_an_error_at_every_length() {
    for len in 0..256 {
        let result = Edid::parse(&DOCK_EDID[..len]);
        let expected = if len < 128 {
            EdidError::TooShort { len }
        } else {
            EdidError::Truncated {
                declared_blocks: 2,
                len,
            }
        };
        assert_eq!(result, Err(expected), "length {len}");
    }
    assert_eq!(Edid::parse(&DOCK_EDID[..256]).unwrap().trailing_bytes, 0);
}

#[test]
fn input_without_the_header_is_rejected() {
    let mut bytes = DOCK_EDID.to_vec();
    bytes[0] = 1;
    assert_eq!(Edid::parse(&bytes), Err(EdidError::BadHeader));
    assert_eq!(Edid::parse(&[0xFF; 300]), Err(EdidError::BadHeader));
}

#[test]
fn a_declared_extension_count_beyond_the_input_is_truncated() {
    let mut block = block_with_name(b"Tiny");
    block[126] = 255;
    fix_checksum(&mut block);
    assert_eq!(
        Edid::parse(&block),
        Err(EdidError::Truncated {
            declared_blocks: 256,
            len: 128
        })
    );
}

#[test]
fn names_are_reduced_to_printable_ascii_without_path_separators() {
    let edid = Edid::parse(&block_with_name(b"../a\x1b[31mb\\")).unwrap();
    assert_eq!(edid.name.unwrap().as_str(), ".._a[31mb_");
    let control_only = Edid::parse(&block_with_name(&[0x01, 0x02, 0x7F, 0xFF])).unwrap();
    assert_eq!(control_only.name, None);
    let stops_at_newline = Edid::parse(&block_with_name(b"Dock\nEvil")).unwrap();
    assert_eq!(stops_at_newline.name.unwrap().as_str(), "Dock");
}

#[test]
fn descriptors_with_a_zero_clock_or_no_size_yield_no_timing() {
    let mut block = block_with_name(b"Tiny");
    block[54 + 18] = 0x01;
    fix_checksum(&mut block);
    let edid = Edid::parse(&block).unwrap();
    assert_eq!(edid.preferred, None);
    assert!(edid.modes.is_empty());
    assert_eq!(edid.physical_size, None);
}

#[test]
fn arbitrary_bytes_behind_a_valid_header_never_panic() {
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    for _ in 0..2000 {
        let mut block = vec![0u8; 128];
        for byte in &mut block {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            *byte = (seed >> 24) as u8;
        }
        block[..8].copy_from_slice(&[0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0]);
        block[126] = 0;
        fix_checksum(&mut block);
        let _ = Edid::parse(&block);
    }
}
