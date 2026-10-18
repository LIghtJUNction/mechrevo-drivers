//! Pure parsing used by the native qualification path. No host input is a contract.
use crate::ControlError;
use sha2::{Digest, Sha256};

pub(crate) const ACPI_PROVIDER: u32 = u32::from_be_bytes(*b"ACPI");
pub(crate) const RSMB_PROVIDER: u32 = u32::from_be_bytes(*b"RSMB");
pub(crate) const DSDT_ID: u32 = u32::from_le_bytes(*b"DSDT");
pub(crate) const MAX_FIRMWARE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FirmwareIdentity {
    pub system_vendor: String,
    pub system_product: String,
    pub board_vendor: String,
    pub board_product: String,
    pub bios_vendor: String,
    pub bios_version: String,
    pub dsdt_sha256: String,
}

pub(crate) fn invalid(message: &str) -> ControlError {
    ControlError::new("windows_oem_identity_invalid", message)
}

pub(crate) fn hex_sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// RawSMBIOSData has an eight-byte wrapper; the following table is not an entry
/// point structure. Required types must be unique; zero string indexes are not
/// accepted as an identity. Unknown structures are skipped by their real length.
pub(crate) fn parse_rsmb(raw: &[u8]) -> Result<FirmwareIdentity, ControlError> {
    if raw.len() < 8 || raw.len() > MAX_FIRMWARE_BYTES {
        return Err(invalid("SMBIOS wrapper length is invalid"));
    }
    let length = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
    if length != raw.len() - 8 {
        return Err(invalid(
            "SMBIOS declared table length does not match returned bytes",
        ));
    }
    let mut bios = None;
    let mut system = None;
    let mut board = None;
    let table = &raw[8..];
    let mut offset = 0;
    let mut ended = false;
    while offset < table.len() {
        if table.len() - offset < 4 {
            return Err(invalid("SMBIOS structure header is truncated"));
        }
        let kind = table[offset];
        let formatted = table[offset + 1] as usize;
        if formatted < 4 || formatted > table.len() - offset {
            return Err(invalid("SMBIOS formatted structure length is invalid"));
        }
        let strings_start = offset + formatted;
        let tail = &table[strings_start..];
        let terminator = tail
            .windows(2)
            .position(|v| v == [0, 0])
            .ok_or_else(|| invalid("SMBIOS string area is unterminated"))?;
        let strings = &tail[..terminator];
        let string = |index: u8| -> Result<String, ControlError> {
            if index == 0 {
                return Err(invalid("SMBIOS required identity string is absent"));
            }
            let bytes = strings
                .split(|b| *b == 0)
                .nth(index as usize - 1)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| invalid("SMBIOS string index is out of range"))?;
            let s =
                std::str::from_utf8(bytes).map_err(|_| invalid("SMBIOS identity is not UTF-8"))?;
            if s.chars().any(char::is_control) {
                return Err(invalid("SMBIOS identity contains control characters"));
            }
            // SMBIOS padding whitespace is not an alternate model spelling.
            let s = s.trim();
            if s.is_empty() {
                return Err(invalid("SMBIOS identity string is empty"));
            }
            Ok(s.to_owned())
        };
        if matches!(kind, 0..=2) {
            if formatted < 6 {
                return Err(invalid("SMBIOS identity structure is truncated"));
            }
            let pair = (string(table[offset + 4])?, string(table[offset + 5])?);
            let slot = match kind {
                0 => &mut bios,
                1 => &mut system,
                _ => &mut board,
            };
            if slot.replace(pair).is_some() {
                return Err(invalid("SMBIOS identity structure is ambiguous"));
            }
        }
        offset = strings_start + terminator + 2;
        if kind == 127 {
            ended = true;
            if table[offset..].iter().any(|b| *b != 0) {
                return Err(invalid("SMBIOS has data after the end marker"));
            }
            break;
        }
    }
    if !ended {
        return Err(invalid("SMBIOS end marker is missing"));
    }
    let (bios_vendor, bios_version) =
        bios.ok_or_else(|| invalid("SMBIOS BIOS identity is missing"))?;
    let (system_vendor, system_product) =
        system.ok_or_else(|| invalid("SMBIOS system identity is missing"))?;
    let (board_vendor, board_product) =
        board.ok_or_else(|| invalid("SMBIOS board identity is missing"))?;
    Ok(FirmwareIdentity {
        system_vendor,
        system_product,
        board_vendor,
        board_product,
        bios_vendor,
        bios_version,
        dsdt_sha256: String::new(),
    })
}

pub(crate) fn validate_dsdt(ids: &[u8], table: &[u8]) -> Result<String, ControlError> {
    if ids.is_empty() || ids.len() % 4 != 0 || ids.len() > MAX_FIRMWARE_BYTES {
        return Err(invalid("ACPI table enumeration has an invalid length"));
    }
    if ids
        .chunks_exact(4)
        .filter(|id| u32::from_le_bytes((*id).try_into().unwrap()) == DSDT_ID)
        .count()
        != 1
    {
        return Err(invalid(
            "DSDT is missing or has an ambiguous duplicate signature",
        ));
    }
    if table.len() < 36 || table.len() > MAX_FIRMWARE_BYTES || &table[..4] != b"DSDT" {
        return Err(invalid("DSDT header or table length is invalid"));
    }
    if u32::from_le_bytes(table[4..8].try_into().unwrap()) as usize != table.len() {
        return Err(invalid(
            "DSDT declared length does not match returned bytes",
        ));
    }
    if table.iter().fold(0u8, |a, b| a.wrapping_add(*b)) != 0 {
        return Err(invalid("DSDT checksum failed"));
    }
    Ok(hex_sha256(table))
}

/// Restrict native path resolution to the actual Windows volume. This is not
/// filesystem canonicalization; the caller must open and compare file IDs.
pub(crate) fn resolve_driver_path(
    path: &str,
    windows: &str,
    volume_nt: &str,
) -> Result<String, ControlError> {
    if path.contains('\0') || path.contains('"') || path.contains('/') || path.contains('%') {
        return Err(invalid("Driver image path contains unsupported syntax"));
    }
    let win = windows.trim_end_matches('\\');
    if win.len() < 3 || win.as_bytes()[1] != b':' || win.as_bytes()[2] != b'\\' {
        return Err(invalid(
            "Native Windows directory is not an absolute drive path",
        ));
    }
    let lower = path.to_ascii_lowercase();
    let resolved = if lower.starts_with("\\systemroot\\") {
        format!("{}\\{}", win, &path[12..])
    } else if lower.starts_with("system32\\") {
        format!("{win}\\{path}")
    } else if lower.starts_with("\\??\\") || lower.starts_with("\\\\?\\") {
        path[4..].to_owned()
    } else if !volume_nt.is_empty()
        && lower.starts_with(&format!("{}\\", volume_nt.to_ascii_lowercase()))
    {
        format!("{}{}", &win[..2], &path[volume_nt.len()..])
    } else {
        path.to_owned()
    };
    let relative = resolved
        .get(win.len()..)
        .filter(|_| {
            resolved
                .get(..win.len())
                .is_some_and(|p| p.eq_ignore_ascii_case(win))
        })
        .ok_or_else(|| invalid("Driver image is outside the native Windows directory"))?;
    if !relative.starts_with('\\')
        || relative
            .split('\\')
            .skip(1)
            .any(|p| p.is_empty() || matches!(p, "." | "..") || p.contains(':'))
    {
        return Err(invalid("Driver path contains an invalid component"));
    }
    let allowed = relative.to_ascii_lowercase();
    if !(allowed.starts_with("\\system32\\drivers\\")
        || allowed.starts_with("\\system32\\driverstore\\filerepository\\"))
        || !allowed.ends_with("\\uwacpidriver.sys")
    {
        return Err(invalid(
            "Driver image is not the fixed driver in a protected driver location",
        ));
    }
    Ok(resolved)
}

pub(crate) fn matches_contract(c: &super::contract::SupportContract, f: &FirmwareIdentity) -> bool {
    c.smbios.system_vendor == f.system_vendor
        && c.smbios.product_name == f.system_product
        && c.smbios.board_name == f.board_product
        && c.smbios.bios_version == f.bios_version
        && c.raw_dsdt_sha256 == f.dsdt_sha256
        && c.smbios
            .board_vendor
            .as_ref()
            .is_none_or(|v| v == &f.board_vendor)
        && c.smbios
            .bios_vendor
            .as_ref()
            .is_none_or(|v| v == &f.bios_vendor)
}

/// Consume PSAPI results rather than infer a loaded identity from a filename.
/// The selected base remains internal; only its association level is exported.
pub(crate) fn select_loaded_module(
    modules: &[(usize, String)],
    windows: &str,
    volume: &str,
    expected: &str,
) -> Result<(usize, String), ControlError> {
    if modules.is_empty() || modules.iter().all(|(base, _)| *base == 0) {
        return Err(invalid("PSAPI did not disclose any loaded module base"));
    }
    let mut found = None;
    for (base, path) in modules {
        if *base == 0 || !path.to_ascii_lowercase().ends_with("\\uwacpidriver.sys") {
            continue;
        }
        let resolved = resolve_driver_path(path, windows, volume)?;
        if !resolved.eq_ignore_ascii_case(expected) {
            return Err(invalid(
                "SCM and loaded driver module point to different paths; Restart is required",
            ));
        }
        if found.replace((*base, resolved)).is_some() {
            return Err(invalid("Multiple loaded modules match the fixed driver"));
        }
    }
    found.ok_or_else(|| invalid("The fixed driver has no associated loaded module"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rsmb() -> Vec<u8> {
        let mut t = Vec::new();
        for k in 0..3u8 {
            t.extend([k, 6, 0, 0, 1, 2]);
            t.extend(b"Vendor\0GM6PX0X\0\0");
        }
        t.extend([127, 4, 0, 0, 0, 0]);
        let mut r = vec![0, 3, 6, 0];
        r.extend((t.len() as u32).to_le_bytes());
        r.extend(t);
        r
    }
    fn dsdt() -> Vec<u8> {
        let mut t = vec![0; 36];
        t[..4].copy_from_slice(b"DSDT");
        t[4..8].copy_from_slice(&36u32.to_le_bytes());
        t[9] = 0u8.wrapping_sub(t.iter().fold(0u8, |a, b| a.wrapping_add(*b)));
        t
    }
    #[test]
    fn exact_firmware_contract_rejects_each_identity_difference() {
        let catalog = super::super::contract::bundled_catalog().unwrap();
        let c = catalog
            .contracts
            .first()
            .expect("a reviewed candidate contract");
        let f = FirmwareIdentity {
            system_vendor: c.smbios.system_vendor.clone(),
            system_product: c.smbios.product_name.clone(),
            board_vendor: c.smbios.board_vendor.clone().unwrap_or_default(),
            board_product: c.smbios.board_name.clone(),
            bios_vendor: c.smbios.bios_vendor.clone().unwrap_or_default(),
            bios_version: c.smbios.bios_version.clone(),
            dsdt_sha256: c.raw_dsdt_sha256.clone(),
        };
        assert!(matches_contract(c, &f));
        for index in 0..7 {
            let mut other = f.clone();
            let field = match index {
                0 => &mut other.system_vendor,
                1 => &mut other.system_product,
                2 => &mut other.board_vendor,
                3 => &mut other.board_product,
                4 => &mut other.bios_vendor,
                5 => &mut other.bios_version,
                _ => &mut other.dsdt_sha256,
            };
            field.push('x');
            assert!(!matches_contract(c, &other));
        }
    }
    #[test]
    fn actual_module_selector_requires_one_nonnull_matching_path() {
        let win = "C:\\Windows";
        let nt = "\\Device\\HarddiskVolume3";
        let path = "C:\\Windows\\System32\\drivers\\UWACPIDriver.sys";
        let module = (
            0x1234usize,
            "\\SystemRoot\\System32\\drivers\\UWACPIDriver.sys".into(),
        );
        assert_eq!(
            select_loaded_module(std::slice::from_ref(&module), win, nt, path).unwrap(),
            (module.0, path.into())
        );
        for modules in [
            vec![],
            vec![(0, module.1.clone())],
            vec![module.clone(), module.clone()],
            vec![(1, "C:\\Users\\me\\UWACPIDriver.sys".into())],
            vec![(1, "\\SystemRoot\\System32\\drivers\\other.sys".into())],
        ] {
            assert!(select_loaded_module(&modules, win, nt, path).is_err());
        }
        assert!(select_loaded_module(
            &[module],
            win,
            nt,
            "C:\\Windows\\System32\\DriverStore\\FileRepository\\pkg\\UWACPIDriver.sys"
        )
        .is_err());
    }
    #[test]
    fn provider_and_table_ids_have_different_byte_orders() {
        assert_eq!(ACPI_PROVIDER, 0x41435049);
        assert_eq!(RSMB_PROVIDER, 0x52534d42);
        assert_eq!(DSDT_ID, 0x54445344);
    }
    #[test]
    fn real_parser_requires_unique_complete_identity() {
        let r = rsmb();
        assert_eq!(parse_rsmb(&r).unwrap().system_product, "GM6PX0X");
        for n in 0..r.len() {
            assert!(parse_rsmb(&r[..n]).is_err());
        }
        let mut r = rsmb();
        r[13] = 9;
        assert!(parse_rsmb(&r).is_err());
        let mut r = rsmb();
        r[4..8].copy_from_slice(&1u32.to_le_bytes());
        assert!(parse_rsmb(&r).is_err());
    }
    #[test]
    fn duplicate_smbios_identity_is_rejected() {
        let r = rsmb();
        let mut t = r[8..r.len() - 6].to_vec();
        t.extend_from_slice(&r[8..30]);
        t.extend([127, 4, 0, 0, 0, 0]);
        let mut r = vec![0, 3, 6, 0];
        r.extend((t.len() as u32).to_le_bytes());
        r.extend(t);
        assert!(parse_rsmb(&r).is_err());
    }
    #[test]
    fn dsdt_checks_presence_uniqueness_length_and_checksum() {
        let t = dsdt();
        assert!(validate_dsdt(b"DSDT", &t).is_ok());
        assert!(validate_dsdt(b"DSDTDSDT", &t).is_err());
        assert!(validate_dsdt(b"SSDT", &t).is_err());
        assert!(validate_dsdt(b"DSD", &t).is_err());
        for n in 0..t.len() {
            assert!(validate_dsdt(b"DSDT", &t[..n]).is_err());
        }
        let mut bad = t.clone();
        bad[35] = 1;
        assert!(validate_dsdt(b"DSDT", &bad).is_err());
        let mut bad = t;
        bad[4] = 35;
        assert!(validate_dsdt(b"DSDT", &bad).is_err());
    }
    #[test]
    fn driver_paths_are_not_shell_or_environment_inputs() {
        let win = "C:\\Windows";
        let nt = "\\Device\\HarddiskVolume3";
        for s in [
            "\\SystemRoot\\System32\\drivers\\UWACPIDriver.sys",
            "System32\\drivers\\UWACPIDriver.sys",
            "\\??\\C:\\Windows\\System32\\drivers\\UWACPIDriver.sys",
            "\\Device\\HarddiskVolume3\\Windows\\System32\\drivers\\UWACPIDriver.sys",
        ] {
            assert_eq!(
                resolve_driver_path(s, win, nt).unwrap(),
                "C:\\Windows\\System32\\drivers\\UWACPIDriver.sys"
            );
        }
        for s in [
            "%SystemRoot%\\System32\\drivers\\UWACPIDriver.sys",
            "C:\\Users\\me\\UWACPIDriver.sys",
            "C:\\Windows\\System32\\drivers\\..\\UWACPIDriver.sys",
            "C:\\Windows\\System32\\drivers\\UWACPIDriver.sys:stream",
            "C:\\Windows\\System32\\drivers\\other.sys",
            "C:\\Windows2\\System32\\drivers\\UWACPIDriver.sys",
        ] {
            assert!(resolve_driver_path(s, win, nt).is_err(), "{s}");
        }
    }
}
