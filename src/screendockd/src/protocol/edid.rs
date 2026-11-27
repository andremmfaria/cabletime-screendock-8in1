//! Parsing of the dock's EDID: base block fields, monitor name, timings and the extension
//! count, with a checksum check per block.
//!
//! The bytes come from the dock and are untrusted: every read is bounds checked, strings are
//! reduced to printable ASCII, and nothing here performs I/O.

pub use super::types::{DisplayName, Edid, EdidError, EdidMode, Millimetres, RefreshMilliHz};
use super::types::{
    DESCRIPTOR_COUNT, DESCRIPTOR_LEN, DESCRIPTOR_START, EDID_BLOCK_LEN, EDID_HEADER,
    EXTENSION_COUNT_OFFSET, NAME_TAG,
};

impl DisplayName {
    /// Reduces descriptor text to printable ASCII, stopping at a line feed and replacing path
    /// separators. Returns `None` when nothing is left.
    pub fn sanitize(raw: &[u8]) -> Option<Self> {
        let end = raw.iter().position(|b| *b == b'\n').unwrap_or(raw.len());
        let text: String = raw[..end]
            .iter()
            .filter(|b| (0x20..=0x7E).contains(*b))
            .map(|b| match b {
                b'/' | b'\\' => '_',
                other => *other as char,
            })
            .collect();
        let text = text.trim();
        (!text.is_empty()).then(|| Self(text.to_string()))
    }
}

impl Edid {
    /// Parses an EDID. Bytes beyond the declared blocks are ignored.
    pub fn parse(bytes: &[u8]) -> Result<Self, EdidError> {
        if bytes.len() < EDID_BLOCK_LEN {
            return Err(EdidError::TooShort { len: bytes.len() });
        }
        if bytes[..8] != EDID_HEADER {
            return Err(EdidError::BadHeader);
        }
        if !block_sums_to_zero(&bytes[..EDID_BLOCK_LEN]) {
            return Err(EdidError::BadChecksum { block: 0 });
        }
        let extension_blocks = bytes[EXTENSION_COUNT_OFFSET];
        let declared_blocks = 1 + usize::from(extension_blocks);
        let used = declared_blocks * EDID_BLOCK_LEN;
        if bytes.len() < used {
            return Err(EdidError::Truncated {
                declared_blocks,
                len: bytes.len(),
            });
        }
        for (index, block) in bytes[..used].chunks(EDID_BLOCK_LEN).enumerate().skip(1) {
            if !block_sums_to_zero(block) {
                return Err(EdidError::BadChecksum { block: index });
            }
        }

        let base = &bytes[..EDID_BLOCK_LEN];
        let mut name = None;
        let mut detailed = Vec::new();
        for slot in 0..DESCRIPTOR_COUNT {
            let start = DESCRIPTOR_START + slot * DESCRIPTOR_LEN;
            let descriptor = &base[start..start + DESCRIPTOR_LEN];
            if descriptor[0] == 0 && descriptor[1] == 0 {
                if descriptor[3] == NAME_TAG && name.is_none() {
                    name = DisplayName::sanitize(&descriptor[5..]);
                }
            } else if let Some(mode) = detailed_timing(descriptor) {
                detailed.push(mode);
            }
        }
        let preferred = (base[DESCRIPTOR_START] != 0 || base[DESCRIPTOR_START + 1] != 0)
            .then(|| detailed_timing(&base[DESCRIPTOR_START..DESCRIPTOR_START + DESCRIPTOR_LEN]))
            .flatten();

        let mut modes: Vec<EdidMode> = Vec::new();
        let standard = base[38..54]
            .chunks(2)
            .filter_map(|pair| standard_timing(pair[0], pair[1], base[19]));
        for mode in detailed.into_iter().chain(standard) {
            if !modes.contains(&mode) {
                modes.push(mode);
            }
        }

        let physical_size = match (base[21], base[22]) {
            (0, _) | (_, 0) => None,
            (w, h) => Some((
                Millimetres(u16::from(w) * 10),
                Millimetres(u16::from(h) * 10),
            )),
        };
        Ok(Self {
            manufacturer: manufacturer_id(u16::from_be_bytes([base[8], base[9]])),
            product_code: u16::from_le_bytes([base[10], base[11]]),
            serial: u32::from_le_bytes([base[12], base[13], base[14], base[15]]),
            year: 1990 + u16::from(base[17]),
            version: (base[18], base[19]),
            physical_size,
            name,
            preferred,
            modes,
            extension_blocks,
            trailing_bytes: bytes.len() - used,
        })
    }
}

fn block_sums_to_zero(block: &[u8]) -> bool {
    block.iter().fold(0u8, |sum, b| sum.wrapping_add(*b)) == 0
}

fn manufacturer_id(packed: u16) -> String {
    [packed >> 10, packed >> 5, packed]
        .iter()
        .map(|letter| match letter & 0x1F {
            n @ 1..=26 => char::from(b'A' + (n as u8) - 1),
            _ => '?',
        })
        .collect()
}

/// A progressive detailed timing descriptor. Interlaced and degenerate timings are skipped.
fn detailed_timing(d: &[u8]) -> Option<EdidMode> {
    let clock_hz = u64::from(u16::from_le_bytes([d[0], d[1]])) * 10_000;
    let h_active = u32::from(d[2]) | (u32::from(d[4] & 0xF0) << 4);
    let h_blank = u32::from(d[3]) | (u32::from(d[4] & 0x0F) << 8);
    let v_active = u32::from(d[5]) | (u32::from(d[7] & 0xF0) << 4);
    let v_blank = u32::from(d[6]) | (u32::from(d[7] & 0x0F) << 8);
    let total = u64::from(h_active + h_blank) * u64::from(v_active + v_blank);
    if clock_hz == 0 || h_active == 0 || v_active == 0 || total == 0 || d[17] & 0x80 != 0 {
        return None;
    }
    let refresh = u32::try_from((clock_hz * 1000 + total / 2) / total).ok()?;
    Some(EdidMode {
        width: u16::try_from(h_active).ok()?,
        height: u16::try_from(v_active).ok()?,
        refresh: RefreshMilliHz(refresh),
    })
}

/// A two byte standard timing. `0x01 0x01` marks an unused slot.
fn standard_timing(first: u8, second: u8, revision: u8) -> Option<EdidMode> {
    if first == 0x01 || first == 0x00 {
        return None;
    }
    let width = (u16::from(first) + 31) * 8;
    let height = match second >> 6 {
        0 if revision >= 3 => width * 10 / 16,
        0 => width,
        1 => width * 3 / 4,
        2 => width * 4 / 5,
        _ => width * 9 / 16,
    };
    let hertz = u32::from(second & 0x3F) + 60;
    Some(EdidMode {
        width,
        height,
        refresh: RefreshMilliHz(hertz * 1000),
    })
}
