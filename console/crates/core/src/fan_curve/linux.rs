//! Persistent Linux fan-curve ioctl session. Opening performs one GET, never an
//! acquisition. Callers must retain the session while a curve is in use.
//!
//! Explicit `restore` is the observable recovery path. Dropping the last kernel
//! file description invokes the driver's fallback release, whose outcome cannot
//! be confirmed by Rust's `Drop`. A fork/dup can prolong that description's life.
//! These synchronous calls have no userspace cancellation or invented timeout.

use super::{RawFanTable, ValidatedFanCurve};
use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    },
};

pub const CONTROL_BYTES: usize = 64;
pub const APPLY_BYTES: usize = 160;
pub const STATE_BYTES: usize = 320;
pub const GET_STATE_REQUEST: u32 = libc::_IOWR::<[u8; STATE_BYTES]>(0xf1, 0) as u32;
pub const ACQUIRE_REQUEST: u32 = libc::_IOWR::<[u8; CONTROL_BYTES]>(0xf1, 1) as u32;
pub const APPLY_REQUEST: u32 = libc::_IOW::<[u8; APPLY_BYTES]>(0xf1, 2) as u32;
pub const RESTORE_RELEASE_REQUEST: u32 = libc::_IOW::<[u8; CONTROL_BYTES]>(0xf1, 3) as u32;
const DEVICE_PATH: &str = "/dev/tuxedo_io";
const DEVICE_NUMBER_PATH: &str = "/sys/class/tuxedo_io/tuxedo_io/dev";
const REQUIRED_CAPABILITIES: u32 = 0x07;
const OWNER: u32 = 0x01;
const ORIGINAL_VALID: u32 = 0x02;
const LIVE_VALID: u32 = 0x04;
const APPLIED: u32 = 0x08;
const REVOKED: u32 = 0x10;
const RESTORED_VERIFIED: u32 = 0x20;

/// The generation changes after a state operation; epoch/lease identify the
/// ownership interval. A globally visible lease number alone grants no rights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseToken {
    pub epoch: u64,
    pub generation: u64,
    pub lease_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanCurvePhase {
    Idle,
    Acquired,
    Active,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FanCurveState {
    pub token: LeaseToken,
    /// Last state operation's ID, not an echo of this GET's request ID.
    pub last_request_id: u64,
    pub phase: FanCurvePhase,
    pub capabilities: u32,
    pub owner: bool,
    pub applied: bool,
    pub revoked: bool,
    /// Last operation's verified restoration receipt, separate from live bytes.
    pub restored_verified: bool,
    /// Error of this GET's live read; does not overwrite last operation errors.
    pub live_error: i32,
    pub last_error: i32,
    pub restore_error: i32,
    pub live_table: Option<RawFanTable>,
    pub original_table: Option<RawFanTable>,
    pub live_controls: Option<[u8; 6]>,
    pub original_controls: Option<[u8; 6]>,
    /// Driver-cached commands; these four bytes are not mechanical RPM.
    pub cached_original_targets: Option<[u8; 4]>,
}

/// An error keeps the primary errno even when a subsequent GET also fails.
#[derive(Debug)]
pub struct FanCurveSessionError {
    pub operation: &'static str,
    pub message: String,
    pub io_error: Option<io::Error>,
    pub state: Option<Box<FanCurveState>>,
    /// True means current kernel metadata was decoded. LIVE_VALID is separate.
    pub state_is_current: bool,
    pub state_query_error: Option<Box<str>>,
    pub state_query_errno: Option<i32>,
    pub state_query_invalid_response: Option<Box<[u8]>>,
    /// A failed write ioctl can still have changed hardware before failing.
    pub mutation_may_have_occurred: bool,
    /// Retained for an invalid response, rather than reinterpreted as zeros.
    pub invalid_response: Option<Box<[u8]>>,
}

impl FanCurveSessionError {
    pub fn raw_os_error(&self) -> Option<i32> {
        self.io_error.as_ref().and_then(io::Error::raw_os_error)
    }
}
impl fmt::Display for FanCurveSessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.operation, self.message)?;
        if let Some(error) = &self.io_error {
            write!(f, ": {error}")?;
        }
        Ok(())
    }
}
impl std::error::Error for FanCurveSessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.io_error.as_ref().map(|error| error as _)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceBinding {
    pub filesystem_device: u64,
    pub inode: u64,
    pub character_device: u64,
}

/// No Clone, raw-fd escape, alternate device path, automatic root or helper.
pub struct FanCurveSession {
    engine: SessionEngine<DeviceTransport>,
    binding: DeviceBinding,
}

impl FanCurveSession {
    pub fn open() -> Result<Self, FanCurveSessionError> {
        let (transport, binding) =
            DeviceTransport::open(true).map_err(|error| local_io("open", error))?;
        Ok(Self {
            engine: SessionEngine::open(transport)?,
            binding,
        })
    }
    pub fn device_binding(&self) -> DeviceBinding {
        self.binding
    }
    pub fn modified_or_uncertain(&self) -> bool {
        self.engine.requires_restoration
    }
    /// A locally confirmed token, not a guarantee of current kernel ownership.
    pub fn has_local_lease(&self) -> bool {
        self.engine.lease.is_some()
    }
    /// Cached state; call refresh explicitly when new kernel metadata is needed.
    pub fn state(&self) -> &FanCurveState {
        self.engine
            .state
            .as_ref()
            .expect("successful open records state")
    }
    pub fn refresh(&mut self) -> Result<FanCurveState, FanCurveSessionError> {
        self.engine.refresh()
    }
    pub fn acquire(&mut self) -> Result<FanCurveState, FanCurveSessionError> {
        self.engine.acquire()
    }
    pub fn apply(
        &mut self,
        curve: &ValidatedFanCurve,
    ) -> Result<FanCurveState, FanCurveSessionError> {
        self.engine.apply(curve)
    }
    /// Restores and relinquishes the lease, then requires a confirming GET.
    /// Failure retains this fd so the caller can inspect/recover explicitly.
    pub fn restore(&mut self) -> Result<FanCurveState, FanCurveSessionError> {
        self.engine.restore()
    }
}

trait IoctlTransport {
    fn ioctl(&mut self, request: u32, packet: &mut [u8]) -> io::Result<()>;
}
struct DeviceTransport(File);
impl DeviceTransport {
    fn open(writable: bool) -> io::Result<(Self, DeviceBinding)> {
        let file = OpenOptions::new()
            .read(true)
            .write(writable)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(DEVICE_PATH)?;
        let opened = file.metadata()?;
        let named = std::fs::symlink_metadata(DEVICE_PATH)?;
        if !opened.file_type().is_char_device()
            || !named.file_type().is_char_device()
            || opened.dev() != named.dev()
            || opened.ino() != named.ino()
            || opened.rdev() != named.rdev()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Device node is not the same opened character device",
            ));
        }
        let mut bytes = Vec::new();
        File::open(DEVICE_NUMBER_PATH)?
            .take(65)
            .read_to_end(&mut bytes)?;
        let (major, minor) = parse_device_number(&bytes)?;
        if u64::from(libc::major(opened.rdev())) != major
            || u64::from(libc::minor(opened.rdev())) != minor
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Character device does not match tuxedo_io sysfs registration",
            ));
        }
        Ok((
            Self(file),
            DeviceBinding {
                filesystem_device: opened.dev(),
                inode: opened.ino(),
                character_device: opened.rdev(),
            },
        ))
    }
}
/// One read-only GET on the fixed character device, never an ownership request.
pub fn inspect() -> Result<FanCurveState, FanCurveSessionError> {
    let (transport, _) =
        DeviceTransport::open(false).map_err(|error| local_io("inspect_open", error))?;
    let session = SessionEngine::open(transport)?;
    Ok(session.state.expect("successful open records state"))
}
impl IoctlTransport for DeviceTransport {
    fn ioctl(&mut self, request: u32, packet: &mut [u8]) -> io::Result<()> {
        // Packets are initialized byte arrays. The driver copies their exact
        // fixed ABI size; no Rust padding or borrowed pointers cross the ABI.
        let result = unsafe { libc::ioctl(self.0.as_raw_fd(), request as _, packet.as_mut_ptr()) };
        if result == -1 {
            Err(io::Error::last_os_error())
        } else if result == 0 {
            Ok(())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Unexpected positive ioctl return",
            ))
        }
    }
}
fn parse_device_number(bytes: &[u8]) -> io::Result<(u64, u64)> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid tuxedo_io sysfs device number",
        )
    };
    if bytes.is_empty() || bytes.len() > 64 {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?;
    let (major, minor) = text.trim().split_once(':').ok_or_else(invalid)?;
    if major.is_empty()
        || minor.is_empty()
        || !major.bytes().all(|b| b.is_ascii_digit())
        || !minor.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(invalid());
    }
    Ok((
        major.parse().map_err(|_| invalid())?,
        minor.parse().map_err(|_| invalid())?,
    ))
}

struct SessionEngine<T: IoctlTransport> {
    transport: T,
    state: Option<FanCurveState>,
    lease: Option<LeaseToken>,
    acquire_attempted: bool,
    requires_restoration: bool,
    request_id: u64,
}
impl<T: IoctlTransport> SessionEngine<T> {
    fn open(transport: T) -> Result<Self, FanCurveSessionError> {
        let mut session = Self {
            transport,
            state: None,
            lease: None,
            acquire_attempted: false,
            requires_restoration: false,
            request_id: 0,
        };
        session.refresh()?;
        Ok(session)
    }
    fn next_request(&mut self) -> Result<u64, FanCurveSessionError> {
        self.request_id = self
            .request_id
            .checked_add(1)
            .ok_or_else(|| self.local("request", "Request ID exhausted"))?;
        Ok(self.request_id)
    }
    fn local(&self, operation: &'static str, message: impl Into<String>) -> FanCurveSessionError {
        FanCurveSessionError {
            operation,
            message: message.into(),
            io_error: None,
            state: self.state.clone().map(Box::new),
            state_is_current: false,
            state_query_error: None,
            state_query_errno: None,
            state_query_invalid_response: None,
            mutation_may_have_occurred: false,
            invalid_response: None,
        }
    }
    fn observe(&mut self, state: FanCurveState) {
        let same_lease = self.lease.is_none_or(|lease| {
            lease.epoch == state.token.epoch && lease.lease_id == state.token.lease_id
        });
        self.lease = (state.owner
            && !state.revoked
            && self.acquire_attempted
            && state.token.lease_id != 0
            && same_lease)
            .then_some(state.token);
        if self.lease.is_some() {
            self.requires_restoration = state.applied || state.phase != FanCurvePhase::Acquired;
        } else if state.phase == FanCurvePhase::Idle {
            self.requires_restoration = false;
        }
        self.state = Some(state);
    }
    fn refresh(&mut self) -> Result<FanCurveState, FanCurveSessionError> {
        let id = self.next_request()?;
        let mut packet = [0; STATE_BYTES];
        encode_control(&mut packet[..CONTROL_BYTES], STATE_BYTES, None, id);
        self.transport
            .ioctl(GET_STATE_REQUEST, &mut packet)
            .map_err(|error| {
                let mut failure = self.local(
                    "get_state",
                    "Kernel state query failed; cached state is not current",
                );
                failure.io_error = Some(error);
                failure
            })?;
        let state = decode_state(&packet).map_err(|message| {
            let mut failure = self.local("get_state", message);
            failure.invalid_response = Some(Box::new(packet));
            failure
        })?;
        self.observe(state.clone());
        Ok(state)
    }
    fn reconcile_error(&mut self, mut failure: FanCurveSessionError) -> FanCurveSessionError {
        match self.refresh() {
            Ok(state) => {
                failure.state = Some(Box::new(state));
                failure.state_is_current = true;
            }
            Err(query) => {
                failure.state = self.state.clone().map(Box::new);
                failure.state_is_current = false;
                failure.state_query_errno = query.raw_os_error();
                failure.state_query_error = Some(query.to_string().into_boxed_str());
                failure.state_query_invalid_response = query.invalid_response;
            }
        }
        failure
    }
    fn mutation_error(
        &mut self,
        operation: &'static str,
        error: io::Error,
    ) -> FanCurveSessionError {
        let mut failure = self.local(
            operation,
            "Kernel operation failed; no automatic retry was attempted",
        );
        failure.io_error = Some(error);
        failure.mutation_may_have_occurred = true;
        self.reconcile_error(failure)
    }
    fn confirmation_error(
        &self,
        operation: &'static str,
        mut error: FanCurveSessionError,
    ) -> FanCurveSessionError {
        error.operation = operation;
        error.mutation_may_have_occurred = true;
        error
    }
    fn owned_token(&self, operation: &'static str) -> Result<LeaseToken, FanCurveSessionError> {
        self.lease.ok_or_else(|| {
            self.local(
                operation,
                "This session has no confirmed fd-owned lease; explicit acquisition is required",
            )
        })
    }
    fn acquire(&mut self) -> Result<FanCurveState, FanCurveSessionError> {
        if self.lease.is_some() {
            return Err(self.local("acquire", "This session already owns a lease"));
        }
        let state = self.state.as_ref().expect("successful open records state");
        if state.capabilities & REQUIRED_CAPABILITIES != REQUIRED_CAPABILITIES
            || state.token.epoch == 0
        {
            return Err(self.local("acquire", "Driver has not advertised qualification, fd lease and raw restoration capabilities"));
        }
        let previous = state.token;
        self.requires_restoration = state.applied || state.phase == FanCurvePhase::RecoveryRequired;
        let id = self.next_request()?;
        let mut packet = [0; CONTROL_BYTES];
        encode_control(
            &mut packet,
            CONTROL_BYTES,
            Some(LeaseToken {
                lease_id: 0,
                ..previous
            }),
            id,
        );
        self.acquire_attempted = true;
        if let Err(error) = self.transport.ioctl(ACQUIRE_REQUEST, &mut packet) {
            return Err(self.mutation_error("acquire", error));
        }
        let returned = decode_control(&packet, CONTROL_BYTES).and_then(|(token, returned_id)| {
            if token.epoch != previous.epoch
                || token.generation <= previous.generation
                || token.lease_id == 0
                || returned_id != id
            {
                Err("ACQUIRE returned inconsistent epoch, generation, lease or request ID".into())
            } else {
                Ok(token)
            }
        });
        let token = match returned {
            Ok(token) => token,
            Err(message) => {
                let mut failure = self.local("acquire", message);
                failure.mutation_may_have_occurred = true;
                failure.invalid_response = Some(Box::new(packet));
                return Err(self.reconcile_error(failure));
            }
        };
        self.lease = Some(token);
        let state = self
            .refresh()
            .map_err(|error| self.confirmation_error("acquire_confirm", error))?;
        if !state.owner
            || state.revoked
            || state.token != token
            || state.original_table.is_none()
            || !matches!(
                state.phase,
                FanCurvePhase::Acquired | FanCurvePhase::RecoveryRequired
            )
        {
            let mut failure = self.local(
                "acquire_confirm",
                "GET did not confirm this fd's acquired lease",
            );
            failure.state_is_current = true;
            failure.mutation_may_have_occurred = true;
            return Err(failure);
        }
        Ok(state)
    }
    fn apply(&mut self, curve: &ValidatedFanCurve) -> Result<FanCurveState, FanCurveSessionError> {
        let token = self.owned_token("apply")?;
        let current = self.state.as_ref().expect("successful open records state");
        if !matches!(
            current.phase,
            FanCurvePhase::Acquired | FanCurvePhase::Active
        ) || current.original_table.is_none()
        {
            return Err(self.local(
                "apply",
                "Recovery-required state permits restoration, not another curve",
            ));
        }
        let expected = current
            .live_table
            .as_ref()
            .ok_or_else(|| {
                self.local("apply", "Live table is unavailable; no curve was submitted")
            })?
            .with_curve(curve);
        let id = self.next_request()?;
        let mut packet = [0; APPLY_BYTES];
        encode_control(&mut packet[..CONTROL_BYTES], APPLY_BYTES, Some(token), id);
        packet[CONTROL_BYTES..].copy_from_slice(&curve.protocol_payload());
        self.requires_restoration = true;
        if let Err(error) = self.transport.ioctl(APPLY_REQUEST, &mut packet) {
            return Err(self.mutation_error("apply", error));
        }
        let state = self
            .refresh()
            .map_err(|error| self.confirmation_error("apply_confirm", error))?;
        if !state.owner
            || state.revoked
            || state.token.epoch != token.epoch
            || state.token.lease_id != token.lease_id
            || state.token.generation <= token.generation
            || state.phase != FanCurvePhase::Active
            || !state.applied
            || state.last_request_id != id
            || state.last_error != 0
            || state.live_table.as_ref() != Some(&expected)
            || state.live_controls.is_none_or(|controls| {
                (controls[0] & 0x80 != 0) != curve.curve().respective
                    || controls[1] & 0x04 == 0
                    || controls[2] & 0x40 != 0
            })
        {
            let mut failure = self.local(
                "apply_confirm",
                "GET did not confirm the applied table, control bits and ownership",
            );
            failure.state_is_current = true;
            failure.mutation_may_have_occurred = true;
            return Err(failure);
        }
        Ok(state)
    }
    fn restore(&mut self) -> Result<FanCurveState, FanCurveSessionError> {
        let token = self.owned_token("restore")?;
        let needs_restoration_receipt = self.requires_restoration;
        let id = self.next_request()?;
        let mut packet = [0; CONTROL_BYTES];
        encode_control(&mut packet, CONTROL_BYTES, Some(token), id);
        if let Err(error) = self.transport.ioctl(RESTORE_RELEASE_REQUEST, &mut packet) {
            return Err(self.mutation_error("restore", error));
        }
        let state = self
            .refresh()
            .map_err(|error| self.confirmation_error("restore_confirm", error))?;
        if state.owner
            || state.phase != FanCurvePhase::Idle
            || state.token.lease_id != 0
            || state.token.epoch != token.epoch
            || state.token.generation <= token.generation
            || state.last_request_id != id
            || state.last_error != 0
            || state.restore_error != 0
            || (needs_restoration_receipt && !state.restored_verified)
        {
            let mut failure = self.local("restore_confirm", "GET did not confirm lease release and the required verified-restoration receipt; later state operations may have replaced that receipt");
            failure.state_is_current = true;
            failure.mutation_may_have_occurred = true;
            return Err(failure);
        }
        Ok(state)
    }
}

fn local_io(operation: &'static str, error: io::Error) -> FanCurveSessionError {
    FanCurveSessionError {
        operation,
        message: "Device could not be bound".into(),
        io_error: Some(error),
        state: None,
        state_is_current: false,
        state_query_error: None,
        state_query_errno: None,
        state_query_invalid_response: None,
        mutation_may_have_occurred: false,
        invalid_response: None,
    }
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("fixed ABI slice"),
    )
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("fixed ABI slice"),
    )
}
fn encode_control(bytes: &mut [u8], size: usize, token: Option<LeaseToken>, request_id: u64) {
    bytes.fill(0);
    bytes[0..4].copy_from_slice(&1u32.to_ne_bytes());
    bytes[4..8].copy_from_slice(&(size as u32).to_ne_bytes());
    if let Some(token) = token {
        bytes[16..24].copy_from_slice(&token.epoch.to_ne_bytes());
        bytes[24..32].copy_from_slice(&token.generation.to_ne_bytes());
        bytes[32..40].copy_from_slice(&token.lease_id.to_ne_bytes());
    }
    bytes[40..48].copy_from_slice(&request_id.to_ne_bytes());
}
fn decode_control(bytes: &[u8], size: usize) -> Result<(LeaseToken, u64), String> {
    if bytes.len() < CONTROL_BYTES || u32_at(bytes, 0) != 1 || u32_at(bytes, 4) != size as u32 {
        return Err("Invalid fan-curve ABI version or message size".into());
    }
    if bytes[8..16]
        .iter()
        .chain(&bytes[48..64])
        .any(|byte| *byte != 0)
    {
        return Err("Nonzero fan-curve control flags or reserved bytes".into());
    }
    Ok((
        LeaseToken {
            epoch: u64_at(bytes, 16),
            generation: u64_at(bytes, 24),
            lease_id: u64_at(bytes, 32),
        },
        u64_at(bytes, 40),
    ))
}
fn decode_state(bytes: &[u8; STATE_BYTES]) -> Result<FanCurveState, String> {
    let (token, last_request_id) = decode_control(bytes, STATE_BYTES)?;
    let phase = match u32_at(bytes, 64) {
        0 => FanCurvePhase::Idle,
        1 => FanCurvePhase::Acquired,
        2 => FanCurvePhase::Active,
        3 => FanCurvePhase::RecoveryRequired,
        _ => return Err("Unknown fan-curve phase".into()),
    };
    let capabilities = u32_at(bytes, 68);
    let flags = u32_at(bytes, 72);
    if capabilities & !0x07 != 0
        || flags & !0x3f != 0
        || bytes[296..320].iter().any(|byte| *byte != 0)
    {
        return Err("Unknown capabilities, flags or nonzero state reserved bytes".into());
    }
    if flags & OWNER != 0 && token.lease_id == 0 {
        return Err("Owner flag without a lease".into());
    }
    let live = flags & LIVE_VALID != 0;
    let original = flags & ORIGINAL_VALID != 0;
    Ok(FanCurveState {
        token,
        last_request_id,
        phase,
        capabilities,
        owner: flags & OWNER != 0,
        applied: flags & APPLIED != 0,
        revoked: flags & REVOKED != 0,
        restored_verified: flags & RESTORED_VERIFIED != 0,
        live_error: i32::from_ne_bytes(bytes[76..80].try_into().expect("fixed ABI slice")),
        last_error: i32::from_ne_bytes(bytes[80..84].try_into().expect("fixed ABI slice")),
        restore_error: i32::from_ne_bytes(bytes[84..88].try_into().expect("fixed ABI slice")),
        live_table: live
            .then(|| RawFanTable::from_bytes(bytes[88..184].try_into().expect("fixed ABI slice"))),
        original_table: original
            .then(|| RawFanTable::from_bytes(bytes[184..280].try_into().expect("fixed ABI slice"))),
        live_controls: live.then(|| bytes[280..286].try_into().expect("fixed ABI slice")),
        original_controls: original.then(|| bytes[286..292].try_into().expect("fixed ABI slice")),
        cached_original_targets: original
            .then(|| bytes[292..296].try_into().expect("fixed ABI slice")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fan_curve::{FanCurve, FanCurveChannel, FanCurveLayout};
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    struct Step {
        request: u32,
        input: Vec<u8>,
        output: Option<Vec<u8>>,
        errno: Option<i32>,
    }
    #[derive(Default)]
    struct Trace {
        steps: VecDeque<Step>,
        requests: Vec<u32>,
        closed: usize,
    }
    struct MockTransport(Rc<RefCell<Trace>>);
    impl IoctlTransport for MockTransport {
        fn ioctl(&mut self, request: u32, packet: &mut [u8]) -> io::Result<()> {
            let mut trace = self.0.borrow_mut();
            let step = trace.steps.pop_front().expect("unexpected extra ioctl");
            assert_eq!(request, step.request);
            assert_eq!(
                packet, step.input,
                "all request bytes, including reserved bytes, must match"
            );
            trace.requests.push(request);
            if let Some(output) = step.output {
                packet.copy_from_slice(&output);
            }
            match step.errno {
                Some(errno) => Err(io::Error::from_raw_os_error(errno)),
                None => Ok(()),
            }
        }
    }
    impl Drop for MockTransport {
        fn drop(&mut self) {
            self.0.borrow_mut().closed += 1;
        }
    }
    fn control(size: usize, token: Option<LeaseToken>, id: u64) -> Vec<u8> {
        let mut packet = vec![0; size];
        encode_control(&mut packet[..CONTROL_BYTES], size, token, id);
        packet
    }
    fn token(generation: u64, lease_id: u64) -> LeaseToken {
        LeaseToken {
            epoch: 11,
            generation,
            lease_id,
        }
    }
    fn original() -> RawFanTable {
        RawFanTable::from_bytes(std::array::from_fn(|i| (i * 3) as u8))
    }
    fn curve() -> ValidatedFanCurve {
        FanCurve {
            schema_version: 1,
            layout: FanCurveLayout::UniwillRamFan1p5,
            respective: true,
            cpu: FanCurveChannel {
                rise_celsius: [35; 15],
                fall_celsius: [30; 15],
                duty_raw: [101; 16],
            },
            gpu: FanCurveChannel {
                rise_celsius: [255; 15],
                fall_celsius: [255; 15],
                duty_raw: [150; 16],
            },
        }
        .validate()
        .unwrap()
    }
    fn state_packet(
        token: LeaseToken,
        phase: u32,
        flags: u32,
        id: u64,
        table: &RawFanTable,
    ) -> Vec<u8> {
        let mut bytes = control(STATE_BYTES, Some(token), id);
        bytes[64..68].copy_from_slice(&phase.to_ne_bytes());
        bytes[68..72].copy_from_slice(&REQUIRED_CAPABILITIES.to_ne_bytes());
        bytes[72..76].copy_from_slice(&flags.to_ne_bytes());
        bytes[88..184].copy_from_slice(table.as_bytes());
        bytes[184..280].copy_from_slice(original().as_bytes());
        bytes[280..286].copy_from_slice(if phase == 2 {
            &[0x81, 0x06, 3, 4, 5, 6]
        } else {
            &[1, 2, 3, 4, 5, 6]
        });
        bytes[286..292].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        bytes[292..296].copy_from_slice(&[51, 52, 53, 54]);
        bytes
    }
    fn get(id: u64, output: Vec<u8>) -> Step {
        Step {
            request: GET_STATE_REQUEST,
            input: control(STATE_BYTES, None, id),
            output: Some(output),
            errno: None,
        }
    }
    fn acquire(id: u64, before: LeaseToken, after: LeaseToken) -> Step {
        Step {
            request: ACQUIRE_REQUEST,
            input: control(
                CONTROL_BYTES,
                Some(LeaseToken {
                    lease_id: 0,
                    ..before
                }),
                id,
            ),
            output: Some(control(CONTROL_BYTES, Some(after), id)),
            errno: None,
        }
    }
    fn apply(id: u64, token: LeaseToken, errno: Option<i32>) -> Step {
        let mut input = control(APPLY_BYTES, Some(token), id);
        input[CONTROL_BYTES..].copy_from_slice(&curve().protocol_payload());
        Step {
            request: APPLY_REQUEST,
            input,
            output: None,
            errno,
        }
    }
    fn restore(id: u64, token: LeaseToken, errno: Option<i32>) -> Step {
        Step {
            request: RESTORE_RELEASE_REQUEST,
            input: control(CONTROL_BYTES, Some(token), id),
            output: None,
            errno,
        }
    }
    fn mock(steps: Vec<Step>) -> (MockTransport, Rc<RefCell<Trace>>) {
        let trace = Rc::new(RefCell::new(Trace {
            steps: steps.into(),
            ..Trace::default()
        }));
        (MockTransport(trace.clone()), trace)
    }
    fn acquired_steps() -> Vec<Step> {
        vec![
            get(
                1,
                state_packet(token(20, 0), 0, LIVE_VALID, 900, &original()),
            ),
            acquire(2, token(20, 0), token(21, 77)),
            get(
                3,
                state_packet(
                    token(21, 77),
                    1,
                    OWNER | ORIGINAL_VALID | LIVE_VALID,
                    2,
                    &original(),
                ),
            ),
        ]
    }

    #[test]
    fn persistent_session_initializes_every_byte_and_confirms_apply_then_restore() {
        let expected = original().with_curve(&curve());
        let mut steps = acquired_steps();
        steps.extend([
            apply(4, token(21, 77), None),
            get(
                5,
                state_packet(
                    token(22, 77),
                    2,
                    OWNER | ORIGINAL_VALID | LIVE_VALID | APPLIED,
                    4,
                    &expected,
                ),
            ),
            restore(6, token(22, 77), None),
            get(
                7,
                state_packet(
                    token(23, 0),
                    0,
                    LIVE_VALID | RESTORED_VERIFIED,
                    6,
                    &original(),
                ),
            ),
        ]);
        let (transport, trace) = mock(steps);
        let mut session = SessionEngine::open(transport).unwrap();
        assert_eq!(trace.borrow().requests, [GET_STATE_REQUEST]);
        assert!(session.lease.is_none(), "open must not acquire");
        session.acquire().unwrap();
        assert_eq!(session.lease, Some(token(21, 77)));
        let active = session.apply(&curve()).unwrap();
        assert_eq!(active.live_table, Some(expected));
        assert_eq!(session.lease, Some(token(22, 77)));
        assert_eq!(
            trace.borrow().closed,
            0,
            "the same transport remains alive while active"
        );
        let idle = session.restore().unwrap();
        assert_eq!(idle.phase, FanCurvePhase::Idle);
        assert!(session.lease.is_none());
        assert_eq!(idle.live_table, Some(original()));
        assert!(trace.borrow().steps.is_empty());
        drop(session);
        assert_eq!(trace.borrow().closed, 1);
        assert_eq!(
            trace.borrow().requests.len(),
            7,
            "Drop does not invent a second restore acknowledgement"
        );
    }
    #[test]
    fn no_default_lease_and_unavailable_live_bytes_are_not_zero_measurements() {
        let mut response = state_packet(token(20, 0), 0, 0, 19, &original());
        response[68..72].fill(0);
        response[80..84].copy_from_slice(&(-libc::ENODEV).to_ne_bytes());
        let (transport, trace) = mock(vec![get(1, response)]);
        let mut session = SessionEngine::open(transport).unwrap();
        let state = session.state.as_ref().unwrap();
        assert!(state.live_table.is_none());
        assert!(state.live_controls.is_none());
        assert!(state.original_table.is_none());
        assert_eq!(state.last_error, -libc::ENODEV);
        assert!(session.acquire().is_err());
        assert!(session.apply(&curve()).is_err());
        assert!(session.restore().is_err());
        assert_eq!(trace.borrow().requests, [GET_STATE_REQUEST]);
    }
    #[test]
    fn mutation_errno_and_secondary_get_failure_are_both_retained_without_retry() {
        let mut steps = acquired_steps();
        steps.push(apply(4, token(21, 77), Some(libc::EINTR)));
        steps.push(Step {
            request: GET_STATE_REQUEST,
            input: control(STATE_BYTES, None, 5),
            output: None,
            errno: Some(libc::ENODEV),
        });
        let (transport, trace) = mock(steps);
        let mut session = SessionEngine::open(transport).unwrap();
        session.acquire().unwrap();
        let error = session.apply(&curve()).unwrap_err();
        assert_eq!(error.operation, "apply");
        assert_eq!(error.raw_os_error(), Some(libc::EINTR));
        assert_eq!(error.state_query_errno, Some(libc::ENODEV));
        assert!(!error.state_is_current);
        assert!(error.mutation_may_have_occurred);
        assert_eq!(error.state.as_ref().unwrap().token, token(21, 77));
        assert_eq!(
            trace
                .borrow()
                .requests
                .iter()
                .filter(|request| **request == APPLY_REQUEST)
                .count(),
            1
        );
        assert!(trace.borrow().steps.is_empty());
    }
    #[test]
    fn failed_restore_keeps_kernel_recovery_state_and_new_generation_for_explicit_retry() {
        let mut failed = state_packet(
            token(22, 77),
            3,
            OWNER | ORIGINAL_VALID | LIVE_VALID,
            4,
            &original(),
        );
        failed[80..84].copy_from_slice(&(-libc::EIO).to_ne_bytes());
        failed[84..88].copy_from_slice(&(-libc::EACCES).to_ne_bytes());
        let mut steps = acquired_steps();
        steps.extend([
            restore(4, token(21, 77), Some(libc::EIO)),
            get(5, failed),
            restore(6, token(22, 77), None),
            get(
                7,
                state_packet(
                    token(23, 0),
                    0,
                    LIVE_VALID | RESTORED_VERIFIED,
                    6,
                    &original(),
                ),
            ),
        ]);
        let (transport, trace) = mock(steps);
        let mut session = SessionEngine::open(transport).unwrap();
        session.acquire().unwrap();
        let error = session.restore().unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::EIO));
        assert!(error.state_is_current);
        assert_eq!(
            error.state.as_ref().unwrap().phase,
            FanCurvePhase::RecoveryRequired
        );
        assert_eq!(error.state.as_ref().unwrap().restore_error, -libc::EACCES);
        assert_eq!(session.lease, Some(token(22, 77)));
        assert!(
            session.apply(&curve()).is_err(),
            "recovery state cannot accept another curve"
        );
        session.restore().unwrap();
        assert!(session.lease.is_none());
        assert!(trace.borrow().steps.is_empty());
    }
    #[test]
    fn acquisition_ack_with_failed_confirmation_can_release_untouched_lease() {
        let (transport, trace) = mock(vec![
            get(1, state_packet(token(20, 0), 0, LIVE_VALID, 0, &original())),
            acquire(2, token(20, 0), token(21, 77)),
            Step {
                request: GET_STATE_REQUEST,
                input: control(STATE_BYTES, None, 3),
                output: None,
                errno: Some(libc::EIO),
            },
            restore(4, token(21, 77), None),
            get(5, state_packet(token(22, 0), 0, LIVE_VALID, 4, &original())),
        ]);
        let mut session = SessionEngine::open(transport).unwrap();
        let error = session.acquire().unwrap_err();
        assert_eq!(error.operation, "acquire_confirm");
        assert_eq!(session.lease, Some(token(21, 77)));
        assert!(!session.requires_restoration);
        assert!(!session.restore().unwrap().restored_verified);
        assert!(trace.borrow().steps.is_empty());
    }
    #[test]
    fn failed_apply_confirmation_retains_required_restore_receipt_even_with_old_cache() {
        let mut steps = acquired_steps();
        steps.extend([
            apply(4, token(21, 77), None),
            Step {
                request: GET_STATE_REQUEST,
                input: control(STATE_BYTES, None, 5),
                output: None,
                errno: Some(libc::EIO),
            },
        ]);
        let (transport, trace) = mock(steps);
        let mut session = SessionEngine::open(transport).unwrap();
        session.acquire().unwrap();
        assert!(session.apply(&curve()).is_err());
        assert!(session.requires_restoration);
        assert_eq!(
            session.state.as_ref().unwrap().phase,
            FanCurvePhase::Acquired
        );
        assert!(trace.borrow().steps.is_empty());
    }
    #[test]
    fn rejected_apply_without_writes_can_release_acquired_lease_with_new_success_receipt() {
        let mut rejected = state_packet(
            token(22, 77),
            1,
            OWNER | ORIGINAL_VALID | LIVE_VALID,
            4,
            &original(),
        );
        rejected[80..84].copy_from_slice(&(-libc::EAGAIN).to_ne_bytes());
        let mut steps = acquired_steps();
        steps.extend([
            apply(4, token(21, 77), Some(libc::EAGAIN)),
            get(5, rejected),
            restore(6, token(22, 77), None),
            get(7, state_packet(token(23, 0), 0, LIVE_VALID, 6, &original())),
        ]);
        let (transport, trace) = mock(steps);
        let mut session = SessionEngine::open(transport).unwrap();
        session.acquire().unwrap();
        assert_eq!(
            session.apply(&curve()).unwrap_err().raw_os_error(),
            Some(libc::EAGAIN)
        );
        let released = session.restore().unwrap();
        assert_eq!(released.last_error, 0);
        assert!(!released.restored_verified);
        assert!(session.lease.is_none());
        assert!(trace.borrow().steps.is_empty());
    }
    #[test]
    fn invalid_reconciliation_keeps_both_primary_and_state_response_evidence() {
        let mut acquire_reply = control(CONTROL_BYTES, Some(token(21, 77)), 2);
        acquire_reply[48] = 1;
        let mut state_reply = state_packet(
            token(21, 77),
            1,
            OWNER | ORIGINAL_VALID | LIVE_VALID,
            2,
            &original(),
        );
        state_reply[319] = 1;
        let (transport, _) = mock(vec![
            get(1, state_packet(token(20, 0), 0, LIVE_VALID, 0, &original())),
            Step {
                request: ACQUIRE_REQUEST,
                input: control(CONTROL_BYTES, Some(token(20, 0)), 2),
                output: Some(acquire_reply),
                errno: None,
            },
            get(3, state_reply),
        ]);
        let mut session = SessionEngine::open(transport).unwrap();
        let error = session.acquire().unwrap_err();
        assert_eq!(
            error.invalid_response.as_ref().unwrap().len(),
            CONTROL_BYTES
        );
        assert_eq!(
            error.state_query_invalid_response.as_ref().unwrap().len(),
            STATE_BYTES
        );
        assert!(!error.state_is_current);
        assert!(error.state_query_error.is_some());
    }
    #[test]
    fn foreign_global_lease_and_revocation_do_not_become_this_fd_ownership_or_late_restore() {
        let mut steps = acquired_steps();
        steps.push(apply(4, token(21, 77), Some(libc::ESTALE)));
        let foreign = LeaseToken {
            epoch: 12,
            generation: 100,
            lease_id: 88,
        };
        steps.push(get(
            5,
            state_packet(foreign, 2, REVOKED | LIVE_VALID | APPLIED, 999, &original()),
        ));
        let (transport, trace) = mock(steps);
        let mut session = SessionEngine::open(transport).unwrap();
        session.acquire().unwrap();
        let error = session.apply(&curve()).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ESTALE));
        assert!(error.state_is_current);
        assert_eq!(error.state.as_ref().unwrap().token, foreign);
        assert!(session.lease.is_none());
        assert!(session.restore().is_err());
        assert!(session.apply(&curve()).is_err());
        drop(session);
        assert_eq!(trace.borrow().closed, 1);
        assert!(
            !trace.borrow().requests.contains(&RESTORE_RELEASE_REQUEST),
            "late Drop must not send stale tokens to a replacement owner"
        );
        // Kernel close's ownership guard is tested by the kernel harness; this
        // mock proves only this production engine's syscall/last-fd lifecycle.
    }
    #[test]
    fn malformed_acquire_reply_retains_response_and_reconciles_actual_fd_owner() {
        let mut reply = control(CONTROL_BYTES, Some(token(20, 77)), 2); // generation failed to advance
        reply[48] = 1;
        let (transport, trace) = mock(vec![
            get(1, state_packet(token(20, 0), 0, LIVE_VALID, 0, &original())),
            Step {
                request: ACQUIRE_REQUEST,
                input: control(CONTROL_BYTES, Some(token(20, 0)), 2),
                output: Some(reply),
                errno: None,
            },
            get(
                3,
                state_packet(
                    token(21, 77),
                    1,
                    OWNER | ORIGINAL_VALID | LIVE_VALID,
                    2,
                    &original(),
                ),
            ),
        ]);
        let mut session = SessionEngine::open(transport).unwrap();
        let error = session.acquire().unwrap_err();
        assert!(error.invalid_response.is_some());
        assert!(error.mutation_may_have_occurred);
        assert!(error.state_is_current);
        assert_eq!(
            session.lease,
            Some(token(21, 77)),
            "confirmed owner may explicitly recover after an ambiguous acknowledgement"
        );
        assert!(trace.borrow().steps.is_empty());
    }
    #[test]
    fn successful_write_with_wrong_readback_is_not_reported_as_applied() {
        let mut steps = acquired_steps();
        steps.extend([
            apply(4, token(21, 77), None),
            get(
                5,
                state_packet(
                    token(22, 77),
                    2,
                    OWNER | ORIGINAL_VALID | LIVE_VALID | APPLIED,
                    4,
                    &original(),
                ),
            ),
        ]);
        let (transport, trace) = mock(steps);
        let mut session = SessionEngine::open(transport).unwrap();
        session.acquire().unwrap();
        let error = session.apply(&curve()).unwrap_err();
        assert_eq!(error.operation, "apply_confirm");
        assert!(error.state_is_current);
        assert!(error.mutation_may_have_occurred);
        assert_eq!(session.lease, Some(token(22, 77)));
        assert_eq!(
            trace.borrow().requests.len(),
            5,
            "no silent retry or restore"
        );
    }
    #[test]
    fn correct_table_cannot_ack_wrong_split_disabled_table_or_full_speed_control() {
        for (offset, mask) in [(280, 0x80), (281, 0x04), (282, 0x40)] {
            let expected = original().with_curve(&curve());
            let mut response = state_packet(
                token(22, 77),
                2,
                OWNER | ORIGINAL_VALID | LIVE_VALID | APPLIED,
                4,
                &expected,
            );
            response[offset] ^= mask;
            let mut steps = acquired_steps();
            steps.extend([apply(4, token(21, 77), None), get(5, response)]);
            let (transport, trace) = mock(steps);
            let mut session = SessionEngine::open(transport).unwrap();
            session.acquire().unwrap();
            let error = session.apply(&curve()).unwrap_err();
            assert_eq!(error.operation, "apply_confirm");
            assert!(error.state_is_current);
            assert!(error.mutation_may_have_occurred);
            assert_eq!(session.lease, Some(token(22, 77)));
            assert!(session.requires_restoration);
            assert_eq!(
                trace.borrow().requests.len(),
                5,
                "no hidden write or retry for control byte {offset}"
            );
            assert!(trace.borrow().steps.is_empty());
        }
    }
    #[test]
    fn restoration_receipt_survives_later_legacy_table_changes_and_unowned_control_bits() {
        let expected = original().with_curve(&curve());
        let later_table = RawFanTable::from_bytes([123; 96]);
        let mut restored = state_packet(
            token(23, 0),
            0,
            LIVE_VALID | RESTORED_VERIFIED,
            6,
            &later_table,
        );
        restored[280..286].copy_from_slice(&[0x7f, 0xfb, 99, 88, 77, 66]);
        let mut steps = acquired_steps();
        steps.extend([
            apply(4, token(21, 77), None),
            get(
                5,
                state_packet(
                    token(22, 77),
                    2,
                    OWNER | ORIGINAL_VALID | LIVE_VALID | APPLIED,
                    4,
                    &expected,
                ),
            ),
            restore(6, token(22, 77), None),
            get(7, restored),
        ]);
        let (transport, trace) = mock(steps);
        let mut session = SessionEngine::open(transport).unwrap();
        session.acquire().unwrap();
        session.apply(&curve()).unwrap();
        let state = session.restore().unwrap();
        assert!(state.restored_verified);
        assert_eq!(state.live_table, Some(later_table), "live state is returned independently; receipt proves the restoration transaction, not permanent ownership");
        assert!(session.lease.is_none());
        assert!(trace.borrow().steps.is_empty());
    }
    #[test]
    fn applied_lease_requires_restore_receipt_but_unmodified_lease_release_does_not() {
        let mut steps = acquired_steps();
        steps.extend([
            restore(4, token(21, 77), None),
            get(5, state_packet(token(22, 0), 0, LIVE_VALID, 4, &original())),
        ]);
        let (transport, _) = mock(steps);
        let mut session = SessionEngine::open(transport).unwrap();
        session.acquire().unwrap();
        assert!(
            !session.restore().unwrap().restored_verified,
            "an untouched acquisition needs no physical restoration"
        );

        let expected = original().with_curve(&curve());
        let mut steps = acquired_steps();
        steps.extend([
            apply(4, token(21, 77), None),
            get(
                5,
                state_packet(
                    token(22, 77),
                    2,
                    OWNER | ORIGINAL_VALID | LIVE_VALID | APPLIED,
                    4,
                    &expected,
                ),
            ),
            restore(6, token(22, 77), None),
            get(7, state_packet(token(23, 0), 0, LIVE_VALID, 6, &original())),
        ]);
        let (transport, _) = mock(steps);
        let mut session = SessionEngine::open(transport).unwrap();
        session.acquire().unwrap();
        session.apply(&curve()).unwrap();
        let error = session.restore().unwrap_err();
        assert_eq!(error.operation, "restore_confirm");
        assert!(error.state_is_current);
        assert!(
            !error.state.unwrap().restored_verified,
            "matching live bytes cannot manufacture a missing transaction receipt"
        );
    }
    #[test]
    fn failed_later_live_query_does_not_erase_verified_restore_receipt() {
        let expected = original().with_curve(&curve());
        let mut restored = state_packet(token(23, 0), 0, RESTORED_VERIFIED, 6, &original());
        restored[76..80].copy_from_slice(&(-libc::EIO).to_ne_bytes());
        let mut steps = acquired_steps();
        steps.extend([
            apply(4, token(21, 77), None),
            get(
                5,
                state_packet(
                    token(22, 77),
                    2,
                    OWNER | ORIGINAL_VALID | LIVE_VALID | APPLIED,
                    4,
                    &expected,
                ),
            ),
            restore(6, token(22, 77), None),
            get(7, restored),
        ]);
        let (transport, _) = mock(steps);
        let mut session = SessionEngine::open(transport).unwrap();
        session.acquire().unwrap();
        session.apply(&curve()).unwrap();
        let state = session.restore().unwrap();
        assert!(state.restored_verified);
        assert_eq!(state.live_error, -libc::EIO);
        assert_eq!(state.last_error, 0);
        assert_eq!(state.restore_error, 0);
        assert!(state.live_table.is_none());
        assert!(state.live_controls.is_none());
    }
    #[test]
    fn acquisition_rejects_wrong_epoch_generation_lease_or_acknowledgement_id() {
        for (reply_token, reply_id) in [
            (
                LeaseToken {
                    epoch: 12,
                    generation: 21,
                    lease_id: 77,
                },
                2,
            ),
            (token(20, 77), 2),
            (token(21, 0), 2),
            (token(21, 77), 99),
        ] {
            let (transport, trace) = mock(vec![
                get(1, state_packet(token(20, 0), 0, LIVE_VALID, 0, &original())),
                Step {
                    request: ACQUIRE_REQUEST,
                    input: control(CONTROL_BYTES, Some(token(20, 0)), 2),
                    output: Some(control(CONTROL_BYTES, Some(reply_token), reply_id)),
                    errno: None,
                },
                get(3, state_packet(token(20, 0), 0, LIVE_VALID, 0, &original())),
            ]);
            let mut session = SessionEngine::open(transport).unwrap();
            let error = session.acquire().unwrap_err();
            assert!(error.invalid_response.is_some());
            assert!(error.state_is_current);
            assert!(session.lease.is_none());
            assert!(trace.borrow().steps.is_empty());
        }
    }
    #[test]
    fn invalid_initial_state_closes_transport_and_request_counter_never_wraps() {
        let mut invalid = state_packet(token(20, 0), 0, LIVE_VALID, 0, &original());
        invalid[319] = 1;
        let (transport, trace) = mock(vec![get(1, invalid)]);
        let error = SessionEngine::open(transport).err().unwrap();
        assert!(error.invalid_response.is_some());
        assert!(error.state.is_none());
        assert_eq!(trace.borrow().closed, 1);
        let (transport, trace) = mock(vec![get(
            1,
            state_packet(token(20, 0), 0, LIVE_VALID, 0, &original()),
        )]);
        let mut session = SessionEngine::open(transport).unwrap();
        session.request_id = u64::MAX;
        assert!(session.refresh().is_err());
        assert_eq!(trace.borrow().requests, [GET_STATE_REQUEST]);
    }
    #[test]
    fn state_reserved_unknown_flags_invalid_header_and_signed_errors_are_checked() {
        let valid: [u8; STATE_BYTES] = state_packet(token(20, 0), 0, LIVE_VALID, 991, &original())
            .try_into()
            .unwrap();
        assert_eq!(
            decode_state(&valid).unwrap().last_request_id,
            991,
            "GET response is not an input echo"
        );
        for offset in [0, 4, 8, 12, 48, 63, 64, 68, 72, 296, 319] {
            let mut invalid = valid;
            invalid[offset] = 255;
            assert!(
                decode_state(&invalid).is_err(),
                "accepted invalid ABI byte {offset}"
            );
        }
        let mut errors = valid;
        errors[76..80].copy_from_slice(&(-libc::ETIMEDOUT).to_ne_bytes());
        errors[80..84].copy_from_slice(&i32::MIN.to_ne_bytes());
        errors[84..88].copy_from_slice(&(-libc::EIO).to_ne_bytes());
        let decoded = decode_state(&errors).unwrap();
        assert_eq!(decoded.live_error, -libc::ETIMEDOUT);
        assert_eq!(decoded.last_error, i32::MIN);
        assert_eq!(decoded.restore_error, -libc::EIO);
        assert!(decode_control(&[0; 63], CONTROL_BYTES).is_err());
    }
    #[test]
    fn device_number_parser_is_bounded_and_rejects_partial_or_non_numeric_values() {
        assert_eq!(parse_device_number(b"241:0\n").unwrap(), (241, 0));
        for bad in [
            b"".as_slice(),
            b"241",
            b"241:",
            b":0",
            b"1:2:3",
            b"1:-1",
            b"+1:0",
            b"1:0x0",
            b"1:0\0",
            b"\xff:0",
        ] {
            assert!(parse_device_number(bad).is_err());
        }
        assert!(parse_device_number(&[b'1'; 65]).is_err());
        assert!(parse_device_number(b"18446744073709551616:0").is_err());
    }
    #[test]
    fn fixed_wire_offsets_sizes_and_ioctl_encoding_match_internal_layout_contract() {
        #[repr(C, align(8))]
        struct Control {
            version: u32,
            size: u32,
            flags: u32,
            reserved0: u32,
            epoch: u64,
            generation: u64,
            lease_id: u64,
            request_id: u64,
            reserved: [u32; 4],
        }
        #[repr(C)]
        struct Apply {
            control: Control,
            rise: [[u8; 15]; 2],
            fall: [[u8; 15]; 2],
            duty_raw: [[u8; 16]; 2],
            respective: u8,
            reserved: [u8; 3],
        }
        #[repr(C)]
        struct State {
            control: Control,
            phase: u32,
            capabilities: u32,
            state_flags: u32,
            live_error: i32,
            last_error: i32,
            restore_error: i32,
            table: [u8; 96],
            original_table: [u8; 96],
            controls: [u8; 6],
            original_controls: [u8; 6],
            original_targets: [u8; 4],
            reserved: [u32; 6],
        }
        assert_eq!(std::mem::size_of::<Control>(), CONTROL_BYTES);
        assert_eq!(std::mem::align_of::<Control>(), 8);
        assert_eq!(std::mem::size_of::<Apply>(), APPLY_BYTES);
        assert_eq!(std::mem::size_of::<State>(), STATE_BYTES);
        assert_eq!(
            [
                std::mem::offset_of!(Control, epoch),
                std::mem::offset_of!(Control, generation),
                std::mem::offset_of!(Control, lease_id),
                std::mem::offset_of!(Control, request_id)
            ],
            [16, 24, 32, 40]
        );
        assert_eq!(
            [
                std::mem::offset_of!(Apply, rise),
                std::mem::offset_of!(Apply, fall),
                std::mem::offset_of!(Apply, duty_raw),
                std::mem::offset_of!(Apply, respective)
            ],
            [64, 94, 124, 156]
        );
        assert_eq!(
            [
                std::mem::offset_of!(State, phase),
                std::mem::offset_of!(State, last_error),
                std::mem::offset_of!(State, restore_error),
                std::mem::offset_of!(State, table),
                std::mem::offset_of!(State, original_table),
                std::mem::offset_of!(State, controls),
                std::mem::offset_of!(State, original_controls),
                std::mem::offset_of!(State, original_targets),
                std::mem::offset_of!(State, reserved)
            ],
            [64, 80, 84, 88, 184, 280, 286, 292, 296]
        );
        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64", target_arch = "x86"))]
        assert_eq!(
            [
                GET_STATE_REQUEST,
                ACQUIRE_REQUEST,
                APPLY_REQUEST,
                RESTORE_RELEASE_REQUEST
            ],
            [0xc140f100, 0xc040f101, 0x40a0f102, 0x4040f103]
        );
        let mut bytes = [255; CONTROL_BYTES];
        encode_control(
            &mut bytes,
            CONTROL_BYTES,
            Some(LeaseToken {
                epoch: 0x0102030405060708,
                generation: 0x1112131415161718,
                lease_id: 0x2122232425262728,
            }),
            0x3132333435363738,
        );
        assert_eq!(u32_at(&bytes, 0), 1);
        assert_eq!(u32_at(&bytes, 4), 64);
        assert!(bytes[8..16]
            .iter()
            .chain(&bytes[48..64])
            .all(|byte| *byte == 0));
        assert_eq!(u64_at(&bytes, 16), 0x0102030405060708);
        assert_eq!(u64_at(&bytes, 24), 0x1112131415161718);
        assert_eq!(u64_at(&bytes, 32), 0x2122232425262728);
        assert_eq!(u64_at(&bytes, 40), 0x3132333435363738);
    }
    #[test]
    fn actual_c_header_layout_requests_and_complete_frame_match_production_rust_bytes() {
        use std::io::Write as _;
        use std::process::{Command, Stdio};
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("core crate resides under console/crates");
        let scratch = repository.join(".local-tests/fan-session-uapi");
        std::fs::create_dir_all(&scratch).unwrap();
        let directory = scratch.join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join("actual-header");
        let mut compiler = Command::new("cc")
            .args(["-x", "c", "-std=c11", "-Wall", "-Wextra", "-Werror"])
            .arg("-I")
            .arg(repository.join("src"))
            .arg("-o")
            .arg(&executable)
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("repository ABI tests require an installed C compiler");
        let mut stdin = compiler.stdin.take().unwrap();
        stdin
            .write_all(
                br#"
#include <stddef.h>
#include <stdio.h>
#include <string.h>
#include "uniwill_fan_curve_ioctl.h"
int main(int argc, char **argv) {
    (void)argv;
    if (argc > 1) {
        struct uw_fan_curve_apply_v1 frame;
        memset(&frame, 0, sizeof(frame));
        frame.control.version = UW_FAN_CURVE_ABI_VERSION;
        frame.control.size = sizeof(frame);
        frame.control.epoch = 11;
        frame.control.generation = 21;
        frame.control.lease_id = 77;
        frame.control.request_id = 42;
        memset(frame.rise[0], 35, sizeof(frame.rise[0]));
        memset(frame.rise[1], 255, sizeof(frame.rise[1]));
        memset(frame.fall[0], 30, sizeof(frame.fall[0]));
        memset(frame.fall[1], 255, sizeof(frame.fall[1]));
        memset(frame.duty_raw[0], 101, sizeof(frame.duty_raw[0]));
        memset(frame.duty_raw[1], 150, sizeof(frame.duty_raw[1]));
        frame.respective = 1;
        return fwrite(&frame, 1, sizeof(frame), stdout) == sizeof(frame) ? 0 : 1;
    }
#define SIZE(T) printf("%zu ", sizeof(struct T))
#define OFF(T, F) printf("%zu ", offsetof(struct T, F))
    SIZE(uw_fan_curve_control_v1); SIZE(uw_fan_curve_apply_v1); SIZE(uw_fan_curve_state_v1);
    printf("%zu ", _Alignof(struct uw_fan_curve_control_v1));
    OFF(uw_fan_curve_control_v1, epoch); OFF(uw_fan_curve_control_v1, generation);
    OFF(uw_fan_curve_control_v1, lease_id); OFF(uw_fan_curve_control_v1, request_id);
    OFF(uw_fan_curve_apply_v1, rise); OFF(uw_fan_curve_apply_v1, fall);
    OFF(uw_fan_curve_apply_v1, duty_raw); OFF(uw_fan_curve_apply_v1, respective);
    OFF(uw_fan_curve_state_v1, phase); OFF(uw_fan_curve_state_v1, capabilities);
    OFF(uw_fan_curve_state_v1, state_flags); OFF(uw_fan_curve_state_v1, live_error);
    OFF(uw_fan_curve_state_v1, last_error); OFF(uw_fan_curve_state_v1, restore_error);
    OFF(uw_fan_curve_state_v1, table); OFF(uw_fan_curve_state_v1, original_table);
    OFF(uw_fan_curve_state_v1, controls); OFF(uw_fan_curve_state_v1, original_controls);
    OFF(uw_fan_curve_state_v1, original_targets); OFF(uw_fan_curve_state_v1, reserved);
    printf("%lu %lu %lu %lu\n", (unsigned long)UW_FAN_CURVE_GET_STATE,
        (unsigned long)UW_FAN_CURVE_ACQUIRE, (unsigned long)UW_FAN_CURVE_APPLY,
        (unsigned long)UW_FAN_CURVE_RESTORE_RELEASE);
    return 0;
}
"#,
            )
            .unwrap();
        drop(stdin);
        let compiled = compiler.wait_with_output().unwrap();
        assert!(
            compiled.status.success(),
            "actual UAPI C compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let output = Command::new(&executable).output().unwrap();
        assert!(output.status.success());
        let actual = std::str::from_utf8(&output.stdout)
            .unwrap()
            .split_whitespace()
            .map(|value| value.parse::<u64>().unwrap())
            .collect::<Vec<_>>();
        let mut expected = vec![
            CONTROL_BYTES as u64,
            APPLY_BYTES as u64,
            STATE_BYTES as u64,
            8,
            16,
            24,
            32,
            40,
            64,
            94,
            124,
            156,
            64,
            68,
            72,
            76,
            80,
            84,
            88,
            184,
            280,
            286,
            292,
            296,
        ];
        expected.extend([
            GET_STATE_REQUEST as u64,
            ACQUIRE_REQUEST as u64,
            APPLY_REQUEST as u64,
            RESTORE_RELEASE_REQUEST as u64,
        ]);
        assert_eq!(
            actual, expected,
            "actual C header drifted from the production Rust ABI"
        );
        let output = Command::new(&executable).arg("frame").output().unwrap();
        assert!(output.status.success());
        let mut expected_frame = control(APPLY_BYTES, Some(token(21, 77)), 42);
        expected_frame[CONTROL_BYTES..].copy_from_slice(&curve().protocol_payload());
        assert_eq!(
            output.stdout, expected_frame,
            "C struct and Rust initialized APPLY bytes differ"
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
