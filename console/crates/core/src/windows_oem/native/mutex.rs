//! One fixed application writer mutex. It is not an OEM/firmware lock or CAS.
//! Windows ownership is thread-affine; a guard must never cross threads.
//!
//! API contracts: Microsoft CreateMutexExW, GetSecurityInfo, WaitForSingleObject,
//! ReleaseMutex, SHGetKnownFolderPath and CreateFileW. All FFI layouts below come
//! from windows-sys; caller-supplied names, security descriptors and paths are
//! deliberately absent from this module's interface.

const NAME: &str = "Global\\MechrevoConsole.Oem.v1.FnLock";
const MUTEX_RIGHTS: u32 = 0x0012_0001; // SYNCHRONIZE | READ_CONTROL | MUTEX_MODIFY_STATE
const DIRECTORY_RIGHTS: u32 = 0x001f_01ff; // FILE_ALL_ACCESS
const MUTEX_SDDL: &str = "O:BAG:BAD:P(A;;0x00120001;;;SY)(A;;0x00120001;;;BA)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Principal {
    System,
    Administrators,
    Other,
}

#[derive(Clone, Copy, Debug)]
struct Grant {
    allow: bool,
    flags: u8,
    mask: u32,
    principal: Principal,
}

#[derive(Debug)]
struct Descriptor {
    owner: Principal,
    group: Principal,
    owner_defaulted: bool,
    group_defaulted: bool,
    protected: bool,
    dacl_present: bool,
    dacl_defaulted: bool,
    grants: Vec<Grant>,
}

#[derive(Clone, Copy)]
enum Policy {
    Mutex,
    GateDirectory,
}

// The Windows extractor below feeds this same policy used by offline tests.
fn validate_descriptor(value: &Descriptor, policy: Policy) -> Result<(), &'static str> {
    if value.owner == Principal::Other || value.group == Principal::Other {
        return Err("The object's owner/group is not SYSTEM or Administrators");
    }
    if value.owner_defaulted || value.group_defaulted {
        return Err("The object's owner/group was not explicitly assigned");
    }
    if !value.protected || !value.dacl_present || value.dacl_defaulted {
        return Err("The object does not have an explicit protected DACL");
    }
    let (mask, flags) = match policy {
        Policy::Mutex => (MUTEX_RIGHTS, 0),
        Policy::GateDirectory => (DIRECTORY_RIGHTS, 3), // OI | CI, no inherited ACE
    };
    if value.grants.len() != 2 {
        return Err("The object's DACL must contain exactly the two fixed grants");
    }
    let mut system = false;
    let mut administrators = false;
    for grant in &value.grants {
        if !grant.allow || grant.flags != flags || grant.mask != mask {
            return Err("Unexpected ACE type, inheritance flags or access rights");
        }
        match grant.principal {
            Principal::System if !system => system = true,
            Principal::Administrators if !administrators => administrators = true,
            _ => return Err("The DACL has an untrusted or duplicate trustee"),
        }
    }
    if system && administrators {
        Ok(())
    } else {
        Err("The DACL does not grant both fixed trusted principals")
    }
}

#[derive(Clone, Copy, Debug)]
enum WaitResult {
    Acquired,
    Abandoned,
}

#[derive(Default)]
struct Ownership {
    owned: bool,
    abandoned: bool,
}

impl Ownership {
    fn acquired(result: WaitResult) -> Self {
        Self {
            owned: true,
            abandoned: matches!(result, WaitResult::Abandoned),
        }
    }

    fn take_release(&mut self) -> bool {
        std::mem::take(&mut self.owned)
    }
}

fn gate_allows_ec(parents_trusted: bool, absent: bool) -> bool {
    parents_trusted && absent
}

// Native callers provide offsets from the SDK's SID/ACCESS_ALLOWED_ACE layouts.
// Establish the entire variable-length SID extent before any SID API sees a
// pointer. IsValidAcl alone does not establish that extent.
fn bounded_sid(
    ace: &[u8],
    sid_offset: usize,
    sid_header_bytes: usize,
    count_offset: usize,
    maximum_count: u8,
) -> Result<&[u8], &'static str> {
    if sid_header_bytes == 0 || count_offset >= sid_header_bytes {
        return Err("Invalid SID layout");
    }
    let sid = ace.get(sid_offset..).ok_or("SID offset exceeds the ACE")?;
    if sid.len() < sid_header_bytes {
        return Err("ACE does not contain a complete SID header");
    }
    let count = sid[count_offset];
    if count > maximum_count {
        return Err("SID has too many subauthorities");
    }
    let required = usize::from(count)
        .checked_mul(std::mem::size_of::<u32>())
        .and_then(|bytes| sid_header_bytes.checked_add(bytes))
        .ok_or("SID length overflow")?;
    if required != sid.len() {
        return Err("SID length does not match the ACE extent");
    }
    Ok(sid)
}

#[cfg(windows)]
pub(super) use platform::FnLockMutex;

#[cfg(windows)]
mod platform {
    use super::*;
    use crate::ControlError;
    use std::{
        ffi::OsString,
        marker::PhantomData,
        os::windows::ffi::{OsStrExt, OsStringExt},
        path::{Component, Path, PathBuf, Prefix},
        ptr,
        rc::Rc,
    };
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, GetLastError, LocalFree, ERROR_FILE_NOT_FOUND, HANDLE,
            INVALID_HANDLE_VALUE, WAIT_ABANDONED, WAIT_FAILED, WAIT_OBJECT_0,
        },
        Security::{
            AclSizeInformation,
            Authorization::{
                ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
                SDDL_REVISION_1, SE_FILE_OBJECT, SE_KERNEL_OBJECT, SE_OBJECT_TYPE,
            },
            GetAce, GetAclInformation, GetLengthSid, GetSecurityDescriptorControl,
            GetSecurityDescriptorDacl, GetSecurityDescriptorGroup, GetSecurityDescriptorOwner,
            IsValidAcl, IsValidSecurityDescriptor, IsValidSid, IsWellKnownSid,
            WinBuiltinAdministratorsSid, WinLocalSystemSid, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
            ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION,
            OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
            SE_DACL_PROTECTED, SID,
        },
        Storage::FileSystem::{
            CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ALL_ACCESS,
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, OPEN_EXISTING,
            READ_CONTROL, SYNCHRONIZE,
        },
        System::{
            Com::CoTaskMemFree,
            SystemServices::{ACCESS_ALLOWED_ACE_TYPE, SID_MAX_SUB_AUTHORITIES},
            Threading::{
                CreateMutexExW, ReleaseMutex, WaitForSingleObject, INFINITE, MUTEX_MODIFY_STATE,
            },
        },
        UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath, KF_FLAG_DEFAULT},
    };

    fn error(code: &str, action: &str, number: u32) -> ControlError {
        let mut error = ControlError::new(
            code,
            format!(
                "{action}: {}",
                std::io::Error::from_raw_os_error(number as i32)
            ),
        );
        error.details.push(format!("win32_error={number}"));
        error
    }

    fn refused(code: &str, reason: &str) -> ControlError {
        ControlError::new(code, reason)
    }

    struct LocalDescriptor(PSECURITY_DESCRIPTOR);
    impl Drop for LocalDescriptor {
        fn drop(&mut self) {
            // SAFETY: both descriptor-producing APIs return one LocalAlloc allocation.
            unsafe {
                LocalFree(self.0.cast());
            }
        }
    }

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: this owns one non-null, non-INVALID_HANDLE_VALUE handle.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn principal(sid: PSID) -> Result<Principal, ControlError> {
        // SAFETY: SID pointers come from a validated security descriptor/ACE.
        if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
            return Err(refused(
                "windows_oem_mutex_security_invalid",
                "The object has an invalid SID",
            ));
        }
        // SAFETY: IsValidSid succeeded and these are SDK well-known SID kinds.
        Ok(unsafe {
            if IsWellKnownSid(sid, WinLocalSystemSid) != 0 {
                Principal::System
            } else if IsWellKnownSid(sid, WinBuiltinAdministratorsSid) != 0 {
                Principal::Administrators
            } else {
                Principal::Other
            }
        })
    }

    fn descriptor(handle: HANDLE, object_type: SE_OBJECT_TYPE) -> Result<Descriptor, ControlError> {
        let mut raw = ptr::null_mut();
        // SAFETY: handle stays open, all optional nested pointers are omitted;
        // the allocated descriptor owns its internal owner/group/DACL pointers.
        let status = unsafe {
            GetSecurityInfo(
                handle,
                object_type,
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                &mut raw,
            )
        };
        if status != 0 {
            return Err(error(
                "windows_oem_mutex_security_query",
                "Cannot read the object's security descriptor",
                status,
            ));
        }
        if raw.is_null() {
            return Err(refused(
                "windows_oem_mutex_security_invalid",
                "No security descriptor was returned",
            ));
        }
        let allocation = LocalDescriptor(raw);
        // SAFETY: allocation is the complete API-returned security descriptor.
        if unsafe { IsValidSecurityDescriptor(allocation.0) } == 0 {
            return Err(refused(
                "windows_oem_mutex_security_invalid",
                "The object's security descriptor is invalid",
            ));
        }
        let mut owner = ptr::null_mut();
        let mut group = ptr::null_mut();
        let mut owner_defaulted = 0;
        let mut group_defaulted = 0;
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl: *mut ACL = ptr::null_mut();
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: valid API descriptor and correctly typed writable SDK outputs.
        let ok = unsafe {
            GetSecurityDescriptorOwner(raw, &mut owner, &mut owner_defaulted) != 0
                && GetSecurityDescriptorGroup(raw, &mut group, &mut group_defaulted) != 0
                && GetSecurityDescriptorDacl(raw, &mut present, &mut acl, &mut defaulted) != 0
                && GetSecurityDescriptorControl(raw, &mut control, &mut revision) != 0
        };
        if !ok {
            // SAFETY: immediately retain the last failed API's thread-local error.
            let last = unsafe { GetLastError() };
            return Err(error(
                "windows_oem_mutex_security_query",
                "Cannot decode the object's security descriptor",
                last,
            ));
        }
        let mut value = Descriptor {
            owner: principal(owner)?,
            group: principal(group)?,
            owner_defaulted: owner_defaulted != 0,
            group_defaulted: group_defaulted != 0,
            protected: control & SE_DACL_PROTECTED != 0,
            dacl_present: present != 0 && !acl.is_null(),
            dacl_defaulted: defaulted != 0,
            grants: Vec::new(),
        };
        if !value.dacl_present {
            return Ok(value);
        }
        // SAFETY: acl belongs to the validated descriptor retained above.
        if unsafe { IsValidAcl(acl) } == 0 {
            return Err(refused(
                "windows_oem_mutex_security_invalid",
                "The object's DACL is invalid",
            ));
        }
        let mut size = ACL_SIZE_INFORMATION::default();
        // SAFETY: size is the official SDK layout with its exact size.
        if unsafe {
            GetAclInformation(
                acl,
                (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
        {
            let last = unsafe { GetLastError() };
            return Err(error(
                "windows_oem_mutex_security_query",
                "Cannot inspect the object's DACL",
                last,
            ));
        }
        if size.AceCount != 2 {
            return Ok(value);
        }
        for index in 0..size.AceCount {
            let mut ace = ptr::null_mut();
            // SAFETY: index is below the validated ACL's advertised count.
            if unsafe { GetAce(acl, index, &mut ace) } == 0 {
                let last = unsafe { GetLastError() };
                return Err(error(
                    "windows_oem_mutex_security_query",
                    "Cannot inspect the object's ACE",
                    last,
                ));
            }
            if ace.is_null() {
                return Err(refused(
                    "windows_oem_mutex_security_invalid",
                    "No ACE was returned",
                ));
            }
            let offset = (ace as usize).checked_sub(acl as usize).ok_or_else(|| {
                refused(
                    "windows_oem_mutex_security_invalid",
                    "ACE is outside its DACL",
                )
            })?;
            let remaining = (size.AclBytesInUse as usize)
                .checked_sub(offset)
                .ok_or_else(|| {
                    refused(
                        "windows_oem_mutex_security_invalid",
                        "ACE is outside its DACL",
                    )
                })?;
            if remaining < std::mem::size_of::<ACE_HEADER>() {
                return Err(refused(
                    "windows_oem_mutex_security_invalid",
                    "Truncated ACE header",
                ));
            }
            // SAFETY: bounds above cover the SDK header; unaligned reads are allowed.
            let header = unsafe { ptr::read_unaligned(ace.cast::<ACE_HEADER>()) };
            if header.AceType as u32 != ACCESS_ALLOWED_ACE_TYPE {
                value.grants.push(Grant {
                    allow: false,
                    flags: header.AceFlags,
                    mask: 0,
                    principal: Principal::Other,
                });
                continue;
            }
            let length = usize::from(header.AceSize);
            if length < std::mem::size_of::<ACCESS_ALLOWED_ACE>() || length > remaining {
                return Err(refused(
                    "windows_oem_mutex_security_invalid",
                    "Truncated access ACE",
                ));
            }
            // SAFETY: length covers the SDK layout, and SID offset is obtained
            // from that layout rather than a manually declared ABI structure.
            let allowed = unsafe { ptr::read_unaligned(ace.cast::<ACCESS_ALLOWED_ACE>()) };
            let sid_offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
            // SAFETY: the ACE's full byte extent was bounded against the ACL
            // above. This slice is inspected before invoking any SID routine.
            let ace_bytes = unsafe { std::slice::from_raw_parts(ace.cast::<u8>(), length) };
            let sid_bytes = bounded_sid(
                ace_bytes,
                sid_offset,
                std::mem::offset_of!(SID, SubAuthority),
                std::mem::offset_of!(SID, SubAuthorityCount),
                SID_MAX_SUB_AUTHORITIES as u8,
            )
            .map_err(|reason| refused("windows_oem_mutex_security_invalid", reason))?;
            let sid = sid_bytes.as_ptr().cast_mut().cast();
            let trustee = principal(sid)?;
            let sid_length = unsafe { GetLengthSid(sid) } as usize;
            if sid_length != sid_bytes.len() {
                return Err(refused(
                    "windows_oem_mutex_security_invalid",
                    "ACE has an unexpected SID size",
                ));
            }
            value.grants.push(Grant {
                allow: true,
                flags: header.AceFlags,
                mask: allowed.Mask,
                principal: trustee,
            });
        }
        Ok(value)
    }

    fn verify(
        handle: HANDLE,
        object_type: SE_OBJECT_TYPE,
        policy: Policy,
    ) -> Result<(), ControlError> {
        validate_descriptor(&descriptor(handle, object_type)?, policy)
            .map_err(|reason| refused("windows_oem_mutex_security_untrusted", reason))
    }

    /// Holds exactly one successful wait, including WAIT_ABANDONED ownership.
    // @mark mechrevo.ec_bridge role=synchronization
    pub struct FnLockMutex {
        handle: Handle,
        ownership: Ownership,
        _thread_affine: PhantomData<Rc<()>>,
    }

    impl FnLockMutex {
        pub fn acquire() -> Result<Self, ControlError> {
            debug_assert_eq!(
                MUTEX_RIGHTS,
                SYNCHRONIZE | READ_CONTROL | MUTEX_MODIFY_STATE
            );
            debug_assert_eq!(DIRECTORY_RIGHTS, FILE_ALL_ACCESS);
            let sddl: Vec<u16> = MUTEX_SDDL.encode_utf16().chain(Some(0)).collect();
            let name: Vec<u16> = NAME.encode_utf16().chain(Some(0)).collect();
            let mut raw = ptr::null_mut();
            // SAFETY: both strings are terminated and the API initializes raw.
            if unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut raw,
                    ptr::null_mut(),
                )
            } == 0
            {
                let last = unsafe { GetLastError() };
                return Err(error(
                    "windows_oem_mutex_security_create",
                    "Cannot construct the fixed mutex security descriptor",
                    last,
                ));
            }
            if raw.is_null() {
                return Err(refused(
                    "windows_oem_mutex_security_invalid",
                    "No fixed mutex security descriptor was returned",
                ));
            }
            let security = LocalDescriptor(raw);
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: security.0,
                bInheritHandle: 0,
            };
            // No initial ownership: an existing object's ownership is unknown.
            // O:BA may be invalid for a token without the appropriate owner
            // group. Preserve that failure; never retry with a weaker owner.
            let raw_handle = unsafe { CreateMutexExW(&attributes, name.as_ptr(), 0, MUTEX_RIGHTS) };
            if raw_handle.is_null() {
                let last = unsafe { GetLastError() };
                return Err(error(
                    "windows_oem_mutex_open",
                    "Cannot create/open the fixed Fn mutex",
                    last,
                ));
            }
            let handle = Handle(raw_handle);
            verify(handle.0, SE_KERNEL_OBJECT, Policy::Mutex)?;
            // SAFETY: live handle with SYNCHRONIZE; this thread has no UI pump.
            let waited = unsafe { WaitForSingleObject(handle.0, INFINITE) };
            let result = match waited {
                WAIT_OBJECT_0 => WaitResult::Acquired,
                WAIT_ABANDONED => WaitResult::Abandoned,
                WAIT_FAILED => {
                    let last = unsafe { GetLastError() };
                    return Err(error(
                        "windows_oem_mutex_wait",
                        "Cannot acquire the Fn mutex",
                        last,
                    ));
                }
                other => {
                    return Err(refused(
                        "windows_oem_mutex_wait",
                        &format!("Unexpected infinite-wait result {other}"),
                    ))
                }
            };
            // Establish RAII immediately: post-wait failures must release even
            // when Windows returned WAIT_ABANDONED rather than WAIT_OBJECT_0.
            let guard = Self {
                handle,
                ownership: Ownership::acquired(result),
                _thread_affine: PhantomData,
            };
            verify(guard.handle.0, SE_KERNEL_OBJECT, Policy::Mutex)?;
            check_fixed_maintenance_gate()?;
            Ok(guard)
        }

        pub fn abandoned(&self) -> bool {
            self.ownership.abandoned
        }
    }

    impl Drop for FnLockMutex {
        fn drop(&mut self) {
            if self.ownership.take_release() {
                // SAFETY: !Send/!Sync pins the single successful wait to this
                // thread. The handle remains live until Handle's Drop below.
                unsafe {
                    ReleaseMutex(self.handle.0);
                }
            }
        }
    }

    struct KnownFolder(*mut u16);
    impl Drop for KnownFolder {
        fn drop(&mut self) {
            // SAFETY: SHGetKnownFolderPath allocates this buffer with COM memory.
            unsafe {
                CoTaskMemFree(self.0.cast());
            }
        }
    }

    fn program_data() -> Result<PathBuf, ControlError> {
        let mut raw = ptr::null_mut();
        // SAFETY: fixed SDK KnownFolder ID; output is COM allocated on success.
        let hr = unsafe {
            SHGetKnownFolderPath(
                &FOLDERID_ProgramData,
                KF_FLAG_DEFAULT as u32,
                ptr::null_mut(),
                &mut raw,
            )
        };
        if hr < 0 || raw.is_null() {
            if !raw.is_null() {
                unsafe {
                    CoTaskMemFree(raw.cast());
                }
            }
            return Err(refused(
                "windows_oem_maintenance_path",
                &format!("Cannot resolve fixed ProgramData KnownFolder (HRESULT {hr:#x})"),
            ));
        }
        let allocation = KnownFolder(raw);
        // SAFETY: the API returns a NUL-terminated UTF-16 string. Bound the
        // accepted length; do not obtain the root from environment variables.
        let length = (0..32768)
            .find(|&index| unsafe { *allocation.0.add(index) == 0 })
            .ok_or_else(|| {
                refused(
                    "windows_oem_maintenance_path",
                    "ProgramData path exceeds the fixed limit",
                )
            })?;
        let path = PathBuf::from(OsString::from_wide(unsafe {
            std::slice::from_raw_parts(allocation.0, length)
        }));
        let mut components = path.components();
        let local_disk = matches!(components.next(), Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)));
        if !local_disk
            || components.next() != Some(Component::RootDir)
            || components.any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(refused(
                "windows_oem_maintenance_path",
                "ProgramData is not a local absolute system path",
            ));
        }
        Ok(path)
    }

    fn path_wide(path: &Path) -> Result<Vec<u16>, ControlError> {
        let mut wide: Vec<_> = path.as_os_str().encode_wide().collect();
        if wide.is_empty() || wide.len() > 32767 || wide.contains(&0) {
            return Err(refused(
                "windows_oem_maintenance_path",
                "Invalid fixed maintenance path",
            ));
        }
        wide.push(0);
        Ok(wide)
    }

    fn directory(path: &Path, protected: bool) -> Result<Handle, ControlError> {
        let path = path_wide(path)?;
        // Share no delete/write access: retain each checked directory until the
        // final gate lookup completes. Do not follow a terminal reparse point.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                READ_CONTROL | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            let last = unsafe { GetLastError() };
            return Err(error(
                "windows_oem_maintenance_parent",
                "Cannot open a maintenance parent directory",
                last,
            ));
        }
        let handle = Handle(handle);
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0 {
            let last = unsafe { GetLastError() };
            return Err(error(
                "windows_oem_maintenance_parent",
                "Cannot inspect a maintenance parent directory",
                last,
            ));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(refused(
                "windows_oem_maintenance_parent",
                "A maintenance parent is not a plain directory",
            ));
        }
        if protected {
            verify(handle.0, SE_FILE_OBJECT, Policy::GateDirectory).map_err(|mut error| {
                error.code = "windows_oem_maintenance_parent_untrusted".into();
                error
            })?;
        }
        Ok(handle)
    }

    // Called only while the fixed writer mutex is held. Installer/repair uses
    // the same mutex. Any existing gate, error or missing/untrusted parent is
    // a rejection; this helper never creates, interprets or deletes the gate.
    fn check_fixed_maintenance_gate() -> Result<(), ControlError> {
        let root = program_data()?;
        let _root = directory(&root, false)?;
        let brand = root.join("Mechrevo");
        let _brand = directory(&brand, true)?;
        let console = brand.join("Console");
        let _console = directory(&console, true)?;
        let gate = path_wide(&console.join("maintenance.json"))?;
        let handle = unsafe {
            CreateFileW(
                gate.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                ptr::null_mut(),
            )
        };
        let absent = if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            let last = unsafe { GetLastError() };
            if last != ERROR_FILE_NOT_FOUND {
                return Err(error(
                    "windows_oem_maintenance_unknown",
                    "Cannot establish that maintenance is absent",
                    last,
                ));
            }
            true
        } else {
            drop(Handle(handle));
            false
        };
        if gate_allows_ec(true, absent) {
            Ok(())
        } else {
            Err(refused(
                "windows_oem_maintenance_active",
                "Installation/repair maintenance is pending; EC access is blocked",
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trusted(policy: Policy) -> Descriptor {
        let (mask, flags) = match policy {
            Policy::Mutex => (MUTEX_RIGHTS, 0),
            Policy::GateDirectory => (DIRECTORY_RIGHTS, 3),
        };
        Descriptor {
            owner: Principal::Administrators,
            group: Principal::Administrators,
            owner_defaulted: false,
            group_defaulted: false,
            protected: true,
            dacl_present: true,
            dacl_defaulted: false,
            grants: vec![
                Grant {
                    allow: true,
                    flags,
                    mask,
                    principal: Principal::System,
                },
                Grant {
                    allow: true,
                    flags,
                    mask,
                    principal: Principal::Administrators,
                },
            ],
        }
    }

    #[test]
    fn fixed_namespace_and_rights_do_not_advertise_a_global_oem_lock() {
        assert_eq!(NAME, "Global\\MechrevoConsole.Oem.v1.FnLock");
        assert_eq!(MUTEX_RIGHTS, 0x100000 | 0x20000 | 1);
        assert_eq!(
            MUTEX_SDDL,
            "O:BAG:BAD:P(A;;0x00120001;;;SY)(A;;0x00120001;;;BA)"
        );
        assert!(validate_descriptor(&trusted(Policy::Mutex), Policy::Mutex).is_ok());
        assert!(
            validate_descriptor(&trusted(Policy::GateDirectory), Policy::GateDirectory).is_ok()
        );
    }

    #[test]
    fn user_owned_or_user_group_objects_are_rejected_even_with_trusted_aces() {
        for owner in [true, false] {
            let mut value = trusted(Policy::Mutex);
            if owner {
                value.owner = Principal::Other;
            } else {
                value.group = Principal::Other;
            }
            assert!(validate_descriptor(&value, Policy::Mutex).is_err());
        }
        let mut system = trusted(Policy::Mutex);
        system.owner = Principal::System;
        system.group = Principal::System;
        assert!(validate_descriptor(&system, Policy::Mutex).is_ok());
    }

    #[test]
    fn missing_unprotected_and_defaulted_security_is_not_accepted() {
        for variant in 0..5 {
            let mut value = trusted(Policy::Mutex);
            match variant {
                0 => value.protected = false,
                1 => value.dacl_present = false,
                2 => value.dacl_defaulted = true,
                3 => value.owner_defaulted = true,
                _ => value.group_defaulted = true,
            }
            assert!(validate_descriptor(&value, Policy::Mutex).is_err());
        }
    }

    #[test]
    fn extra_missing_duplicate_and_unknown_trustees_never_pass() {
        for variant in 0..4 {
            let mut value = trusted(Policy::Mutex);
            match variant {
                0 => {
                    value.grants.pop();
                }
                1 => value.grants.push(value.grants[0]),
                2 => value.grants[1].principal = Principal::System,
                _ => value.grants[1].principal = Principal::Other,
            }
            assert!(validate_descriptor(&value, Policy::Mutex).is_err());
        }
    }

    #[test]
    fn wider_or_missing_rights_and_nonallow_aces_are_rejected() {
        for variant in 0..5 {
            let mut value = trusted(Policy::Mutex);
            match variant {
                0 => value.grants[0].mask |= 0x40000,
                1 => value.grants[0].mask &= !1,
                2 => value.grants[0].flags = 0x10,
                3 => value.grants[0].allow = false,
                _ => value.grants[0].mask = 0x10000000,
            }
            assert!(validate_descriptor(&value, Policy::Mutex).is_err());
        }
        assert!(validate_descriptor(&trusted(Policy::GateDirectory), Policy::Mutex).is_err());
        assert!(validate_descriptor(&trusted(Policy::Mutex), Policy::GateDirectory).is_err());
    }

    #[test]
    fn maintenance_directory_must_use_the_exact_installer_acl() {
        for flags in [0, 1, 2, 0x13] {
            let mut value = trusted(Policy::GateDirectory);
            value.grants[0].flags = flags;
            assert!(validate_descriptor(&value, Policy::GateDirectory).is_err());
        }
        let mut value = trusted(Policy::GateDirectory);
        value.grants[1].mask &= !0x10000; // missing delete is not the fixed installer descriptor
        assert!(validate_descriptor(&value, Policy::GateDirectory).is_err());
        value = trusted(Policy::GateDirectory);
        value.grants.push(Grant {
            allow: true,
            flags: 3,
            mask: DIRECTORY_RIGHTS,
            principal: Principal::Other,
        });
        assert!(validate_descriptor(&value, Policy::GateDirectory).is_err());
    }

    #[test]
    fn a_successful_or_abandoned_wait_releases_exactly_once() {
        for result in [WaitResult::Acquired, WaitResult::Abandoned] {
            let mut state = Ownership::acquired(result);
            assert_eq!(state.abandoned, matches!(result, WaitResult::Abandoned));
            assert!(state.take_release());
            assert!(!state.take_release());
        }
        assert!(!Ownership::default().take_release());
    }

    #[test]
    fn maintenance_requires_both_trusted_parents_and_proven_absence() {
        for parents in [false, true] {
            for absent in [false, true] {
                assert_eq!(gate_allows_ec(parents, absent), parents && absent);
            }
        }
    }

    #[test]
    fn actual_ace_bytes_bound_the_sid_before_any_native_sid_function() {
        // ACCESS_ALLOWED_ACE prefix followed by the documented SID byte format;
        // native production offsets are obtained with offset_of! on SDK types.
        fn ace(count: u8) -> Vec<u8> {
            let mut bytes = vec![0; 8 + 8 + usize::from(count) * 4];
            let length = bytes.len() as u16;
            bytes[2..4].copy_from_slice(&length.to_le_bytes());
            bytes[4..8].copy_from_slice(&MUTEX_RIGHTS.to_le_bytes());
            bytes[8] = 1;
            bytes[9] = count;
            bytes[15] = 5;
            bytes
        }
        for count in [1, 2, 15] {
            let value = ace(count);
            assert_eq!(
                bounded_sid(&value, 8, 8, 1, 15).unwrap().len(),
                8 + usize::from(count) * 4
            );
        }
        for length in [8, 12, 15] {
            let value = ace(1);
            assert!(bounded_sid(&value[..length], 8, 8, 1, 15).is_err());
        }
        let mut value = ace(1);
        value[9] = 2;
        assert!(bounded_sid(&value, 8, 8, 1, 15).is_err());
        value[9] = 255;
        assert!(bounded_sid(&value, 8, 8, 1, 15).is_err());
        value = ace(2);
        value.push(0);
        assert!(bounded_sid(&value, 8, 8, 1, 15).is_err());
        assert!(bounded_sid(&value, value.len() + 1, 8, 1, 15).is_err());
        assert!(bounded_sid(&value, 8, 8, 8, 15).is_err());
    }
}
