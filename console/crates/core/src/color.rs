//! Shared RGB text conversion for CLI input and native UI display.
//!
//! Project Z's `zhex` performs the byte conversion in fixed-size buffers.
//! This module owns the driver's exact `#RRGGBB` input contract.

use crate::ControlError;

/// Parse exactly `#RRGGBB`, accepting either ASCII hex letter case.
pub fn parse_rgb_hex(raw: &str) -> Result<[u8; 3], ControlError> {
    let invalid = || ControlError::new("invalid_oem_value", "Keyboard color requires #RRGGBB");
    let bytes = raw.as_bytes();
    if bytes.len() != 7 || bytes.first() != Some(&b'#') {
        return Err(invalid());
    }
    let mut rgb = [0; 3];
    zhex::decode(&bytes[1..], &mut rgb).map_err(|_| invalid())?;
    Ok(rgb)
}

/// Format RGB channel order as uppercase `#RRGGBB` with one final allocation.
pub fn format_rgb_hex(rgb: [u8; 3]) -> String {
    let mut bytes = [b'#'; 7];
    zhex::encode_upper(&rgb, &mut bytes[1..]).expect("fixed RGB buffer has exact capacity");
    std::str::from_utf8(&bytes)
        .expect("hex encoding produces ASCII")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixed_case_preserves_channels_and_formats_canonically() {
        for (text, rgb) in [
            ("#000000", [0, 0, 0]),
            ("#ffffff", [255, 255, 255]),
            ("#aB12cD", [171, 18, 205]),
            ("#00fF80", [0, 255, 128]),
        ] {
            assert_eq!(parse_rgb_hex(text).unwrap(), rgb);
            assert_eq!(parse_rgb_hex(&format_rgb_hex(rgb)).unwrap(), rgb);
            assert_eq!(format_rgb_hex(rgb), text.to_uppercase());
        }
    }

    #[test]
    fn rejects_wrong_prefix_length_whitespace_and_unicode_without_panicking() {
        for text in [
            "",
            "#",
            "ffffff",
            "0xffffff",
            "#fff",
            "#ffffffff",
            " #123456",
            "#123456\n",
            "#12 456",
            "#12_456",
            "#12GG56",
            "#é1234",
            "＃1234",
        ] {
            let error = parse_rgb_hex(text).unwrap_err();
            assert_eq!(error.code, "invalid_oem_value", "{text:?}");
        }
    }

    #[test]
    fn each_channel_roundtrips_all_byte_values() {
        for value in 0..=255 {
            for rgb in [[value, 0, 0], [0, value, 0], [0, 0, value]] {
                let text = format_rgb_hex(rgb);
                assert_eq!(text.len(), 7);
                assert_eq!(parse_rgb_hex(&text).unwrap(), rgb);
            }
        }
    }
}
