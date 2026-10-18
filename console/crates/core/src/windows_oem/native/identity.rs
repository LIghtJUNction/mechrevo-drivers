//! Native read-only qualification. Disk identity and loaded-path association are
//! separate evidence; neither is a measurement of the driver's current memory.
// API contracts: learn.microsoft.com/windows/win32/api/{sysinfoapi,setupapi,
// cfgmgr32,winsvc,psapi,fileapi,aclapi,wintrust,mscat}/ . No shell or PATH lookup.
use super::super::{
    contract::{self, SessionIdentity, SupportContract},
    identity::{self, FirmwareIdentity},
    IdentityEvidence, LoadedModuleEvidence, Status,
};
use crate::ControlError;
use std::{
    ffi::c_void,
    fs::File,
    io::{Read, Seek, SeekFrom},
    mem::{size_of, zeroed},
    os::windows::{
        fs::OpenOptionsExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::Arc,
};
use windows_sys::Win32::{
    Devices::DeviceAndDriverInstallation::*,
    Foundation::{
        GetLastError, LocalFree, SetLastError, ERROR_NOT_ALL_ASSIGNED, ERROR_NO_MORE_ITEMS,
        INVALID_HANDLE_VALUE,
    },
    Security::{
        Authorization::{GetSecurityInfo, SE_FILE_OBJECT},
        Cryptography::Catalog::*,
        WinTrust::*,
        *,
    },
    Storage::FileSystem::{
        GetFileInformationByHandle, GetFinalPathNameByHandleW, QueryDosDeviceW,
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    },
    System::{
        ProcessStatus::{EnumDeviceDrivers, GetDeviceDriverFileNameW},
        Registry::{RegCloseKey, RegGetValueW, KEY_QUERY_VALUE, RRF_RT_REG_SZ},
        Services::*,
        SystemInformation::{
            EnumSystemFirmwareTables, GetSystemFirmwareTable, GetSystemWindowsDirectoryW,
        },
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const MAX_NATIVE_BYTES: usize = 64 * 1024;
const INF_SHA256: &str = "ed17fb99b0d9fab6eb116a603ea34a42b8aa38e1978f128c826f8856b13f5e8e";
const CAT_SHA256: &str = "27a8c480c1d9cdaa1ce5c4c8c9963c9df519061ce1476e398e8f6cdab2bacf6b";
fn err(message: &str) -> ControlError {
    ControlError::new("windows_oem_identity_unavailable", message)
}
fn winerr(message: &str) -> ControlError {
    let mut e = err(message);
    e.details
        .push(format!("win32={}", unsafe { GetLastError() }));
    e
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
fn decode_wide(bytes: &[u8]) -> Result<String, ControlError> {
    if bytes.len() < 2 || bytes.len() % 2 != 0 {
        return Err(err("Native UTF-16 property has an invalid size"));
    }
    let v: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .collect();
    if v.last() != Some(&0) || v[..v.len() - 1].contains(&0) {
        return Err(err("Native property is not one terminated string"));
    }
    String::from_utf16(&v[..v.len() - 1]).map_err(|_| err("Native property has invalid UTF-16"))
}
fn vec_bytes(size: usize) -> Result<Vec<u8>, ControlError> {
    if size == 0 || size > identity::MAX_FIRMWARE_BYTES {
        return Err(err("Native buffer size is outside its bound"));
    }
    let mut v = Vec::new();
    v.try_reserve_exact(size)
        .map_err(|_| err("Native buffer allocation failed"))?;
    v.resize(size, 0);
    Ok(v)
}
fn firmware_table(provider: u32, id: u32) -> Result<Vec<u8>, ControlError> {
    let n = unsafe { GetSystemFirmwareTable(provider, id, null_mut(), 0) } as usize;
    let mut b = vec_bytes(n)?;
    let actual =
        unsafe { GetSystemFirmwareTable(provider, id, b.as_mut_ptr(), b.len() as u32) } as usize;
    if actual != n {
        return Err(winerr("Firmware table size changed or retrieval failed"));
    }
    Ok(b)
}
fn firmware() -> Result<FirmwareIdentity, ControlError> {
    let mut f = identity::parse_rsmb(&firmware_table(identity::RSMB_PROVIDER, 0)?)?;
    let n = unsafe { EnumSystemFirmwareTables(identity::ACPI_PROVIDER, null_mut(), 0) } as usize;
    let mut ids = vec_bytes(n)?;
    if unsafe {
        EnumSystemFirmwareTables(identity::ACPI_PROVIDER, ids.as_mut_ptr().cast(), n as u32)
    } as usize
        != n
    {
        return Err(winerr("ACPI table enumeration changed or failed"));
    }
    f.dsdt_sha256 = identity::validate_dsdt(
        &ids,
        &firmware_table(identity::ACPI_PROVIDER, identity::DSDT_ID)?,
    )?;
    Ok(f)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PnpIdentity {
    instance: String,
    driver_key: String,
    inf: String,
    version: String,
    devinst: u32,
    status: u32,
}
struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}
fn registry_property(
    set: HDEVINFO,
    dev: &SP_DEVINFO_DATA,
    prop: u32,
) -> Result<Vec<u8>, ControlError> {
    let mut size = 0;
    let mut kind = 0;
    unsafe {
        SetupDiGetDeviceRegistryPropertyW(set, dev, prop, &mut kind, null_mut(), 0, &mut size);
    }
    if size == 0 || size as usize > MAX_NATIVE_BYTES {
        return Err(winerr("PnP registry property is absent or too large"));
    }
    let mut b = vec_bytes(size as usize)?;
    if unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            dev,
            prop,
            &mut kind,
            b.as_mut_ptr(),
            size,
            &mut size,
        )
    } == 0
        || size as usize > b.len()
    {
        return Err(winerr("PnP registry property retrieval failed"));
    }
    let expected_type = if prop == SPDRP_HARDWAREID { 7 } else { 1 };
    if kind != expected_type {
        return Err(err("PnP registry property has an unexpected registry type"));
    }
    b.truncate(size as usize);
    Ok(b)
}
fn reg_string(
    key: windows_sys::Win32::System::Registry::HKEY,
    name: &str,
) -> Result<String, ControlError> {
    let name = wide(name);
    let mut size = 0;
    if unsafe {
        RegGetValueW(
            key,
            null(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut size,
        )
    } != 0
        || size == 0
        || size as usize > MAX_NATIVE_BYTES
    {
        return Err(err("Current PnP driver registry field is unavailable"));
    }
    let mut b = vec_bytes(size as usize)?;
    if unsafe {
        RegGetValueW(
            key,
            null(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            b.as_mut_ptr().cast(),
            &mut size,
        )
    } != 0
        || size as usize > b.len()
    {
        return Err(err("Current PnP driver registry field changed or failed"));
    }
    decode_wide(&b[..size as usize])
}
fn pnp(c: &SupportContract) -> Result<PnpIdentity, ControlError> {
    let acpi = wide("ACPI");
    let set = DeviceSet(unsafe {
        SetupDiGetClassDevsW(
            null(),
            acpi.as_ptr(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_ALLCLASSES,
        )
    });
    if set.0 == INVALID_HANDLE_VALUE as isize {
        return Err(winerr("Present ACPI node enumeration failed"));
    }
    let mut candidates = Vec::new();
    let mut enumeration_complete = false;
    for index in 0..4096 {
        let mut dev: SP_DEVINFO_DATA = unsafe { zeroed() };
        dev.cbSize = size_of::<SP_DEVINFO_DATA>() as u32;
        if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut dev) } == 0 {
            if unsafe { GetLastError() } == ERROR_NO_MORE_ITEMS {
                enumeration_complete = true;
                break;
            }
            return Err(winerr("ACPI node enumeration failed mid-list"));
        }
        let mut instance = vec![0u16; 4096];
        let mut required = 0;
        if unsafe {
            SetupDiGetDeviceInstanceIdW(
                set.0,
                &dev,
                instance.as_mut_ptr(),
                instance.len() as u32,
                &mut required,
            )
        } == 0
            || required == 0
            || required as usize > instance.len()
        {
            return Err(winerr("ACPI instance identity is unavailable"));
        }
        let instance = String::from_utf16(&instance[..required as usize - 1])
            .map_err(|_| err("ACPI instance identity is malformed"))?;
        if !instance
            .to_ascii_uppercase()
            .starts_with(&format!("{}\\", c.driver.hardware_id.to_ascii_uppercase()))
        {
            continue;
        }
        let hw = registry_property(set.0, &dev, SPDRP_HARDWAREID)?;
        if hw.len() < 4 || hw.len() % 2 != 0 {
            return Err(err("ACPI hardware ID property is malformed"));
        }
        let words: Vec<u16> = hw
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        if !words.ends_with(&[0, 0])
            || !words.split(|v| *v == 0).filter(|v| !v.is_empty()).any(|v| {
                String::from_utf16(v).is_ok_and(|s| s.eq_ignore_ascii_case(&c.driver.hardware_id))
            })
        {
            return Err(err("ACPI current hardware IDs do not match the contract"));
        }
        let service = decode_wide(&registry_property(set.0, &dev, SPDRP_SERVICE)?)?;
        if !service.eq_ignore_ascii_case(&c.driver.service) {
            return Err(err("ACPI node is bound to a different service"));
        }
        let driver_key = decode_wide(&registry_property(set.0, &dev, SPDRP_DRIVER)?)?;
        let mut status = 0;
        let mut problem = 0;
        if unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, dev.DevInst, 0) } != CR_SUCCESS
            || problem != 0
            || status & DN_STARTED == 0
            || status & (DN_HAS_PROBLEM | DN_NEED_RESTART) != 0
        {
            return Err(err("ACPI node is not started cleanly or needs restart"));
        }
        let key = unsafe {
            SetupDiOpenDevRegKey(set.0, &dev, DICS_FLAG_GLOBAL, 0, DIREG_DRV, KEY_QUERY_VALUE)
        };
        if key == INVALID_HANDLE_VALUE {
            return Err(winerr("Current ACPI driver key cannot be opened"));
        }
        let properties = (|| {
            Ok::<_, ControlError>((
                reg_string(key, "InfPath")?,
                reg_string(key, "DriverVersion")?,
            ))
        })();
        unsafe {
            RegCloseKey(key);
        }
        let (inf, version) = properties?;
        if inf.contains(['\\', '/', ':'])
            || !inf.to_ascii_lowercase().ends_with(".inf")
            || version != c.driver.driver_version
        {
            return Err(err("Current PnP INF or driver version does not match"));
        }
        candidates.push(PnpIdentity {
            instance,
            driver_key,
            inf,
            version,
            devinst: dev.DevInst,
            status,
        });
    }
    if !enumeration_complete {
        return Err(err("ACPI enumeration exceeded its finite node bound"));
    }
    if candidates.len() != 1 {
        return Err(err("Expected exactly one present qualified ACPI node"));
    }
    Ok(candidates.remove(0))
}

struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
fn service_path(c: &SupportContract) -> Result<String, ControlError> {
    let scm = ServiceHandle(unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) });
    if scm.0.is_null() {
        return Err(winerr("SCM cannot be opened for query"));
    }
    let name = wide(&c.driver.service);
    let svc = ServiceHandle(unsafe {
        OpenServiceW(
            scm.0,
            name.as_ptr(),
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS,
        )
    });
    if svc.0.is_null() {
        return Err(winerr("Fixed kernel-driver service cannot be queried"));
    }
    let mut status: SERVICE_STATUS_PROCESS = unsafe { zeroed() };
    let mut size = 0;
    if unsafe {
        QueryServiceStatusEx(
            svc.0,
            SC_STATUS_PROCESS_INFO,
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast(),
            size_of::<SERVICE_STATUS_PROCESS>() as u32,
            &mut size,
        )
    } == 0
        || status.dwServiceType != SERVICE_KERNEL_DRIVER
        || status.dwCurrentState != SERVICE_RUNNING
        || status.dwCheckPoint != 0
    {
        return Err(err("Fixed kernel-driver service is not stably running"));
    }
    unsafe {
        QueryServiceConfigW(svc.0, null_mut(), 0, &mut size);
    }
    if size as usize > MAX_NATIVE_BYTES || size < (size_of::<QUERY_SERVICE_CONFIGW>() as u32) {
        return Err(winerr("SCM configuration size is invalid"));
    }
    let mut storage = vec![0usize; (size as usize).div_ceil(size_of::<usize>())];
    let cfg = storage.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    if unsafe { QueryServiceConfigW(svc.0, cfg, size, &mut size) } == 0 {
        return Err(winerr("SCM configuration query failed"));
    }
    let cfg = unsafe { &*cfg };
    if cfg.dwServiceType != SERVICE_KERNEL_DRIVER {
        return Err(err("SCM service type is not a kernel driver"));
    }
    let pointer = cfg.lpBinaryPathName as usize;
    let start = storage.as_ptr() as usize;
    let end = start + storage.len() * size_of::<usize>();
    if pointer < start || pointer >= end || pointer % 2 != 0 {
        return Err(err("SCM image path is outside its query buffer"));
    }
    let chars = unsafe { std::slice::from_raw_parts(cfg.lpBinaryPathName, (end - pointer) / 2) };
    let n = chars
        .iter()
        .position(|v| *v == 0)
        .ok_or_else(|| err("SCM image path is unterminated"))?;
    String::from_utf16(&chars[..n]).map_err(|_| err("SCM image path is malformed"))
}

/// Privilege changes are scoped and restored. An elevation prompt is handled by
/// the authenticated helper launcher, never by this read-only qualification.
struct DebugPrivilege {
    token: OwnedHandle,
    previous: TOKEN_PRIVILEGES,
}
impl DebugPrivilege {
    fn enable() -> Result<Self, ControlError> {
        unsafe {
            let mut raw = null_mut();
            if OpenProcessToken(
                GetCurrentProcess(),
                TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES,
                &mut raw,
            ) == 0
            {
                return Err(winerr("Helper token cannot be queried"));
            }
            let token = OwnedHandle::from_raw_handle(raw);
            let mut elevation: TOKEN_ELEVATION = zeroed();
            let mut n = 0;
            if GetTokenInformation(
                raw,
                TokenElevation,
                (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut n,
            ) == 0
                || elevation.TokenIsElevated == 0
            {
                return Err(err(
                    "Driver qualification requires the authorized elevated helper",
                ));
            }
            let mut luid = zeroed();
            if LookupPrivilegeValueW(null(), SE_DEBUG_NAME, &mut luid) == 0 {
                return Err(winerr("SeDebugPrivilege lookup failed"));
            }
            let request = TOKEN_PRIVILEGES {
                PrivilegeCount: 1,
                Privileges: [LUID_AND_ATTRIBUTES {
                    Luid: luid,
                    Attributes: SE_PRIVILEGE_ENABLED,
                }],
            };
            let mut previous: TOKEN_PRIVILEGES = zeroed();
            SetLastError(0);
            if AdjustTokenPrivileges(
                raw,
                0,
                &request,
                size_of::<TOKEN_PRIVILEGES>() as u32,
                &mut previous,
                &mut n,
            ) == 0
                || GetLastError() == ERROR_NOT_ALL_ASSIGNED
            {
                return Err(winerr("SeDebugPrivilege was not granted"));
            }
            Ok(Self { token, previous })
        }
    }
}
impl Drop for DebugPrivilege {
    fn drop(&mut self) {
        unsafe {
            AdjustTokenPrivileges(
                self.token.as_raw_handle(),
                0,
                &self.previous,
                0,
                null_mut(),
                null_mut(),
            );
        }
    }
}
fn windows_directory() -> Result<(String, String), ControlError> {
    let mut b = vec![0u16; 32768];
    let n = unsafe { GetSystemWindowsDirectoryW(b.as_mut_ptr(), b.len() as u32) } as usize;
    if n == 0 || n >= b.len() {
        return Err(winerr("Native Windows directory is unavailable"));
    }
    let windows =
        String::from_utf16(&b[..n]).map_err(|_| err("Native Windows directory is malformed"))?;
    if windows.len() < 3 || windows.as_bytes()[1] != b':' {
        return Err(err("Native Windows directory has no fixed drive"));
    }
    let drive = wide(&windows[..2]);
    let mut target = vec![0u16; 32768];
    let n = unsafe { QueryDosDeviceW(drive.as_ptr(), target.as_mut_ptr(), target.len() as u32) }
        as usize;
    if n == 0 || n > target.len() {
        return Err(winerr("Windows volume mapping is unavailable"));
    }
    let end = target[..n]
        .iter()
        .position(|c| *c == 0)
        .ok_or_else(|| err("Windows volume mapping is malformed"))?;
    Ok((
        windows,
        String::from_utf16(&target[..end])
            .map_err(|_| err("Windows volume mapping is malformed"))?,
    ))
}
fn legacy_target() -> Result<String, ControlError> {
    let name = wide("ACPIDriver");
    let mut b = vec![0u16; 32768];
    let n = unsafe { QueryDosDeviceW(name.as_ptr(), b.as_mut_ptr(), b.len() as u32) } as usize;
    if n == 0 || n > b.len() {
        return Err(winerr("Fixed legacy device-name mapping is unavailable"));
    }
    let end = b[..n]
        .iter()
        .position(|c| *c == 0)
        .ok_or_else(|| err("Legacy device mapping is malformed"))?;
    let value =
        String::from_utf16(&b[..end]).map_err(|_| err("Legacy device mapping is malformed"))?;
    if !value.starts_with("\\Device\\") || value.len() > 4096 {
        return Err(err("Legacy device mapping is not a native device name"));
    }
    Ok(value)
}
fn loaded_path(
    windows: &str,
    volume: &str,
    expected: &str,
) -> Result<(usize, String), ControlError> {
    let _privilege = DebugPrivilege::enable()?;
    let mut b = vec![null_mut::<c_void>(); 4096];
    let mut n = 0;
    if unsafe {
        EnumDeviceDrivers(
            b.as_mut_ptr(),
            (b.len() * size_of::<*mut c_void>()) as u32,
            &mut n,
        )
    } == 0
        || n as usize > b.len() * size_of::<*mut c_void>()
        || n as usize % size_of::<*mut c_void>() != 0
    {
        return Err(winerr("Loaded kernel-module enumeration failed or changed"));
    }
    b.truncate(n as usize / size_of::<*mut c_void>());
    if b.is_empty() || b.iter().all(|base| base.is_null()) {
        return Err(err(
            "Loaded module bases are unknown; PSAPI did not disclose identity",
        ));
    }
    let mut modules = Vec::new();
    for base in b.into_iter().filter(|base| !base.is_null()) {
        let mut name = vec![0u16; 32768];
        let n = unsafe { GetDeviceDriverFileNameW(base, name.as_mut_ptr(), name.len() as u32) }
            as usize;
        if n == 0 || n >= name.len() {
            return Err(winerr("Loaded module path could not be obtained"));
        }
        let name =
            String::from_utf16(&name[..n]).map_err(|_| err("Loaded module path is malformed"))?;
        modules.push((base as usize, name));
    }
    identity::select_loaded_module(&modules, windows, volume, expected)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    volume: u32,
    index_high: u32,
    index_low: u32,
    size_high: u32,
    size_low: u32,
    last_write_high: u32,
    last_write_low: u32,
}
fn file_identity(file: &File) -> Result<FileIdentity, ControlError> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(winerr("Opened driver file identity is unavailable"));
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(err("Protected driver path is a reparse point"));
    }
    Ok(FileIdentity {
        volume: info.dwVolumeSerialNumber,
        index_high: info.nFileIndexHigh,
        index_low: info.nFileIndexLow,
        size_high: info.nFileSizeHigh,
        size_low: info.nFileSizeLow,
        last_write_high: info.ftLastWriteTime.dwHighDateTime,
        last_write_low: info.ftLastWriteTime.dwLowDateTime,
    })
}
fn trusted_sid(sid: PSID, installer: PSID) -> bool {
    unsafe {
        !sid.is_null()
            && (IsWellKnownSid(sid, WinLocalSystemSid) != 0
                || IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0
                || (!installer.is_null() && EqualSid(sid, installer) != 0))
    }
}
fn installer_sid() -> Result<Vec<u8>, ControlError> {
    let name = wide("NT SERVICE\\TrustedInstaller");
    let mut n = 0;
    let mut domains = 0;
    let mut usage = 0;
    unsafe {
        LookupAccountNameW(
            null(),
            name.as_ptr(),
            null_mut(),
            &mut n,
            null_mut(),
            &mut domains,
            &mut usage,
        );
    }
    if n == 0 || n > 1024 || domains > 32768 {
        return Err(winerr("TrustedInstaller SID lookup failed"));
    }
    let mut sid = vec![0u8; n as usize];
    let mut domain = vec![0u16; domains as usize];
    if unsafe {
        LookupAccountNameW(
            null(),
            name.as_ptr(),
            sid.as_mut_ptr().cast(),
            &mut n,
            domain.as_mut_ptr(),
            &mut domains,
            &mut usage,
        )
    } == 0
    {
        return Err(winerr("TrustedInstaller SID lookup failed"));
    }
    Ok(sid)
}
fn protected_acl(file: &File) -> Result<(), ControlError> {
    unsafe {
        let installer = installer_sid()?;
        let mut owner = null_mut();
        let mut acl = null_mut();
        let mut descriptor = null_mut();
        let result = GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut acl,
            null_mut(),
            &mut descriptor,
        );
        if result != 0 {
            return Err(err("Protected file owner/DACL cannot be read"));
        }
        let check = (|| {
            if !trusted_sid(owner, installer.as_ptr() as PSID)
                || acl.is_null()
                || IsValidAcl(acl) == 0
            {
                return Err(err("Protected file owner or DACL is not trusted"));
            }
            // Reject grants, irrespective of preceding deny ACEs. This conservative
            // check does not try to emulate the complete Windows access-check engine.
            let dangerous = 0x4000_0000u32 | 0x1000_0000 | 0x000d_0000 | 0x0000_0156;
            for i in 0..(*acl).AceCount as u32 {
                let mut ace = null_mut();
                if GetAce(acl, i, &mut ace) == 0 {
                    return Err(winerr("Protected file ACE could not be read"));
                }
                let header = &*(ace as *const ACE_HEADER);
                if header.AceSize < 8 {
                    return Err(err("Protected file ACE is malformed"));
                }
                if header.AceFlags as u32 & INHERIT_ONLY_ACE != 0 {
                    continue;
                }
                match header.AceType {
                    0 => {
                        if header.AceSize < 16 {
                            return Err(err("Allowed ACE has no complete SID header"));
                        }
                        let a = &*(ace as *const ACCESS_ALLOWED_ACE);
                        let sid = (&a.SidStart as *const u32).cast_mut().cast();
                        let subauthorities = *((sid as *const u8).add(1)) as usize;
                        if 8 + 8 + subauthorities * 4 > header.AceSize as usize
                            || IsValidSid(sid) == 0
                        {
                            return Err(err("Allowed ACE SID is malformed"));
                        }
                        if a.Mask & dangerous != 0 && !trusted_sid(sid, installer.as_ptr() as PSID)
                        {
                            return Err(err("A non-trusted principal can modify the protected driver or package"));
                        }
                    }
                    1 | 2 | 3 => {}
                    _ => {
                        return Err(err(
                            "Protected file uses an unsupported conditional or object ACE",
                        ))
                    }
                }
            }
            Ok(())
        })();
        LocalFree(descriptor);
        check
    }
}
fn final_path(file: &File) -> Result<String, ControlError> {
    let mut b = vec![0u16; 32768];
    let n = unsafe {
        GetFinalPathNameByHandleW(file.as_raw_handle(), b.as_mut_ptr(), b.len() as u32, 0)
    } as usize;
    if n == 0 || n >= b.len() {
        return Err(winerr("Opened file final path is unavailable"));
    }
    let p = String::from_utf16(&b[..n]).map_err(|_| err("Opened file final path is malformed"))?;
    p.strip_prefix("\\\\?\\")
        .map(str::to_owned)
        .ok_or_else(|| err("Opened file final path is not local drive syntax"))
}
fn open_locked(path: &Path) -> Result<File, ControlError> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)
        .map_err(|e| {
            err(&format!(
                "Protected file cannot be held read-only without write/delete sharing: {e}"
            ))
        })?;
    file_identity(&file)?;
    protected_acl(&file)?;
    if !final_path(&file)?.eq_ignore_ascii_case(&path.to_string_lossy()) {
        return Err(err(
            "Protected path resolves through a different file location",
        ));
    }
    Ok(file)
}
fn open_protected(path: &Path) -> Result<File, ControlError> {
    let file = open_locked(path)?;
    let (windows, _) = windows_directory()?;
    let mut reached = false;
    for parent in path.ancestors().skip(1) {
        let p = parent.to_string_lossy();
        if !p
            .get(..windows.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(&windows))
        {
            return Err(err("Protected file parent escaped the Windows directory"));
        }
        let _parent = open_locked(parent)?;
        if p.eq_ignore_ascii_case(&windows) {
            reached = true;
            break;
        }
    }
    if !reached {
        return Err(err("Protected driver parent chain was incomplete"));
    }
    Ok(file)
}
fn hash_file(file: &mut File) -> Result<String, ControlError> {
    let length = file.metadata().map_err(|e| err(&e.to_string()))?.len();
    if length == 0 || length > 32 * 1024 * 1024 {
        return Err(err("Driver package file length is outside its bound"));
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|e| err(&e.to_string()))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(length as usize)
        .map_err(|_| err("Driver hash allocation failed"))?;
    file.take(length + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| err(&e.to_string()))?;
    if bytes.len() as u64 != length {
        return Err(err("Driver package file length changed while hashing"));
    }
    Ok(identity::hex_sha256(&bytes))
}
fn trust_file(file: &File, path: &Path) -> Result<(), ControlError> {
    unsafe {
        let path = wide(&path.to_string_lossy());
        let mut info = WINTRUST_FILE_INFO {
            cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: path.as_ptr(),
            hFile: file.as_raw_handle(),
            pgKnownSubject: null_mut(),
        };
        let mut data: WINTRUST_DATA = zeroed();
        data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
        data.dwUIChoice = WTD_UI_NONE;
        data.fdwRevocationChecks = WTD_REVOKE_WHOLECHAIN;
        data.dwUnionChoice = WTD_CHOICE_FILE;
        data.Anonymous.pFile = &mut info;
        data.dwStateAction = WTD_STATEACTION_VERIFY;
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let result = WinVerifyTrust(
            INVALID_HANDLE_VALUE,
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(
            INVALID_HANDLE_VALUE,
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        if result != 0 {
            let mut e = err("Authenticode trust or revocation verification failed");
            e.details.push(format!("trust_status={result:#x}"));
            return Err(e);
        }
        Ok(())
    }
}
fn trust_catalog_member(file: &File, path: &Path, cat_path: &Path) -> Result<(), ControlError> {
    unsafe {
        let mut admin = 0;
        let algorithm = wide("SHA256");
        if CryptCATAdminAcquireContext2(&mut admin, null(), algorithm.as_ptr(), null(), 0) == 0 {
            return Err(winerr("SHA256 catalog verification context is unavailable"));
        }
        let check = (|| {
            let mut n = 0;
            if CryptCATAdminCalcHashFromFileHandle2(
                admin,
                file.as_raw_handle(),
                &mut n,
                null_mut(),
                0,
            ) == 0
                || n == 0
                || n > 128
            {
                return Err(winerr("Catalog member hash size failed"));
            }
            let mut hash = vec![0; n as usize];
            if CryptCATAdminCalcHashFromFileHandle2(
                admin,
                file.as_raw_handle(),
                &mut n,
                hash.as_mut_ptr(),
                0,
            ) == 0
                || n as usize != hash.len()
            {
                return Err(winerr("Catalog member hash failed"));
            }
            let tag = wide(&hash.iter().map(|b| format!("{b:02X}")).collect::<String>());
            let cat = wide(&cat_path.to_string_lossy());
            let member = wide(&path.to_string_lossy());
            let mut info: WINTRUST_CATALOG_INFO = zeroed();
            info.cbStruct = size_of::<WINTRUST_CATALOG_INFO>() as u32;
            info.pcwszCatalogFilePath = cat.as_ptr();
            info.pcwszMemberTag = tag.as_ptr();
            info.pcwszMemberFilePath = member.as_ptr();
            info.hMemberFile = file.as_raw_handle();
            info.pbCalculatedFileHash = hash.as_mut_ptr();
            info.cbCalculatedFileHash = n;
            info.hCatAdmin = admin;
            let mut data: WINTRUST_DATA = zeroed();
            data.cbStruct = size_of::<WINTRUST_DATA>() as u32;
            data.dwUIChoice = WTD_UI_NONE;
            data.fdwRevocationChecks = WTD_REVOKE_WHOLECHAIN;
            data.dwUnionChoice = WTD_CHOICE_CATALOG;
            data.Anonymous.pCatalog = &mut info;
            data.dwStateAction = WTD_STATEACTION_VERIFY;
            let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
            let result = WinVerifyTrust(
                INVALID_HANDLE_VALUE,
                &mut action,
                (&mut data as *mut WINTRUST_DATA).cast(),
            );
            data.dwStateAction = WTD_STATEACTION_CLOSE;
            WinVerifyTrust(
                INVALID_HANDLE_VALUE,
                &mut action,
                (&mut data as *mut WINTRUST_DATA).cast(),
            );
            if result != 0 {
                let mut e =
                    err("The fixed driver is not a trusted member of its current package catalog");
                e.details.push(format!("trust_status={result:#x}"));
                return Err(e);
            }
            Ok(())
        })();
        CryptCATAdminReleaseContext(admin, 0);
        check
    }
}
fn package_files(
    windows: &str,
    pnp: &PnpIdentity,
    driver: &File,
    driver_path: &Path,
    expected: &str,
) -> Result<Vec<File>, ControlError> {
    let published = PathBuf::from(windows).join("INF").join(&pnp.inf);
    let published = wide(&published.to_string_lossy());
    let mut b = vec![0u16; 32768];
    let mut needed = 0;
    if unsafe {
        SetupGetInfDriverStoreLocationW(
            published.as_ptr(),
            null(),
            null(),
            b.as_mut_ptr(),
            b.len() as u32,
            &mut needed,
        )
    } == 0
        || needed == 0
        || needed as usize > b.len()
    {
        return Err(winerr("Current INF package location cannot be resolved"));
    }
    let inf = PathBuf::from(
        String::from_utf16(&b[..needed as usize - 1])
            .map_err(|_| err("DriverStore package location is malformed"))?,
    );
    let folder = inf
        .parent()
        .ok_or_else(|| err("Current INF has no DriverStore parent"))?;
    let prefix = PathBuf::from(windows)
        .join("System32")
        .join("DriverStore")
        .join("FileRepository");
    if !folder
        .to_string_lossy()
        .to_ascii_lowercase()
        .starts_with(&format!(
            "{}\\",
            prefix.to_string_lossy().to_ascii_lowercase()
        ))
    {
        return Err(err("Current INF is outside the native DriverStore"));
    }
    let cat = folder.join("uwacpidriver.cat");
    let package_driver = folder.join("UWACPIDriver.sys");
    let mut inf_file = open_protected(&inf)?;
    let mut cat_file = open_protected(&cat)?;
    let mut package_file = open_protected(&package_driver)?;
    if hash_file(&mut inf_file)? != INF_SHA256
        || hash_file(&mut cat_file)? != CAT_SHA256
        || hash_file(&mut package_file)? != expected
    {
        return Err(err(
            "Current INF/CAT/driver package is not the fixed reviewed package",
        ));
    }
    trust_file(&cat_file, &cat)?;
    trust_catalog_member(driver, driver_path, &cat)?;
    Ok(vec![inf_file, cat_file, package_file])
}

/// Private capability, constructed only after native checks. Retained files deny
/// write/delete sharing throughout the transaction. Never deserialized.
pub(super) struct QualifiedDriverGuard {
    file: File,
    package_files: Vec<File>,
    file_id: FileIdentity,
    driver_path: String,
    pnp: PnpIdentity,
    loaded_base: usize,
    legacy_target: String,
    firmware: FirmwareIdentity,
    contract: SupportContract,
}
impl QualifiedDriverGuard {
    pub(super) fn session_identity(&self) -> SessionIdentity {
        // Debug-formatted native data remains helper-internal; no base is exposed
        // in the serializable evidence. Full-byte binding is added by transaction.
        let material = format!(
            "{:?}|{:?}|{:?}|{}|{}|{:?}|{:x}",
            self.firmware,
            self.pnp,
            self.file_id,
            self.driver_path,
            self.legacy_target,
            self.contract,
            self.loaded_base
        );
        SessionIdentity::new(&self.contract.id, material.as_bytes())
    }
    pub(super) fn status(&self) -> Status {
        Status{contract_id:Some(self.contract.id.clone()),support_stage:Some(self.contract.stage),qualified:true,control:None,evidence:IdentityEvidence{disk_file_sha256:Some(self.contract.driver.disk_file_sha256.clone()),loaded_module_metadata:Some(LoadedModuleEvidence{service:self.contract.driver.service.clone(),device_instance_id:self.pnp.instance.clone(),driver_inf:self.pnp.inf.clone(),driver_version:self.pnp.version.clone(),loaded_path:self.driver_path.clone(),association_level:"pnp_service_loaded_path_and_legacy_device_name_not_attachment_or_memory_measurement".into()}),load_time_provenance:None,current_memory_measurement:None,runtime_acceptance_id:self.contract.runtime_acceptance.as_ref().map(|a|a.id.clone())},blocking_errors:vec![],diagnostics:vec!["Disk hash and PnP/SCM/PSAPI path association are not a loaded-memory hash or a kernel device-stack attachment proof.".into(),"Legacy device mapping has no public per-object generation proof; concurrent OEM or firmware writes remain possible.".into(),"No load-time provenance was inferred; Windows runtime acceptance and Restart evidence remain separate.".into()]}
    }
    pub(super) fn revalidate(&self) -> Result<SessionIdentity, ControlError> {
        if firmware()? != self.firmware
            || pnp(&self.contract)? != self.pnp
            || file_identity(&self.file)? != self.file_id
        {
            return Err(err("Firmware, PnP lifecycle or held file identity changed"));
        }
        // Retained handles keep package bytes stable; each revalidation checks
        // association and the held handles, not a newly substituted file alone.
        for file in &self.package_files {
            file_identity(file)?;
        }
        let (windows, volume) = windows_directory()?;
        let configured =
            identity::resolve_driver_path(&service_path(&self.contract)?, &windows, &volume)?;
        let loaded = loaded_path(&windows, &volume, &configured)?;
        if !configured.eq_ignore_ascii_case(&self.driver_path)
            || loaded.0 != self.loaded_base
            || legacy_target()? != self.legacy_target
        {
            return Err(err(
                "Loaded service/module or fixed device-name association changed",
            ));
        }
        let reopened = open_protected(Path::new(&configured))?;
        if file_identity(&reopened)? != self.file_id {
            return Err(err(
                "Loaded-path disk file was replaced after qualification",
            ));
        }
        Ok(self.session_identity())
    }
}

/// Typed partial evidence survives a qualification failure. Neither this status
/// nor any deserialized status can manufacture the private guard capability.
pub(super) struct QualificationFailure {
    pub(super) status: Status,
    pub(super) error: ControlError,
}
impl QualificationFailure {
    pub(super) fn new(mut status: Status, error: ControlError) -> Self {
        status.qualified = false;
        status.control = None;
        status.blocking_errors.push(error.clone());
        Self { status, error }
    }
    pub(super) fn empty(error: ControlError) -> Self {
        Self::new(Status::default(), error)
    }
}
pub(super) fn qualify() -> Result<Arc<QualifiedDriverGuard>, QualificationFailure> {
    let mut partial = Status::default();
    let attempt = (|| -> Result<Arc<QualifiedDriverGuard>, ControlError> {
        let firmware = firmware()?;
        partial.diagnostics.push(format!(
            "Native SMBIOS system={:?}/{:?}, board={:?}/{:?}, BIOS={:?}/{:?}; raw DSDT SHA256={}",
            firmware.system_vendor,
            firmware.system_product,
            firmware.board_vendor,
            firmware.board_product,
            firmware.bios_vendor,
            firmware.bios_version,
            firmware.dsdt_sha256
        ));
        let catalog = contract::bundled_catalog()?;
        let mut matches = catalog
            .contracts
            .into_iter()
            .filter(|c| identity::matches_contract(c, &firmware));
        let contract = matches.next().ok_or_else(|| {
            err("No release-owned contract matches the exact SMBIOS and raw DSDT")
        })?;
        if matches.next().is_some() {
            return Err(err("More than one release contract matches this firmware"));
        }
        partial.contract_id = Some(contract.id.clone());
        partial.support_stage = Some(contract.stage);
        partial.evidence.runtime_acceptance_id =
            contract.runtime_acceptance.as_ref().map(|a| a.id.clone());
        let build = windows_build()?;
        if !contract.permits_current_build(build) {
            return Err(ControlError::new("windows_fnlock_not_accepted","This firmware is a candidate without runtime acceptance for this build; use the separately built candidate artifact for supervised acceptance"));
        }
        if !cfg!(target_arch = "x86_64") {
            return Err(err("The fixed driver contract requires native x64"));
        }
        let pnp = pnp(&contract)?;
        let (windows, volume) = windows_directory()?;
        let driver_path =
            identity::resolve_driver_path(&service_path(&contract)?, &windows, &volume)?;
        let loaded = loaded_path(&windows, &volume, &driver_path)?;
        partial.evidence.loaded_module_metadata = Some(LoadedModuleEvidence {
            service: contract.driver.service.clone(),
            device_instance_id: pnp.instance.clone(),
            driver_inf: pnp.inf.clone(),
            driver_version: pnp.version.clone(),
            loaded_path: driver_path.clone(),
            association_level:
                "pnp_service_loaded_path_not_device_attachment_or_memory_measurement".into(),
        });
        let legacy_target = legacy_target()?;
        let mut file = open_protected(Path::new(&driver_path))?;
        let file_id = file_identity(&file)?;
        let actual_hash = hash_file(&mut file)?;
        partial.evidence.disk_file_sha256 = Some(actual_hash.clone());
        if actual_hash != contract.driver.disk_file_sha256 {
            return Err(err(
                "Loaded-path disk driver hash does not match the fixed contract",
            ));
        }
        trust_file(&file, Path::new(&driver_path))?;
        let package_files = package_files(
            &windows,
            &pnp,
            &file,
            Path::new(&driver_path),
            &contract.driver.disk_file_sha256,
        )?;
        let guard = Arc::new(QualifiedDriverGuard {
            file,
            package_files,
            file_id,
            driver_path,
            pnp,
            loaded_base: loaded.0,
            legacy_target,
            firmware,
            contract,
        });
        guard.revalidate()?;
        Ok(guard)
    })();
    attempt.map_err(|e| QualificationFailure::new(partial, e))
}

fn windows_build() -> Result<u32, ControlError> {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;
    let mut info: OSVERSIONINFOW = unsafe { zeroed() };
    info.dwOSVersionInfoSize = size_of::<OSVERSIONINFOW>() as u32;
    if unsafe { RtlGetVersion(&mut info) } != 0 || info.dwMajorVersion < 10 {
        return Err(err("Native Windows build could not be qualified"));
    }
    Ok(info.dwBuildNumber)
}
