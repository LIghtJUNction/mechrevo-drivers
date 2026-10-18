//! Fixed local, one-shot Windows OEM broker. A response and a reaped process
//! are separate facts. Pending OVERLAPPED allocations survive failed waits.
use super::super::{
    client::TransportReport,
    ipc_policy::{
        self, frame_length, launch_decision, release_after_completion, unowned_lifetime,
        valid_nonce, LaunchDecision, Reservation,
    },
    protocol::{self, Request},
    ClientHelperLifetime,
};
use crate::ControlError;
use std::{
    ffi::OsStr,
    fs::File,
    io::{Read, Write},
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
};
use windows_sys::Win32::{
    Foundation::{
        GetLastError, LocalFree, ERROR_BROKEN_PIPE, ERROR_CANCELLED, ERROR_IO_INCOMPLETE,
        ERROR_IO_PENDING, ERROR_NOT_FOUND, ERROR_PIPE_CONNECTED, FILETIME, HANDLE,
        INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0,
    },
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            SDDL_REVISION_1,
        },
        Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG},
        GetTokenInformation, TokenElevation, TokenSessionId, TokenUser, SECURITY_ATTRIBUTES,
        TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER,
    },
    Storage::FileSystem::{
        CreateFileW, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED,
        FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_WRITE_DATA, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
        SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT,
    },
    System::{
        Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE},
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeClientProcessId,
            GetNamedPipeClientSessionId, GetNamedPipeServerProcessId, GetNamedPipeServerSessionId,
            PeekNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE,
            PIPE_WAIT,
        },
        RemoteDesktop::ProcessIdToSessionId,
        Threading::{
            CreateEventW, GetCurrentProcess, GetCurrentProcessId, GetExitCodeProcess, GetProcessId,
            GetProcessTimes, OpenProcess, OpenProcessToken, WaitForMultipleObjects,
            WaitForSingleObject, INFINITE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        },
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
    },
    UI::{
        Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
        WindowsAndMessaging::SW_HIDE,
    },
};

const REQUEST_LIMIT: usize = protocol::MAX_REQUEST_BYTES;
const RESPONSE_LIMIT: usize = protocol::MAX_RESPONSE_BYTES;
const HELPER: &str = "mechrevo-console-oem-helper.exe";
const CALLERS: [&str; 2] = ["mechrevo-console.exe", "mechrevo-console-gui.exe"];
// No FILE_APPEND_DATA / FILE_CREATE_PIPE_INSTANCE, GENERIC_WRITE, WRITE_DAC or
// WRITE_OWNER is granted to a connecting client, including BA/SY clients.
const CLIENT_ACCESS: u32 = ipc_policy::PIPE_CLIENT_ACCESS;
// Check the portable policy against this SDK's concrete Win32 access bits.
const _: () =
    assert!(CLIENT_ACCESS == FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | 0x0010_0000);

fn rejected(message: impl Into<String>) -> ControlError {
    ControlError::new("windows_oem_ipc", message)
}
fn win_error(action: &str, code: u32) -> ControlError {
    let mut error = rejected(action);
    error.details.push(format!("Win32={code}"));
    error
}
fn last_error(action: &str) -> ControlError {
    win_error(action, unsafe { GetLastError() })
}
fn owned(handle: HANDLE, action: &str) -> Result<OwnedHandle, ControlError> {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return Err(last_error(action));
    }
    // SAFETY: this successful Win32 API returned one owned, non-inheritable handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}
fn raw(handle: &OwnedHandle) -> HANDLE {
    handle.as_raw_handle()
}
fn wide(value: &OsStr) -> Result<Vec<u16>, ControlError> {
    let value: Vec<_> = value.encode_wide().collect();
    if value.is_empty() || value.len() >= 32_767 || value.contains(&0) {
        return Err(rejected("Invalid fixed Windows string"));
    }
    Ok(value.into_iter().chain(Some(0)).collect())
}
fn creation_time(process: HANDLE) -> Result<u64, ControlError> {
    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    if unsafe { GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return Err(last_error(
            "Cannot obtain the authenticated process creation time",
        ));
    }
    Ok((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
}
fn session_id(pid: u32) -> Result<u32, ControlError> {
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(pid, &mut session) } == 0 {
        return Err(last_error("Cannot obtain process session"));
    }
    Ok(session)
}
fn process_token() -> Result<OwnedHandle, ControlError> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error("Cannot query this process token"));
    }
    owned(token, "Invalid current process token")
}
fn parent_sid() -> Result<String, ControlError> {
    let token = process_token()?;
    let mut needed = 0;
    unsafe {
        GetTokenInformation(raw(&token), TokenUser, ptr::null_mut(), 0, &mut needed);
    }
    if needed < size_of::<TOKEN_USER>() as u32 || needed > 65_536 {
        return Err(rejected("Invalid token user buffer size"));
    }
    // usize storage gives TOKEN_USER its required pointer alignment.
    let mut buffer = vec![0usize; (needed as usize).div_ceil(size_of::<usize>())];
    if unsafe {
        GetTokenInformation(
            raw(&token),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(last_error("Cannot obtain this process SID"));
    }
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut string = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut string) } == 0 {
        return Err(last_error("Cannot encode this process SID"));
    }
    let result = (|| {
        let mut length = 0;
        while length < 256 && unsafe { *string.add(length) } != 0 {
            length += 1;
        }
        if length == 256 {
            return Err(rejected("Unexpected SID string length"));
        }
        String::from_utf16(unsafe { std::slice::from_raw_parts(string, length) })
            .map_err(|_| rejected("Invalid SID string"))
    })();
    unsafe {
        LocalFree(string.cast());
    }
    result
}
fn require_elevated_helper() -> Result<(), ControlError> {
    let token = process_token()?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut needed = 0;
    if unsafe {
        GetTokenInformation(
            raw(&token),
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut needed,
        )
    } == 0
        || needed as usize != size_of::<TOKEN_ELEVATION>()
        || elevation.TokenIsElevated == 0
    {
        return Err(rejected(
            "The fixed OEM helper requires its own elevated token",
        ));
    }
    let mut token_session = 0u32;
    if unsafe {
        GetTokenInformation(
            raw(&token),
            TokenSessionId,
            (&mut token_session as *mut u32).cast(),
            size_of::<u32>() as u32,
            &mut needed,
        )
    } == 0
        || needed as usize != size_of::<u32>()
        || token_session != session_id(unsafe { GetCurrentProcessId() })?
    {
        return Err(rejected(
            "The helper token and process session do not agree",
        ));
    }
    Ok(())
}
fn nonce() -> Result<String, ControlError> {
    let mut bytes = [0u8; 32];
    let status = unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status < 0 {
        return Err(rejected(format!(
            "Windows CSPRNG failed: NTSTATUS=0x{:08x}",
            status as u32
        )));
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn pipe_name(nonce: &str) -> Result<Vec<u16>, ControlError> {
    wide(OsStr::new(&ipc_policy::pipe_name(nonce)?))
}
fn create_pipe(nonce: &str) -> Result<OwnedHandle, ControlError> {
    let name = pipe_name(nonce)?;
    let sid = parent_sid()?;
    let sddl = wide(OsStr::new(&format!("D:P(A;;0x{CLIENT_ACCESS:08x};;;{sid})(A;;0x{CLIENT_ACCESS:08x};;;BA)(A;;0x{CLIENT_ACCESS:08x};;;SY)")))?;
    let mut descriptor = ptr::null_mut();
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(last_error("Cannot build the explicit local pipe DACL"));
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            RESPONSE_LIMIT as u32 + 4,
            REQUEST_LIMIT as u32 + 4,
            0,
            &attributes,
        )
    };
    // Capture GetLastError before LocalFree can replace the thread error.
    let error = unsafe { GetLastError() };
    unsafe {
        LocalFree(descriptor);
    }
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(win_error(
            "Cannot create the first local pipe instance",
            error,
        ));
    }
    owned(handle, "Invalid newly created pipe")
}

enum IoKind {
    Connect,
    Read(usize),
    Write(Vec<u8>),
}
struct PendingIo {
    overlapped: Box<OVERLAPPED>,
    event: OwnedHandle,
    buffer: Vec<u8>,
    completed: Option<Result<usize, u32>>,
    in_flight: bool,
}
// SAFETY: the OVERLAPPED and buffer allocations remain fixed, owned exclusively
// by one exchange. Moving the owner between worker threads does not move them.
unsafe impl Send for PendingIo {}
impl PendingIo {
    fn begin(pipe: HANDLE, kind: IoKind) -> Result<Self, ControlError> {
        let event = owned(
            unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) },
            "Cannot create the pipe I/O event",
        )?;
        let mut overlapped = Box::<OVERLAPPED>::default();
        overlapped.hEvent = raw(&event);
        let (mut buffer, write, connect) = match kind {
            IoKind::Connect => (vec![], false, true),
            IoKind::Read(length) => (vec![0; length], false, false),
            IoKind::Write(bytes) => (bytes, true, false),
        };
        let mut transferred = 0;
        let ok = unsafe {
            if connect {
                ConnectNamedPipe(pipe, &mut *overlapped)
            } else if write {
                WriteFile(
                    pipe,
                    buffer.as_ptr(),
                    buffer.len() as u32,
                    &mut transferred,
                    &mut *overlapped,
                )
            } else {
                ReadFile(
                    pipe,
                    buffer.as_mut_ptr(),
                    buffer.len() as u32,
                    &mut transferred,
                    &mut *overlapped,
                )
            }
        };
        let error = if ok == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        let completed = if ok != 0 || (connect && error == ERROR_PIPE_CONNECTED) {
            Some(Ok(transferred as usize))
        } else if error == ERROR_IO_PENDING {
            None
        } else {
            Some(Err(error))
        };
        Ok(Self {
            overlapped,
            event,
            buffer,
            completed,
            in_flight: completed.is_none(),
        })
    }
    fn wait(
        &mut self,
        pipe: HANDLE,
        process: Option<HANDLE>,
    ) -> Result<Result<usize, u32>, ControlError> {
        if let Some(result) = self.completed {
            return Ok(result);
        }
        let mut observed_event = false;
        loop {
            let mut count = 0;
            if unsafe { GetOverlappedResult(pipe, &*self.overlapped, &mut count, 0) } != 0 {
                self.in_flight = false;
                self.completed = Some(Ok(count as usize));
                return Ok(Ok(count as usize));
            }
            let error = unsafe { GetLastError() };
            if error != ERROR_IO_INCOMPLETE {
                // A failed result query can itself be INVALID_HANDLE/PARAMETER.
                // Only this operation's non-inherited, initially-unsignalled
                // event proves completion when the query did not succeed.
                let event = unsafe { WaitForSingleObject(raw(&self.event), 0) };
                if event != WAIT_OBJECT_0 {
                    return Err(win_error("Pipe result query failed without terminal completion evidence; I/O retained", error));
                }
                self.in_flight = false;
                self.completed = Some(Err(error));
                return Ok(Err(error));
            }
            if observed_event {
                return Err(rejected(
                    "Signalled pipe event still reports incomplete I/O; allocation retained",
                ));
            }
            let wait = if let Some(process) = process {
                unsafe {
                    WaitForMultipleObjects(2, [raw(&self.event), process].as_ptr(), 0, INFINITE)
                }
            } else {
                unsafe { WaitForSingleObject(raw(&self.event), INFINITE) }
            };
            if wait == WAIT_OBJECT_0 {
                observed_event = true;
                continue;
            }
            if process.is_some() && wait == WAIT_OBJECT_0 + 1 {
                // The helper has actually exited. Cancel only our outstanding
                // pipe operation, never a live hardware transaction or process.
                if unsafe { CancelIoEx(pipe, &*self.overlapped) } == 0 {
                    let error = unsafe { GetLastError() };
                    if error != ERROR_NOT_FOUND {
                        return Err(win_error("Cannot drain pipe I/O after helper exit", error));
                    }
                }
                if unsafe { WaitForSingleObject(raw(&self.event), INFINITE) } != WAIT_OBJECT_0 {
                    return Err(last_error(
                        "Pipe I/O completion remains unknown after helper exit",
                    ));
                }
                // Process is already signalled; wait only for this I/O from now on.
                return self.wait(pipe, None);
            }
            return Err(if wait == WAIT_FAILED {
                last_error("Cannot wait for pipe completion")
            } else {
                rejected("Unexpected wait status; pipe I/O lifetime remains unknown")
            });
        }
    }
    fn cancel_and_drain(&mut self, pipe: HANDLE) -> Result<(), ControlError> {
        if self.in_flight && unsafe { CancelIoEx(pipe, &*self.overlapped) } == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_NOT_FOUND {
                return Err(win_error("Cannot cancel the local pipe operation", error));
            }
        }
        // The final error (including OPERATION_ABORTED) is a completed I/O, not
        // proof of successful hardware work. Keep the original transaction error.
        let _ = self.wait(pipe, None)?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Stage {
    Connect,
    Send,
    Header,
    Body(usize),
    Trailing,
    Wait,
    UntrackedLaunch,
}
struct PendingExchange {
    installation: install::Installation,
    pipe: Option<OwnedHandle>,
    process: Option<OwnedHandle>,
    process_created: Option<u64>,
    io: Option<PendingIo>,
    stage: Stage,
    outgoing: Vec<u8>,
    sent: usize,
    header: Vec<u8>,
    body: Vec<u8>,
    reply: Option<Vec<u8>>,
    error: Option<ControlError>,
    cancelled_before_start: bool,
}
impl PendingExchange {
    fn fail(&mut self, error: ControlError) {
        if let Some(primary) = &mut self.error {
            if primary.details.len() < 15 {
                primary
                    .details
                    .push(format!("{}: {}", error.code, error.message));
            } else if primary.details.len() == 15 {
                primary.details.push(
                    "Further repeated IPC errors omitted; retained helper ownership is unchanged"
                        .into(),
                );
            }
        } else {
            self.error = Some(error);
        }
    }
    fn unknown(&mut self, error: ControlError) -> TransportReport {
        self.fail(error);
        TransportReport {
            reply: self.reply.clone(),
            exit_code: None,
            lifetime: ClientHelperLifetime::Unknown,
            error: self.error.clone(),
        }
    }
    fn pipe_raw(&self) -> HANDLE {
        raw(self.pipe.as_ref().expect("live exchange pipe"))
    }
    fn close_pipe(&mut self) -> Result<(), ControlError> {
        if let Some(io) = &mut self.io {
            let pipe = raw(self.pipe.as_ref().expect("pending I/O owns its pipe"));
            io.cancel_and_drain(pipe)?;
        }
        self.io = None;
        // Closing the channel makes a helper blocked sending a reply observe a
        // broken pipe. We never stop reading and then wait with a full pipe.
        self.pipe = None;
        Ok(())
    }
    fn drive(&mut self) -> TransportReport {
        loop {
            if self.error.is_some() && !matches!(self.stage, Stage::Wait | Stage::UntrackedLaunch) {
                if let Err(error) = self.close_pipe() {
                    return self.unknown(error);
                }
                self.stage = if self.process.is_some() || self.cancelled_before_start {
                    Stage::Wait
                } else {
                    Stage::UntrackedLaunch
                };
            }
            match self.stage {
                Stage::Wait => {
                    let Some(process) = self.process.as_ref() else {
                        return TransportReport {
                            reply: self.reply.take(),
                            exit_code: None,
                            lifetime: ClientHelperLifetime::NotStarted,
                            error: self.error.take(),
                        };
                    };
                    if unsafe { WaitForSingleObject(raw(process), INFINITE) } != WAIT_OBJECT_0 {
                        return self.unknown(last_error("The actual helper exit is not confirmed"));
                    }
                    let mut exit = 0;
                    let exit_code = if unsafe { GetExitCodeProcess(raw(process), &mut exit) } != 0 {
                        Some(exit)
                    } else {
                        self.fail(last_error(
                            "Helper exited but its exit code could not be read",
                        ));
                        None
                    };
                    return TransportReport {
                        reply: self.reply.take(),
                        exit_code,
                        lifetime: ClientHelperLifetime::Reaped,
                        error: self.error.take(),
                    };
                }
                Stage::UntrackedLaunch => {
                    return TransportReport {
                        reply: self.reply.clone(),
                        exit_code: None,
                        lifetime: ClientHelperLifetime::Unknown,
                        error: self.error.clone().or_else(|| {
                            Some(rejected(
                                "Shell launch returned no trackable process handle",
                            ))
                        }),
                    }
                }
                _ => {}
            }
            if self.io.is_none() {
                let kind = match self.stage {
                    Stage::Connect => IoKind::Connect,
                    Stage::Send => IoKind::Write(self.outgoing[self.sent..].to_vec()),
                    Stage::Header => IoKind::Read(4 - self.header.len()),
                    Stage::Body(length) => IoKind::Read(length - self.body.len()),
                    Stage::Trailing => IoKind::Read(1),
                    _ => unreachable!(),
                };
                match PendingIo::begin(self.pipe_raw(), kind) {
                    Ok(io) => self.io = Some(io),
                    Err(error) => {
                        self.fail(error);
                        continue;
                    }
                }
            }
            let pipe = self.pipe_raw();
            let process = self.process.as_ref().map(raw);
            let completion = self
                .io
                .as_mut()
                .expect("issued operation")
                .wait(pipe, process);
            let completion = match completion {
                Ok(completion) => completion,
                Err(error) => return self.unknown(error),
            };
            let io = self.io.take().expect("completed operation");
            if matches!(self.stage, Stage::Trailing)
                && (completion == Ok(0) || completion == Err(ERROR_BROKEN_PIPE))
            {
                self.reply = Some(std::mem::take(&mut self.body));
                self.pipe = None;
                self.stage = Stage::Wait;
                continue;
            }
            let count = match completion {
                Ok(count) => count,
                Err(code) => {
                    self.fail(win_error("Fixed pipe exchange failed", code));
                    continue;
                }
            };
            if !matches!(self.stage, Stage::Connect) && (count == 0 || count > io.buffer.len()) {
                self.fail(rejected("Truncated pipe frame or invalid I/O byte count"));
                continue;
            }
            match self.stage {
                Stage::Connect => {
                    let authenticated = self.authenticate_helper();
                    if let Err(error) = authenticated {
                        self.fail(error);
                        continue;
                    }
                    self.stage = Stage::Send;
                }
                Stage::Send => {
                    self.sent += count;
                    if self.sent == self.outgoing.len() {
                        self.stage = Stage::Header;
                    }
                }
                Stage::Header => {
                    self.header.extend_from_slice(&io.buffer[..count]);
                    if self.header.len() == 4 {
                        match frame_length(
                            self.header.as_slice().try_into().expect("four-byte prefix"),
                            RESPONSE_LIMIT,
                        ) {
                            Ok(length) => self.stage = Stage::Body(length),
                            Err(error) => self.fail(error),
                        }
                    }
                }
                Stage::Body(length) => {
                    self.body.extend_from_slice(&io.buffer[..count]);
                    if self.body.len() == length {
                        self.stage = Stage::Trailing;
                    }
                }
                Stage::Trailing => self.fail(rejected(
                    "The helper sent trailing data after its one response",
                )),
                _ => unreachable!(),
            }
        }
    }
    fn authenticate_helper(&self) -> Result<(), ControlError> {
        let process = self
            .process
            .as_ref()
            .ok_or_else(|| rejected("No actual launched helper handle"))?;
        let mut peer = 0;
        let mut peer_session = 0;
        if unsafe { GetNamedPipeClientProcessId(self.pipe_raw(), &mut peer) } == 0
            || unsafe { GetNamedPipeClientSessionId(self.pipe_raw(), &mut peer_session) } == 0
        {
            return Err(last_error("Cannot authenticate pipe client PID/session"));
        }
        if peer == 0
            || peer != unsafe { GetProcessId(raw(process)) }
            || Some(creation_time(raw(process))?) != self.process_created
            || peer_session != session_id(peer)?
            || peer_session != session_id(unsafe { GetCurrentProcessId() })?
        {
            return Err(rejected(
                "Pipe peer differs from the launched helper process identity/session",
            ));
        }
        // Do not query an alternate administrator's token from an ordinary
        // parent. The fixed verified helper checks its own elevation before I/O.
        self.installation
            .authenticate_image(raw(process), &[HELPER])
    }
}

// A dropped client must not release live kernel I/O allocations or forget the
// writer gate. New transports see this retained exchange until it is reaped.
static ORPHANED: OnceLock<Mutex<Vec<PendingExchange>>> = OnceLock::new();
fn orphaned() -> &'static Mutex<Vec<PendingExchange>> {
    ORPHANED.get_or_init(|| Mutex::new(Vec::new()))
}
static ACTIVE: AtomicBool = AtomicBool::new(false);
pub(crate) struct NativeTransport {
    pending: Option<PendingExchange>,
}
impl Default for NativeTransport {
    fn default() -> Self {
        Self::new()
    }
}
impl NativeTransport {
    pub(crate) fn new() -> Self {
        Self { pending: None }
    }
    pub(crate) fn helper_reaped(&self) -> bool {
        self.pending.is_none()
            && !ACTIVE.load(Ordering::Acquire)
            && orphaned().lock().is_ok_and(|pending| pending.is_empty())
    }
    pub(crate) fn exchange(&mut self, request: &Request) -> TransportReport {
        if self.pending.is_some() {
            return TransportReport {
                reply: None,
                exit_code: None,
                lifetime: ClientHelperLifetime::Unknown,
                error: Some(rejected(
                    "A prior helper still requires drain/wait; no new request was launched",
                )),
            };
        }
        let Some(mut reservation) = Reservation::acquire(&ACTIVE) else {
            return TransportReport {
                reply: None,
                exit_code: None,
                lifetime: ClientHelperLifetime::Unknown,
                error: Some(rejected(
                    "Another live or retained helper owns the process-wide launch reservation",
                )),
            };
        };
        if !orphaned().lock().is_ok_and(|pending| pending.is_empty()) {
            // This cannot occur with an intact reservation invariant. Keep the
            // gate occupied rather than hide a poisoned/retained owner.
            reservation.transfer();
            return TransportReport {
                reply: None,
                exit_code: None,
                lifetime: ClientHelperLifetime::Unknown,
                error: Some(rejected("Retained helper ownership cannot be proven empty")),
            };
        }
        let prepared = (|| {
            let bytes = protocol::encode_request(request)?;
            if bytes.is_empty() || bytes.len() > REQUEST_LIMIT {
                return Err(rejected("Invalid fixed request frame size"));
            }
            let installation = install::Installation::open()?;
            installation.authenticate_image(unsafe { GetCurrentProcess() }, &CALLERS)?;
            let nonce = nonce()?;
            let pipe = create_pipe(&nonce)?;
            let mut outgoing = (bytes.len() as u32).to_le_bytes().to_vec();
            outgoing.extend_from_slice(&bytes);
            Ok((
                nonce,
                PendingExchange {
                    installation,
                    pipe: Some(pipe),
                    process: None,
                    process_created: None,
                    io: None,
                    stage: Stage::Connect,
                    outgoing,
                    sent: 0,
                    header: vec![],
                    body: vec![],
                    reply: None,
                    error: None,
                    cancelled_before_start: false,
                },
            ))
        })();
        let (nonce, pending) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                return TransportReport {
                    reply: None,
                    exit_code: None,
                    lifetime: ClientHelperLifetime::NotStarted,
                    error: Some(error),
                }
            }
        };
        self.pending = Some(pending);
        // From here the PendingExchange, including Drop/orphan adoption, owns
        // this reservation until actual Reaped/known NotStarted.
        reservation.transfer();
        let pending = self.pending.as_mut().expect("prepared exchange retained");
        let launch = launch_helper(pending.installation.helper_path(), &nonce);
        match launch {
            Ok((process, launch_error)) => {
                match creation_time(raw(&process)) {
                    Ok(time) => pending.process_created = Some(time),
                    Err(error) => pending.fail(error),
                }
                pending.process = Some(process);
                if let Some(error) = launch_error {
                    pending.fail(error);
                }
            }
            Err((error, cancelled)) => {
                pending.cancelled_before_start = cancelled;
                pending.fail(error);
            }
        }
        self.finish_pending()
    }
    pub(crate) fn finish_pending(&mut self) -> TransportReport {
        if self.pending.is_none() {
            match orphaned().lock() {
                Ok(mut pending) if !pending.is_empty() => self.pending = Some(pending.remove(0)),
                Ok(_) => {
                    let lifetime = unowned_lifetime(ACTIVE.load(Ordering::Acquire));
                    return TransportReport { reply: None, exit_code: None, lifetime,
                        error: (lifetime == ClientHelperLifetime::Unknown).then(|| rejected("Another transport still owns an active helper; its exit is not confirmed")) };
                }
                Err(_) => {
                    return TransportReport {
                        reply: None,
                        exit_code: None,
                        lifetime: ClientHelperLifetime::Unknown,
                        error: Some(rejected("Retained helper ownership lock is poisoned")),
                    }
                }
            }
        }
        let report = self
            .pending
            .as_mut()
            .expect("pending exchange retained")
            .drive();
        if release_after_completion(&ACTIVE, report.lifetime) {
            self.pending = None;
        }
        report
    }
}
impl Drop for NativeTransport {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.take() {
            match orphaned().lock() {
                Ok(mut retained) => retained.push(pending),
                // Do not free possibly live OVERLAPPED/buffer memory even if a
                // prior panic poisoned the retention gate. No success is claimed.
                Err(_) => std::mem::forget(pending),
            }
        }
    }
}
fn launch_helper(
    path: &std::path::Path,
    nonce: &str,
) -> Result<(OwnedHandle, Option<ControlError>), (ControlError, bool)> {
    let pid = unsafe { GetCurrentProcessId() };
    let created = creation_time(unsafe { GetCurrentProcess() }).map_err(|error| (error, true))?;
    let executable = wide(path.as_os_str()).map_err(|error| (error, true))?;
    let parameters = wide(OsStr::new(&format!("--serve {nonce} {pid} {created}")))
        .map_err(|error| (error, true))?;
    let runas = wide(OsStr::new("runas")).map_err(|error| (error, true))?;
    let initialized = unsafe {
        CoInitializeEx(
            ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    if initialized < 0 {
        return Err((
            rejected(format!(
                "Cannot initialize the Shell launch apartment: HRESULT=0x{:08x}",
                initialized as u32
            )),
            true,
        ));
    }
    let mut launch = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: runas.as_ptr(),
        lpFile: executable.as_ptr(),
        lpParameters: parameters.as_ptr(),
        nShow: SW_HIDE,
        ..SHELLEXECUTEINFOW::default()
    };
    let ok = unsafe { ShellExecuteExW(&mut launch) };
    let error = unsafe { GetLastError() };
    unsafe {
        CoUninitialize();
    }
    // Shell launch is synchronous in this worker, including late UAC response.
    // NOASYNC does not mean the helper process has exited.
    match launch_decision(
        ok != 0,
        !launch.hProcess.is_null() && launch.hProcess != INVALID_HANDLE_VALUE,
        error == ERROR_CANCELLED,
    ) {
        LaunchDecision::Tracked | LaunchDecision::TrackedFailure => {
            let process = owned(launch.hProcess, "Invalid Shell helper handle")
                .map_err(|error| (error, false))?;
            Ok((process, (ok == 0).then(|| win_error("Shell launch failed despite returning a process handle; no request was sent", error))))
        }
        LaunchDecision::CancelledBeforeStart => Err((
            win_error("The UAC request was cancelled before launch", error),
            true,
        )),
        LaunchDecision::Untracked => Err((
            if ok == 0 {
                win_error(
                    "The fixed UAC helper launch failed without a trackable handle",
                    error,
                )
            } else {
                rejected("Shell reported launch success without an actual process handle")
            },
            false,
        )),
    }
}

/// Authenticate first, read one bounded request, finish its handler even when
/// the parent disappears, then send one response. No stdout redirection is used.
pub(crate) fn serve_authenticated(
    nonce: &str,
    parent_pid: u32,
    parent_created: u64,
    handler: impl FnOnce(&[u8]) -> Result<(Vec<u8>, u32), ControlError>,
) -> Result<u32, ControlError> {
    if !valid_nonce(nonce) || parent_pid == 0 || parent_created == 0 {
        return Err(rejected("Invalid fixed helper launch association"));
    }
    require_elevated_helper()?;
    let installation = install::Installation::open()?;
    installation.authenticate_image(unsafe { GetCurrentProcess() }, &[HELPER])?;
    let parent = owned(
        unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                parent_pid,
            )
        },
        "Cannot open the declared parent process",
    )?;
    if creation_time(raw(&parent))? != parent_created {
        return Err(rejected(
            "Declared parent creation time does not match its actual process",
        ));
    }
    installation.authenticate_image(raw(&parent), &CALLERS)?;
    let name = pipe_name(nonce)?;
    let pipe = owned(
        unsafe {
            CreateFileW(
                name.as_ptr(),
                CLIENT_ACCESS,
                0,
                ptr::null(),
                OPEN_EXISTING,
                SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                ptr::null_mut(),
            )
        },
        "Cannot connect to the fixed local parent pipe",
    )?;
    let mut peer_pid = 0;
    let mut peer_session = 0;
    if unsafe { GetNamedPipeServerProcessId(raw(&pipe), &mut peer_pid) } == 0
        || unsafe { GetNamedPipeServerSessionId(raw(&pipe), &mut peer_session) } == 0
    {
        return Err(last_error(
            "Cannot authenticate the pipe server PID/session",
        ));
    }
    if peer_pid != parent_pid
        || peer_session != session_id(parent_pid)?
        || peer_session != session_id(unsafe { GetCurrentProcessId() })?
        || creation_time(raw(&parent))? != parent_created
        || unsafe { WaitForSingleObject(raw(&parent), 0) }
            != windows_sys::Win32::Foundation::WAIT_TIMEOUT
    {
        return Err(rejected(
            "The pipe server is not the live authenticated parent in this session",
        ));
    }
    installation.authenticate_image(raw(&parent), &CALLERS)?;
    let mut channel = File::from(pipe);
    let mut prefix = [0u8; 4];
    channel
        .read_exact(&mut prefix)
        .map_err(|error| rejected(format!("Cannot receive request prefix: {error}")))?;
    let length = frame_length(prefix, REQUEST_LIMIT)?;
    let mut request = vec![0; length];
    channel
        .read_exact(&mut request)
        .map_err(|error| rejected(format!("Cannot receive complete request frame: {error}")))?;
    let mut queued = 0;
    if unsafe {
        PeekNamedPipe(
            channel.as_raw_handle(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            &mut queued,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(last_error("Cannot check the one-shot request frame"));
    }
    if queued != 0 {
        return Err(rejected("Trailing request bytes are not accepted"));
    }
    // The fixed handler performs strict protocol validation and all hardware
    // confirmation/restoration. Do not poll or kill it after submission.
    let (response, exit_code) = handler(&request)?;
    if response.is_empty() || response.len() > RESPONSE_LIMIT || !matches!(exit_code, 0 | 2) {
        return Err(rejected(
            "The fixed handler produced an invalid response frame/exit code",
        ));
    }
    channel
        .write_all(&(response.len() as u32).to_le_bytes())
        .and_then(|()| channel.write_all(&response))
        .map_err(|error| rejected(format!("Cannot send the terminal helper response: {error}")))?;
    // No server FlushFileBuffers: it can wait indefinitely for a vanished reader.
    drop(channel);
    drop(parent);
    drop(installation);
    Ok(exit_code)
}

mod install {
    use crate::{
        windows_oem::{
            ipc_policy::require_real_volume_root,
            release_manifest::{
                parse_manifest, parse_sha256, Manifest, Trust, IMAGE_NAMES, MAX_MANIFEST_BYTES,
            },
        },
        ControlError,
    };
    use sha2::{Digest, Sha256};
    use std::{
        ffi::{c_void, OsStr, OsString},
        fs::File,
        mem::size_of,
        os::windows::{
            ffi::{OsStrExt, OsStringExt},
            fs::FileExt,
            io::{AsRawHandle, FromRawHandle},
        },
        path::{Component, Path, PathBuf, Prefix},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{
            GetLastError, LocalFree, GENERIC_ALL, GENERIC_READ, GENERIC_WRITE, HANDLE,
            INVALID_HANDLE_VALUE,
        },
        Globalization::{CompareStringOrdinal, CSTR_EQUAL},
        Security::{
            Authorization::{ConvertStringSidToSidW, GetSecurityInfo, SE_FILE_OBJECT},
            EqualSid, GetAce, IsValidAcl, IsValidSecurityDescriptor, IsValidSid, ACE_HEADER, ACL,
            DACL_SECURITY_INFORMATION, INHERIT_ONLY_ACE, OWNER_SECURITY_INFORMATION,
            PSECURITY_DESCRIPTOR, PSID,
        },
        Storage::FileSystem::{
            CreateFileW, FileAttributeTagInfo, FileIdInfo, GetDriveTypeW,
            GetFileInformationByHandleEx, GetFinalPathNameByHandleW, DELETE, FILE_APPEND_DATA,
            FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
            FILE_DELETE_CHILD, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
            FILE_ID_INFO, FILE_SHARE_READ, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA,
            OPEN_EXISTING, VOLUME_NAME_NT, WRITE_DAC, WRITE_OWNER,
        },
        System::{
            Com::CoTaskMemFree,
            Threading::{GetCurrentProcess, QueryFullProcessImageNameW, PROCESS_NAME_NATIVE},
            WindowsProgramming::DRIVE_FIXED,
        },
        UI::Shell::{FOLDERID_ProgramFilesX64, SHGetKnownFolderPath},
    };
    const MANIFEST_NAME: &str = "console-release.json";
    const MAX_PATH_UNITS: usize = 32_768;
    const MAX_MANIFEST: u64 = MAX_MANIFEST_BYTES as u64;
    const MAX_IMAGE: u64 = 512 * 1024 * 1024;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct FileIdentity {
        volume: u64,
        id: [u8; 16],
    }
    struct HeldFile {
        file: File,
        identity: FileIdentity,
        native: Vec<u16>,
        path: PathBuf,
    }
    struct Image {
        name: &'static str,
        held: HeldFile,
    }
    pub(super) struct Installation {
        _ancestors: Vec<HeldFile>,
        _manifest_file: HeldFile,
        _manifest: Manifest,
        images: [Image; 3],
    }
    impl Installation {
        pub(super) fn open() -> Result<Self, ControlError> {
            if !cfg!(target_arch = "x86_64") {
                return Err(reject(
                    "This installation protocol only accepts x86_64 builds",
                ));
            }
            let program_files = program_files_x64()?;
            let mut components = program_files.components();
            let drive = match components.next() {
                Some(Component::Prefix(p)) => match p.kind() {
                    Prefix::Disk(drive) => drive,
                    _ => return Err(reject("ProgramFilesX64 must use a local disk path")),
                },
                _ => return Err(reject("ProgramFilesX64 has no local disk prefix")),
            };
            if !matches!(components.next(), Some(Component::RootDir)) {
                return Err(reject("ProgramFilesX64 is not an absolute disk path"));
            }
            let mut names = Vec::<OsString>::new();
            for component in components {
                match component {
                    Component::Normal(name)
                        if !name.encode_wide().any(|v| v == 0 || v == b':' as u16) =>
                    {
                        names.push(name.to_owned())
                    }
                    _ => return Err(reject("ProgramFilesX64 contains an unsafe path component")),
                }
            }
            if names.is_empty() {
                return Err(reject("ProgramFilesX64 cannot be the volume root"));
            }
            names.push(OsString::from("Mechrevo"));
            names.push(OsString::from("Console"));
            let mut logical = PathBuf::from(format!("{}:\\", char::from(drive)));
            if unsafe { GetDriveTypeW(wide_path(&logical)?.as_ptr()) } != DRIVE_FIXED {
                return Err(reject("ProgramFilesX64 must be on a fixed local drive"));
            }
            // The volume root may allow creation of unrelated directories. All
            // non-root ancestors and installation files have strict owner/ACL checks.
            let root = HeldFile::open(&logical, true, false)?;
            require_real_volume_root(&root.native)?;
            let mut parent_native = root.native.clone();
            let mut ancestors = vec![root];
            for name in names {
                logical.push(&name);
                let held = HeldFile::open(&logical, true, true)?;
                require_same_native(
                    &held.native,
                    &native_child(&parent_native, &name),
                    "installation ancestor",
                )?;
                parent_native = held.native.clone();
                ancestors.push(held);
            }
            let manifest_file = HeldFile::open(&logical.join(MANIFEST_NAME), false, true)?;
            require_same_native(
                &manifest_file.native,
                &native_child(&parent_native, OsStr::new(MANIFEST_NAME)),
                "release manifest",
            )?;
            let manifest = parse_manifest(
                &read_bounded(&manifest_file.file, MAX_MANIFEST)?,
                option_env!("MECHREVO_WINDOWS_RELEASE_ID"),
                option_env!("MECHREVO_WINDOWS_SOURCE_COMMIT"),
            )?;
            let hashes = [
                &manifest.files.cli,
                &manifest.files.gui,
                &manifest.files.helper,
            ];
            let mut opened = Vec::with_capacity(3);
            let mut signer: Option<[u8; 32]> = None;
            for (name, expected_hash) in IMAGE_NAMES.into_iter().zip(hashes) {
                let held = HeldFile::open(&logical.join(name), false, true)?;
                require_same_native(
                    &held.native,
                    &native_child(&parent_native, OsStr::new(name)),
                    "release executable",
                )?;
                if sha256_file(&held.file)? != parse_sha256(expected_hash)? {
                    return Err(reject(format!("Release SHA256 mismatch for {name}")));
                }
                match manifest.trust {
                    Trust::DevelopmentCandidate => { /* Protected unsigned candidate; never publisher trust. */
                    }
                    Trust::Authenticode => {
                        let current = verify_authenticode(&held.file, &held.path)?;
                        if signer.is_some_and(|first| first != current) {
                            return Err(reject("The three executables do not have the same verified signing certificate"));
                        }
                        signer = Some(current);
                    }
                }
                opened.push(Image { name, held });
            }
            let images: [Image; 3] = opened
                .try_into()
                .map_err(|_| reject("Internal image count mismatch"))?;
            let installation = Self {
                _ancestors: ancestors,
                _manifest_file: manifest_file,
                _manifest: manifest,
                images,
            };
            installation.authenticate_image(unsafe { GetCurrentProcess() }, &IMAGE_NAMES)?;
            Ok(installation)
        }
        pub(super) fn helper_path(&self) -> &Path {
            &self.images[2].held.path
        }
        pub(super) fn authenticate_image(
            &self,
            process: HANDLE,
            allowed: &[&str],
        ) -> Result<(), ControlError> {
            if process.is_null()
                || allowed.is_empty()
                || allowed.iter().any(|name| !IMAGE_NAMES.contains(name))
            {
                return Err(reject(
                    "Invalid process handle or executable role allowlist",
                ));
            }
            let native = process_native_image(process)?;
            let expected = self.images.iter().find(|image| allowed.contains(&image.name) && native_equal(&native, &image.held.native))
                .ok_or_else(|| reject("The process image is not an allowed executable in this verified installation"))?;
            let reopened = HeldFile::open(&global_path(&native)?, false, true)?;
            require_same_native(&reopened.native, &expected.held.native, "process image")?;
            if reopened.identity != expected.held.identity {
                return Err(reject(
                    "The process image file identity differs from the held installation executable",
                ));
            }
            Ok(())
        }
    }
    impl HeldFile {
        fn open(path: &Path, directory: bool, check_acl: bool) -> Result<Self, ControlError> {
            let wide = wide_path(path)?;
            let raw = unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    GENERIC_READ,
                    FILE_SHARE_READ,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                    ptr::null_mut(),
                )
            };
            if raw == INVALID_HANDLE_VALUE || raw.is_null() {
                return Err(win_failure("opening a protected installation object"));
            }
            let file = unsafe { File::from_raw_handle(raw) };
            let mut attrs = FILE_ATTRIBUTE_TAG_INFO::default();
            if unsafe {
                GetFileInformationByHandleEx(
                    file.as_raw_handle(),
                    FileAttributeTagInfo,
                    ptr::from_mut(&mut attrs).cast(),
                    size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
                )
            } == 0
            {
                return Err(win_failure("reading installation object attributes"));
            }
            if attrs.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || attrs.ReparseTag != 0 {
                return Err(reject("An installation path component is a reparse point"));
            }
            if (attrs.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory {
                return Err(reject("An installation object has the wrong file type"));
            }
            if check_acl {
                require_protected_acl(&file)?;
            }
            let mut info = FILE_ID_INFO::default();
            if unsafe {
                GetFileInformationByHandleEx(
                    file.as_raw_handle(),
                    FileIdInfo,
                    ptr::from_mut(&mut info).cast(),
                    size_of::<FILE_ID_INFO>() as u32,
                )
            } == 0
            {
                return Err(win_failure("reading installation file identity"));
            }
            let native = final_native_path(&file)?;
            let path = global_path(&native)?;
            Ok(Self {
                file,
                native,
                path,
                identity: FileIdentity {
                    volume: info.VolumeSerialNumber,
                    id: info.FileId.Identifier,
                },
            })
        }
    }
    fn reject(message: impl Into<String>) -> ControlError {
        ControlError::new("windows_oem_installation", message)
    }
    fn win_failure(stage: &str) -> ControlError {
        let code = unsafe { GetLastError() };
        reject(format!("{stage}: Win32 {code}"))
    }
    fn wide_path(path: &Path) -> Result<Vec<u16>, ControlError> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if wide.is_empty() || wide.len() >= MAX_PATH_UNITS || wide.contains(&0) {
            return Err(reject("Invalid or oversized installation path"));
        }
        wide.push(0);
        Ok(wide)
    }
    struct TaskMem(*mut c_void);
    impl Drop for TaskMem {
        fn drop(&mut self) {
            unsafe { CoTaskMemFree(self.0) };
        }
    }
    fn program_files_x64() -> Result<PathBuf, ControlError> {
        let mut raw = ptr::null_mut();
        let hr = unsafe {
            SHGetKnownFolderPath(&FOLDERID_ProgramFilesX64, 0, ptr::null_mut(), &mut raw)
        };
        let _allocation = TaskMem(raw.cast());
        if hr < 0 || raw.is_null() {
            return Err(reject(format!(
                "Cannot resolve ProgramFilesX64: HRESULT 0x{:08X}",
                hr as u32
            )));
        }
        let mut length = 0;
        while length < MAX_PATH_UNITS && unsafe { *raw.add(length) } != 0 {
            length += 1;
        }
        if length == 0 || length >= MAX_PATH_UNITS {
            return Err(reject("Invalid ProgramFilesX64 known-folder result"));
        }
        Ok(PathBuf::from(OsString::from_wide(unsafe {
            std::slice::from_raw_parts(raw, length)
        })))
    }
    fn final_native_path(file: &File) -> Result<Vec<u16>, ControlError> {
        let mut wide = vec![0u16; MAX_PATH_UNITS];
        let count = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                wide.as_mut_ptr(),
                wide.len() as u32,
                VOLUME_NAME_NT,
            )
        };
        if count == 0 {
            return Err(win_failure("resolving a held installation path"));
        }
        if count as usize >= wide.len() {
            return Err(reject("Held installation path exceeds the path bound"));
        }
        wide.truncate(count as usize);
        if wide.contains(&0) {
            return Err(reject("Held installation path contains an embedded NUL"));
        }
        Ok(wide)
    }
    fn process_native_image(process: HANDLE) -> Result<Vec<u16>, ControlError> {
        let mut wide = vec![0u16; MAX_PATH_UNITS];
        let mut count = wide.len() as u32;
        if unsafe {
            QueryFullProcessImageNameW(process, PROCESS_NAME_NATIVE, wide.as_mut_ptr(), &mut count)
        } == 0
        {
            return Err(win_failure("querying the peer process image"));
        }
        if count == 0 || count as usize >= wide.len() {
            return Err(reject("Invalid peer process image path length"));
        }
        wide.truncate(count as usize);
        if wide.contains(&0) {
            return Err(reject("Peer process image path contains an embedded NUL"));
        }
        Ok(wide)
    }
    fn native_child(parent: &[u16], name: &OsStr) -> Vec<u16> {
        let mut result = parent.to_vec();
        if result.last().copied() != Some(b'\\' as u16) {
            result.push(b'\\' as u16);
        }
        result.extend(name.encode_wide());
        result
    }
    fn global_path(native: &[u16]) -> Result<PathBuf, ControlError> {
        let device: Vec<u16> = "\\Device\\".encode_utf16().collect();
        if !native.starts_with(&device) || native.contains(&0) {
            return Err(reject("Expected a bound native device path"));
        }
        let mut wide: Vec<u16> = r"\\?\GLOBALROOT".encode_utf16().collect();
        wide.extend_from_slice(native);
        if wide.len() >= MAX_PATH_UNITS {
            return Err(reject("Bound GLOBALROOT path exceeds the path bound"));
        }
        Ok(PathBuf::from(OsString::from_wide(&wide)))
    }
    fn native_equal(a: &[u16], b: &[u16]) -> bool {
        !a.is_empty()
            && a.len() < MAX_PATH_UNITS
            && b.len() < MAX_PATH_UNITS
            && unsafe {
                CompareStringOrdinal(a.as_ptr(), a.len() as i32, b.as_ptr(), b.len() as i32, 1)
            } == CSTR_EQUAL
    }
    fn require_same_native(
        actual: &[u16],
        expected: &[u16],
        what: &str,
    ) -> Result<(), ControlError> {
        if native_equal(actual, expected) {
            Ok(())
        } else {
            Err(reject(format!(
                "The {what} resolved outside its held parent path"
            )))
        }
    }
    fn file_length(file: &File, maximum: u64) -> Result<u64, ControlError> {
        let size = file
            .metadata()
            .map_err(|e| reject(format!("Cannot read held file size: {e}")))?
            .len();
        if size == 0 || size > maximum {
            return Err(reject(
                "A release file is empty or exceeds the configured size bound",
            ));
        }
        Ok(size)
    }
    fn read_chunk(file: &File, buffer: &mut [u8], offset: u64) -> Result<usize, ControlError> {
        loop {
            match file.seek_read(buffer, offset) {
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(reject(format!("Cannot read held release file: {e}"))),
                Ok(count) => return Ok(count),
            }
        }
    }
    fn require_unchanged_length(file: &File, size: u64) -> Result<(), ControlError> {
        if file
            .metadata()
            .map_err(|e| reject(format!("Cannot recheck held file size: {e}")))?
            .len()
            != size
            || read_chunk(file, &mut [0u8; 1], size)? != 0
        {
            return Err(reject(
                "A held release file changed size during verification",
            ));
        }
        Ok(())
    }
    fn read_bounded(file: &File, maximum: u64) -> Result<Vec<u8>, ControlError> {
        let size = file_length(file, maximum)?;
        let mut data = vec![0u8; size as usize];
        let mut offset = 0usize;
        while offset < data.len() {
            let count = read_chunk(file, &mut data[offset..], offset as u64)?;
            if count == 0 {
                return Err(reject("Release manifest was truncated during verification"));
            }
            offset += count;
        }
        require_unchanged_length(file, size)?;
        Ok(data)
    }
    fn sha256_file(file: &File) -> Result<[u8; 32], ControlError> {
        let size = file_length(file, MAX_IMAGE)?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut offset = 0u64;
        while offset < size {
            let wanted = (size - offset).min(buffer.len() as u64) as usize;
            let count = read_chunk(file, &mut buffer[..wanted], offset)?;
            if count == 0 {
                return Err(reject(
                    "A release executable was truncated during verification",
                ));
            }
            hash.update(&buffer[..count]);
            offset += count as u64;
        }
        require_unchanged_length(file, size)?;
        Ok(hash.finalize().into())
    }
    struct LocalMem(*mut c_void);
    impl Drop for LocalMem {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { LocalFree(self.0) };
            }
        }
    }
    fn trusted_sid(text: &str) -> Result<LocalMem, ControlError> {
        let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        let mut sid = ptr::null_mut();
        if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut sid) } == 0 {
            return Err(win_failure("constructing a trusted security principal"));
        }
        let allocation = LocalMem(sid);
        if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
            return Err(reject("Invalid trusted security principal"));
        }
        Ok(allocation)
    }
    fn trusted(sid: PSID, principals: &[LocalMem; 3]) -> bool {
        principals
            .iter()
            .any(|principal| unsafe { EqualSid(sid, principal.0) } != 0)
    }
    fn require_protected_acl(file: &File) -> Result<(), ControlError> {
        let principals = [
            trusted_sid("S-1-5-18")?,
            trusted_sid("S-1-5-32-544")?,
            trusted_sid("S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464")?,
        ];
        let mut owner: PSID = ptr::null_mut();
        let mut dacl: *mut ACL = ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                ptr::null_mut(),
                &mut dacl,
                ptr::null_mut(),
                &mut descriptor,
            )
        };
        let _allocation = LocalMem(descriptor);
        if status != 0 {
            return Err(reject(format!(
                "Cannot inspect installation security descriptor: Win32 {status}"
            )));
        }
        if descriptor.is_null()
            || unsafe { IsValidSecurityDescriptor(descriptor) } == 0
            || owner.is_null()
            || unsafe { IsValidSid(owner) } == 0
            || !trusted(owner, &principals)
        {
            return Err(reject(
                "Installation ownership must be SYSTEM, Administrators or TrustedInstaller",
            ));
        }
        if dacl.is_null() || unsafe { IsValidAcl(dacl) } == 0 {
            return Err(reject("Installation has no valid restrictive DACL"));
        }
        let dangerous = GENERIC_ALL
            | GENERIC_WRITE
            | DELETE
            | WRITE_DAC
            | WRITE_OWNER
            | FILE_WRITE_DATA
            | FILE_APPEND_DATA
            | FILE_WRITE_EA
            | FILE_WRITE_ATTRIBUTES
            | FILE_DELETE_CHILD
            | 0x0200_0000;
        let base = dacl as usize;
        let acl_size = unsafe { (*dacl).AclSize } as usize;
        let end = base
            .checked_add(acl_size)
            .ok_or_else(|| reject("Invalid installation ACL range"))?;
        let count = unsafe { (*dacl).AceCount } as u32;
        for index in 0..count {
            let mut raw: *mut c_void = ptr::null_mut();
            if unsafe { GetAce(dacl, index, &mut raw) } == 0 {
                return Err(win_failure("reading an installation ACL entry"));
            }
            let address = raw as usize;
            if raw.is_null()
                || address < base + size_of::<ACL>()
                || address
                    .checked_add(size_of::<ACE_HEADER>())
                    .is_none_or(|v| v > end)
            {
                return Err(reject("Invalid installation ACL entry range"));
            }
            let header = unsafe { ptr::read_unaligned(raw.cast::<ACE_HEADER>()) };
            let length = header.AceSize as usize;
            if length < size_of::<ACE_HEADER>()
                || address.checked_add(length).is_none_or(|v| v > end)
            {
                return Err(reject("Invalid installation ACL entry size"));
            }
            if u32::from(header.AceFlags) & INHERIT_ONLY_ACE != 0 {
                continue;
            }
            if !matches!(header.AceType, 0 | 1) {
                return Err(reject("Unsupported installation ACL entry type"));
            }
            if length < 16 {
                return Err(reject("Installation ACL entry has a truncated SID"));
            }
            let bytes = unsafe { std::slice::from_raw_parts(raw.cast::<u8>(), length) };
            let sid_length = 8usize + usize::from(bytes[9]) * 4;
            if 8usize.checked_add(sid_length).is_none_or(|v| v > length) {
                return Err(reject("Installation ACL SID exceeds its entry"));
            }
            let sid = unsafe { raw.cast::<u8>().add(8).cast::<c_void>() };
            if unsafe { IsValidSid(sid) } == 0 {
                return Err(reject("Installation ACL entry has an invalid SID"));
            }
            if header.AceType == 1 {
                continue;
            }
            let mask = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
            if mask & dangerous != 0 && !trusted(sid, &principals) {
                return Err(reject(
                    "Installation grants dangerous write access to an untrusted principal",
                ));
            }
        }
        Ok(())
    }

    use windows_sys::{
        core::GUID,
        Win32::Security::{
            Cryptography::{CertGetCertificateContextProperty, CERT_SHA256_HASH_PROP_ID},
            WinTrust::{
                WTHelperGetProvCertFromChain, WTHelperGetProvSignerFromChain,
                WTHelperProvDataFromStateData, WinVerifyTrust, WINTRUST_ACTION_GENERIC_VERIFY_V2,
                WINTRUST_DATA, WINTRUST_DATA_0, WINTRUST_FILE_INFO, WINTRUST_SIGNATURE_SETTINGS,
                WSS_VERIFY_SPECIFIC, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE,
                WTD_DISABLE_MD2_MD4, WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT, WTD_REVOKE_NONE,
                WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
            },
        },
    };
    fn trust_status_error(stage: &str, status: i32) -> ControlError {
        ControlError::new(
            "windows_oem_authenticode",
            format!("{stage}: WinVerifyTrust status 0x{:08X}", status as u32),
        )
    }
    struct TrustState<'a> {
        data: &'a mut WINTRUST_DATA,
        action: &'a mut GUID,
        needs_close: bool,
    }
    impl TrustState<'_> {
        fn verify(&mut self) -> i32 {
            self.needs_close = true;
            unsafe {
                WinVerifyTrust(
                    INVALID_HANDLE_VALUE,
                    &mut *self.action,
                    ptr::from_mut(&mut *self.data).cast(),
                )
            }
        }
        fn close(&mut self) -> Result<(), ControlError> {
            if !self.needs_close {
                return Ok(());
            }
            self.needs_close = false;
            self.data.dwStateAction = WTD_STATEACTION_CLOSE;
            let status = unsafe {
                WinVerifyTrust(
                    INVALID_HANDLE_VALUE,
                    &mut *self.action,
                    ptr::from_mut(&mut *self.data).cast(),
                )
            };
            if status == 0 {
                Ok(())
            } else {
                Err(trust_status_error("closing Authenticode state", status))
            }
        }
    }
    impl Drop for TrustState<'_> {
        fn drop(&mut self) {
            let _ = self.close();
        }
    }
    fn verified_leaf_sha256(state: HANDLE) -> Result<[u8; 32], ControlError> {
        if state.is_null() {
            return Err(reject(
                "Successful Authenticode verification returned no provider state",
            ));
        }
        let provider = unsafe { WTHelperProvDataFromStateData(state) };
        if provider.is_null() {
            return Err(reject("Verified Authenticode provider data is unavailable"));
        }
        let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, 0, 0) };
        if signer.is_null() || unsafe { (*signer).csCertChain } == 0 {
            return Err(reject("Verified Authenticode code signer is unavailable"));
        }
        let provider_cert = unsafe { WTHelperGetProvCertFromChain(signer, 0) };
        if provider_cert.is_null() {
            return Err(reject("Verified Authenticode signer leaf is unavailable"));
        }
        let cert = unsafe { (*provider_cert).pCert };
        if cert.is_null() {
            return Err(reject("Verified Authenticode signer leaf context is null"));
        }
        let mut digest = [0u8; 32];
        let mut length = digest.len() as u32;
        if unsafe {
            CertGetCertificateContextProperty(
                cert,
                CERT_SHA256_HASH_PROP_ID,
                digest.as_mut_ptr().cast(),
                &mut length,
            )
        } == 0
        {
            return Err(win_failure(
                "hashing the verified Authenticode signer certificate",
            ));
        }
        if length != 32 {
            return Err(reject(format!(
                "Signer SHA256 property has unexpected length {length}"
            )));
        }
        Ok(digest)
    }
    fn verify_authenticode(file: &File, path: &Path) -> Result<[u8; 32], ControlError> {
        let wide = wide_path(path)?;
        let mut info = WINTRUST_FILE_INFO {
            cbStruct: size_of::<WINTRUST_FILE_INFO>() as u32,
            pcwszFilePath: wide.as_ptr(),
            hFile: file.as_raw_handle(),
            pgKnownSubject: ptr::null_mut(),
        };
        let mut signature = WINTRUST_SIGNATURE_SETTINGS {
            cbStruct: size_of::<WINTRUST_SIGNATURE_SETTINGS>() as u32,
            dwIndex: 0,
            dwFlags: WSS_VERIFY_SPECIFIC,
            ..Default::default()
        };
        let mut data = WINTRUST_DATA {
            cbStruct: size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_NONE,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 { pFile: &mut info },
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT
                | WTD_CACHE_ONLY_URL_RETRIEVAL
                | WTD_DISABLE_MD2_MD4,
            pSignatureSettings: &mut signature,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let mut state = TrustState {
            data: &mut data,
            action: &mut action,
            needs_close: false,
        };
        let status = state.verify();
        let result = if status == 0 {
            verified_leaf_sha256(state.data.hWVTStateData)
        } else {
            Err(trust_status_error(
                "verifying primary Authenticode signature",
                status,
            ))
        };
        let closed = state.close();
        match (result, closed) {
            (Ok(digest), Ok(())) => Ok(digest),
            (Ok(_), Err(error)) | (Err(error), Ok(())) => Err(error),
            (Err(mut error), Err(close_error)) => {
                error.details.push(close_error.to_string());
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_construction_is_inert_and_can_move_to_one_worker() {
        fn require_send<T: Send>() {}
        require_send::<NativeTransport>();
        let transport = NativeTransport::new();
        assert!(transport.pending.is_none());
        // Platform-specific Send/constructor check only. All pure protocol,
        // reservation and release rules run from portable ipc_policy tests.
    }
}
