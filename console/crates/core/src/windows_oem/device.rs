//! Fixed Fn-lock transport contract. This is not a general EC/IOCTL interface.
//!
//! The driver's completion length is the caller's output capacity, including
//! when ECRR/ECRW fails internally. Neither it nor a successful Win32 call is a
//! hardware acknowledgement. Only a separate strict read can verify a write.
use crate::ControlError;

#[cfg(test)]
const DEVICE_PATH: &str = r"\\.\ACPIDriver";
pub(super) const DEVICE_PATH_WIDE: [u16; 15] = [
    92, 92, 46, 92, 65, 67, 80, 73, 68, 114, 105, 118, 101, 114, 0,
];
pub(super) const BUFFER_BYTES: usize = 4 * 1024 * 1024;
pub(super) const FN_ADDRESS: u32 = 0x074e;
const GUARD_BYTES: usize = 64;
const OUTPUT_INITIAL_BYTE: u8 = 0xd6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TransferEvidence {
    pub input_bytes: u32,
    pub output_capacity_bytes: u32,
    pub returned_bytes: u32,
    pub input_guard_intact: bool,
    pub output_guard_intact: bool,
    pub input_unchanged: bool,
    pub output_tail_matches_input: bool,
    /// Only the allowed four-byte prefix is retained, never the raw 4MiB output.
    pub output_prefix_le: u32,
    /// Changes are relative to the initialized METHOD_BUFFERED input image.
    pub changed_output_bytes: u32,
    pub first_changed_output_offset: Option<u32>,
    pub last_changed_output_offset: Option<u32>,
}

#[derive(Debug, Clone)]
pub(crate) struct FnByteRead {
    pub value: u8,
    pub transfer: TransferEvidence,
}

#[derive(Debug, Clone)]
pub(crate) enum WriteSubmission {
    NotSubmitted(ControlError),
    /// Entering DeviceIoControl is submission, including a FALSE return. This
    /// result describes transport only; it never promises the EC accepted it.
    Submitted(Result<TransferEvidence, ControlError>),
}

pub(crate) trait FnDevice {
    fn read_fn_byte(&mut self) -> Result<FnByteRead, ControlError>;
    fn write_fn_byte(&mut self, value: u8) -> WriteSubmission;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FnOperation {
    Read,
    Write(u8),
}
impl FnOperation {
    pub(super) fn ioctl(self) -> u32 {
        match self {
            Self::Read => 0x9c40_a488,
            Self::Write(_) => 0x9c40_a48c,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Read => "fn_read",
            Self::Write(_) => "fn_write",
        }
    }
    fn expected_input(self, offset: usize) -> u8 {
        match offset {
            0..=3 => FN_ADDRESS.to_le_bytes()[offset],
            4 => match self {
                Self::Write(value) => value,
                Self::Read => 0,
            },
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) enum BufferRole {
    Input,
    Output,
}
impl BufferRole {
    fn guard(self, offset: usize) -> u8 {
        let seed: u8 = match self {
            Self::Input => 0xa7,
            Self::Output => 0x39,
        };
        seed.wrapping_add((offset as u8).wrapping_mul(17))
    }
}

pub(super) struct GuardedBuffer {
    storage: Vec<u8>,
    role: BufferRole,
}
impl GuardedBuffer {
    fn allocate(role: BufferRole) -> Result<Self, ControlError> {
        let size = BUFFER_BYTES.checked_add(GUARD_BYTES * 2).ok_or_else(|| {
            ControlError::new(
                "windows_fn_buffer_allocation_failed",
                "Fn buffer size overflow",
            )
        })?;
        let mut storage = Vec::new();
        storage.try_reserve_exact(size).map_err(|error| {
            ControlError::new(
                "windows_fn_buffer_allocation_failed",
                format!("Unable to allocate bounded Fn {role:?} buffer: {error}"),
            )
        })?;
        storage.resize(size, OUTPUT_INITIAL_BYTE);
        let mut buffer = Self { storage, role };
        for offset in 0..GUARD_BYTES {
            buffer.storage[offset] = role.guard(offset);
            buffer.storage[GUARD_BYTES + BUFFER_BYTES + offset] = role.guard(GUARD_BYTES + offset);
        }
        Ok(buffer)
    }
    pub(super) fn payload(&self) -> &[u8] {
        &self.storage[GUARD_BYTES..GUARD_BYTES + BUFFER_BYTES]
    }
    pub(super) fn payload_mut(&mut self) -> &mut [u8] {
        &mut self.storage[GUARD_BYTES..GUARD_BYTES + BUFFER_BYTES]
    }
    fn guards_intact(&self) -> bool {
        (0..GUARD_BYTES).all(|offset| {
            self.storage[offset] == self.role.guard(offset)
                && self.storage[GUARD_BYTES + BUFFER_BYTES + offset]
                    == self.role.guard(GUARD_BYTES + offset)
        })
    }
}

pub(super) trait BufferAllocator {
    fn allocate(&mut self, role: BufferRole) -> Result<GuardedBuffer, ControlError>;
}
pub(super) struct SystemAllocator;
impl BufferAllocator for SystemAllocator {
    fn allocate(&mut self, role: BufferRole) -> Result<GuardedBuffer, ControlError> {
        GuardedBuffer::allocate(role)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CallCompletion {
    pub succeeded: bool,
    pub returned_bytes: u32,
    /// Captured immediately on the calling thread, before any other Win32 call.
    pub win32_error: Option<u32>,
}
pub(super) trait Transport {
    fn invoke(
        &mut self,
        operation: FnOperation,
        input: &mut GuardedBuffer,
        output: &mut GuardedBuffer,
    ) -> CallCompletion;
}

pub(super) struct FnChannel<T, A = SystemAllocator> {
    pub(super) transport: T,
    allocator: A,
}
impl<T: Transport> FnChannel<T> {
    pub(super) fn new(transport: T) -> Self {
        Self {
            transport,
            allocator: SystemAllocator,
        }
    }
}
impl<T: Transport, A: BufferAllocator> FnChannel<T, A> {
    fn buffers(
        &mut self,
        operation: FnOperation,
    ) -> Result<(GuardedBuffer, GuardedBuffer), ControlError> {
        let mut input = self.allocator.allocate(BufferRole::Input)?;
        let output = self.allocator.allocate(BufferRole::Output)?;
        input.payload_mut().fill(0);
        input.payload_mut()[..4].copy_from_slice(&FN_ADDRESS.to_le_bytes());
        if let FnOperation::Write(value) = operation {
            input.payload_mut()[4..8].copy_from_slice(&u32::from(value).to_le_bytes());
        }
        Ok((input, output))
    }
    fn complete(
        operation: FnOperation,
        call: CallCompletion,
        input: &GuardedBuffer,
        output: &GuardedBuffer,
    ) -> Result<TransferEvidence, ControlError> {
        let input_unchanged = input
            .payload()
            .iter()
            .enumerate()
            .all(|(offset, value)| *value == operation.expected_input(offset));
        let mut changed = 0_u32;
        let mut first = None;
        let mut last = None;
        let mut tail_matches = true;
        for (offset, value) in output.payload().iter().enumerate() {
            if *value != operation.expected_input(offset) {
                changed += 1;
                first.get_or_insert(offset as u32);
                last = Some(offset as u32);
                if offset >= 4 {
                    tail_matches = false;
                }
            }
        }
        let evidence = TransferEvidence {
            input_bytes: BUFFER_BYTES as u32,
            output_capacity_bytes: BUFFER_BYTES as u32,
            returned_bytes: call.returned_bytes,
            input_guard_intact: input.guards_intact(),
            output_guard_intact: output.guards_intact(),
            input_unchanged,
            output_tail_matches_input: tail_matches,
            output_prefix_le: u32::from_le_bytes(
                output.payload()[..4]
                    .try_into()
                    .expect("fixed four-byte prefix"),
            ),
            changed_output_bytes: changed,
            first_changed_output_offset: first,
            last_changed_output_offset: last,
        };
        let failure = if !call.succeeded {
            Some(win32_error(operation.name(), call.win32_error.unwrap_or(0)))
        } else if !evidence.input_guard_intact || !evidence.output_guard_intact {
            Some(ControlError::new(
                "windows_fn_transfer_guard_corrupted",
                "Fn transport buffer guard changed; hardware state is unknown",
            ))
        } else if !input_unchanged {
            Some(ControlError::new(
                "windows_fn_input_changed",
                "The Fn input buffer changed unexpectedly",
            ))
        } else if call.returned_bytes != BUFFER_BYTES as u32 {
            Some(ControlError::new(
                "windows_fn_transfer_length_mismatch",
                "Fn transport did not return the fixed 4MiB completion length",
            ))
        } else if !tail_matches {
            Some(ControlError::new(
                "windows_fn_output_unexpected_changes",
                "Fn output changed outside the allowed DWORD prefix",
            ))
        } else if call.win32_error.is_some() {
            Some(ControlError::new(
                "windows_fn_invalid_call_evidence",
                "Successful Fn call carried contradictory failure evidence",
            ))
        } else {
            None
        };
        if let Some(mut error) = failure {
            error.details.push(format!("operation={} input_guard={} output_guard={} input_unchanged={} returned_bytes={} expected_bytes={} output_tail_matches_input={} changed_bytes={} first_change={:?} last_change={:?}", operation.name(), evidence.input_guard_intact, evidence.output_guard_intact, input_unchanged, call.returned_bytes, BUFFER_BYTES, tail_matches, changed, first, last));
            // Unexpected output is intentionally not included in diagnostics.
            return Err(error);
        }
        Ok(evidence)
    }
}
impl<T: Transport, A: BufferAllocator> FnDevice for FnChannel<T, A> {
    fn read_fn_byte(&mut self) -> Result<FnByteRead, ControlError> {
        let operation = FnOperation::Read;
        let (mut input, mut output) = self.buffers(operation)?;
        let call = self.transport.invoke(operation, &mut input, &mut output);
        let transfer = Self::complete(operation, call, &input, &output)?;
        let value = u8::try_from(transfer.output_prefix_le).map_err(|_| {
            let code = if transfer.output_prefix_le == FN_ADDRESS { "windows_fn_read_address_echo" } else { "windows_fn_read_out_of_range" };
            let mut error = ControlError::new(code, "Fn read did not produce a complete DWORD byte value; no hardware reading is available");
            error.details.push(format!("allowed_prefix_dword=0x{:08x}", transfer.output_prefix_le));
            error
        })?;
        Ok(FnByteRead { value, transfer })
    }
    fn write_fn_byte(&mut self, value: u8) -> WriteSubmission {
        let operation = FnOperation::Write(value);
        let (mut input, mut output) = match self.buffers(operation) {
            Ok(buffers) => buffers,
            Err(error) => return WriteSubmission::NotSubmitted(error),
        };
        let call = self.transport.invoke(operation, &mut input, &mut output);
        WriteSubmission::Submitted(Self::complete(operation, call, &input, &output))
    }
}

pub(super) fn win32_error(stage: &str, value: u32) -> ControlError {
    let code = match (stage, value) {
        ("open", 2 | 3) => "windows_fn_device_not_found",
        ("open", 5) => "windows_fn_device_access_denied",
        ("open", 32 | 33) => "windows_fn_device_busy",
        ("open", _) => "windows_fn_device_open_failed",
        ("close", _) => "windows_fn_device_close_failed",
        (_, 5) => "windows_fn_ioctl_access_denied",
        (_, 1 | 50) => "windows_fn_ioctl_not_supported",
        (_, 87) => "windows_fn_ioctl_invalid_parameter",
        (_, 122) => "windows_fn_ioctl_insufficient_buffer",
        (_, 234) => "windows_fn_ioctl_more_data",
        (_, 6) => "windows_fn_invalid_handle",
        (_, 55 | 1167) => "windows_fn_device_disconnected",
        _ => "windows_fn_ioctl_failed",
    };
    let mut error = ControlError::new(code, format!("Fixed Windows Fn {stage} call failed (Win32 {value}); no hardware acknowledgement is implied"));
    error
        .details
        .push(format!("stage={stage} win32_error={value}"));
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Clone, Copy)]
    enum Fault {
        None,
        InputGuard,
        OutputGuard,
        InputPayload,
        Tail,
        NoCopy,
    }
    struct FakeTransport {
        value: u32,
        returned: u32,
        error: Option<u32>,
        fault: Fault,
        calls: Vec<FnOperation>,
    }
    impl FakeTransport {
        fn new(value: u32) -> Self {
            Self {
                value,
                returned: BUFFER_BYTES as u32,
                error: None,
                fault: Fault::None,
                calls: vec![],
            }
        }
    }
    impl Transport for FakeTransport {
        fn invoke(
            &mut self,
            operation: FnOperation,
            input: &mut GuardedBuffer,
            output: &mut GuardedBuffer,
        ) -> CallCompletion {
            self.calls.push(operation);
            assert_eq!(input.payload().len(), 4_194_304);
            assert_eq!(output.payload().len(), 4_194_304);
            assert_eq!(&input.payload()[..4], &[0x4e, 0x07, 0, 0]);
            assert!(input.guards_intact() && output.guards_intact());
            match operation {
                FnOperation::Read => assert!(input.payload()[4..].iter().all(|byte| *byte == 0)),
                FnOperation::Write(value) => {
                    assert_eq!(&input.payload()[4..8], &[value, 0, 0, 0]);
                    assert!(input.payload()[8..].iter().all(|byte| *byte == 0));
                }
            }
            assert!(output
                .payload()
                .iter()
                .all(|byte| *byte == OUTPUT_INITIAL_BYTE));
            if !matches!(self.fault, Fault::NoCopy) {
                output.payload_mut().copy_from_slice(input.payload());
                output.payload_mut()[..4].copy_from_slice(&self.value.to_le_bytes());
            }
            match self.fault {
                Fault::InputGuard => input.storage[0] ^= 1,
                Fault::OutputGuard => output.storage[GUARD_BYTES + BUFFER_BYTES] ^= 1,
                Fault::InputPayload => input.payload_mut()[17] ^= 1,
                Fault::Tail => output.payload_mut()[BUFFER_BYTES - 1] ^= 1,
                _ => {}
            }
            CallCompletion {
                succeeded: self.error.is_none(),
                returned_bytes: self.returned,
                win32_error: self.error,
            }
        }
    }
    #[test]
    fn fixed_abi_and_complete_dword_preserve_legitimate_low_byte_echo() {
        assert_eq!(DEVICE_PATH, r"\\.\ACPIDriver");
        assert_eq!(DEVICE_PATH_WIDE.last(), Some(&0));
        assert_eq!(
            String::from_utf16(&DEVICE_PATH_WIDE[..14]).unwrap(),
            DEVICE_PATH
        );
        assert_eq!(super::super::contract::FN_MASK, 0x10);
        assert_eq!(FnOperation::Read.ioctl(), 0x9c40_a488);
        assert_eq!(FnOperation::Write(255).ioctl(), 0x9c40_a48c);
        for value in [0, 0x4e, 255] {
            let mut channel = FnChannel::new(FakeTransport::new(value));
            let read = channel.read_fn_byte().unwrap();
            assert_eq!(u32::from(read.value), value);
            assert_eq!(read.transfer.output_prefix_le, value);
            assert!(read.transfer.output_tail_matches_input);
            assert!(read.transfer.last_changed_output_offset.unwrap() < 4);
            assert_eq!(channel.transport.calls, [FnOperation::Read]);
        }
    }
    #[test]
    fn address_echo_and_nonzero_high_bytes_are_unknown_instead_of_truncated() {
        for (value, expected) in [
            (FN_ADDRESS, "windows_fn_read_address_echo"),
            (256, "windows_fn_read_out_of_range"),
            (0x1234_004e, "windows_fn_read_out_of_range"),
        ] {
            let mut channel = FnChannel::new(FakeTransport::new(value));
            assert_eq!(channel.read_fn_byte().unwrap_err().code, expected);
            assert_eq!(channel.transport.calls.len(), 1);
        }
    }
    #[test]
    fn exact_completion_length_guards_input_and_unexplained_output_are_required() {
        for returned in [
            0,
            3,
            BUFFER_BYTES as u32 - 1,
            BUFFER_BYTES as u32 + 1,
            u32::MAX,
        ] {
            let mut fake = FakeTransport::new(1);
            fake.returned = returned;
            assert_eq!(
                FnChannel::new(fake).read_fn_byte().unwrap_err().code,
                "windows_fn_transfer_length_mismatch"
            );
        }
        for (fault, code) in [
            (Fault::InputGuard, "windows_fn_transfer_guard_corrupted"),
            (Fault::OutputGuard, "windows_fn_transfer_guard_corrupted"),
            (Fault::InputPayload, "windows_fn_input_changed"),
            (Fault::Tail, "windows_fn_output_unexpected_changes"),
            (Fault::NoCopy, "windows_fn_output_unexpected_changes"),
        ] {
            let mut fake = FakeTransport::new(1);
            fake.fault = fault;
            let error = FnChannel::new(fake).read_fn_byte().unwrap_err();
            assert_eq!(error.code, code);
            assert!(
                error.details.iter().all(|value| value.len() < 1024),
                "Unexpected raw output must not be dumped"
            );
        }
    }
    #[test]
    fn writes_only_submit_once_and_never_convert_driver_completion_into_ack() {
        for value in [0, 0x10, 255] {
            // An unchanged METHOD_BUFFERED prefix can mean swallowed ECRW
            // failure. It remains submission evidence, not a verified write.
            let mut channel = FnChannel::new(FakeTransport::new(FN_ADDRESS));
            let WriteSubmission::Submitted(Ok(evidence)) = channel.write_fn_byte(value) else {
                panic!("Expected transport submission only");
            };
            assert_eq!(evidence.output_prefix_le, FN_ADDRESS);
            assert_eq!(channel.transport.calls, [FnOperation::Write(value)]);
        }
    }
    #[test]
    fn failed_win32_write_remains_submitted_and_retains_original_error() {
        for (code, expected) in [
            (5, "windows_fn_ioctl_access_denied"),
            (1, "windows_fn_ioctl_not_supported"),
            (50, "windows_fn_ioctl_not_supported"),
            (6, "windows_fn_invalid_handle"),
            (87, "windows_fn_ioctl_invalid_parameter"),
            (122, "windows_fn_ioctl_insufficient_buffer"),
            (234, "windows_fn_ioctl_more_data"),
            (1167, "windows_fn_device_disconnected"),
            (0xdead, "windows_fn_ioctl_failed"),
        ] {
            let mut fake = FakeTransport::new(FN_ADDRESS);
            fake.error = Some(code);
            fake.fault = Fault::OutputGuard;
            let mut channel = FnChannel::new(fake);
            let WriteSubmission::Submitted(Err(error)) = channel.write_fn_byte(0x10) else {
                panic!("FALSE after submission must not become NotSubmitted");
            };
            assert_eq!(error.code, expected);
            assert!(error
                .details
                .iter()
                .any(|line| line.contains(&format!("win32_error={code}"))));
            assert!(error
                .details
                .iter()
                .any(|line| line.contains("output_guard=false")));
            assert_eq!(channel.transport.calls, [FnOperation::Write(0x10)]);
        }
    }
    struct FailAllocation {
        attempt: usize,
        fail_at: usize,
    }
    impl BufferAllocator for FailAllocation {
        fn allocate(&mut self, role: BufferRole) -> Result<GuardedBuffer, ControlError> {
            self.attempt += 1;
            if self.attempt == self.fail_at {
                Err(ControlError::new(
                    "windows_fn_buffer_allocation_failed",
                    "Injected production allocation boundary failure",
                ))
            } else {
                GuardedBuffer::allocate(role)
            }
        }
    }
    #[test]
    fn either_allocation_failure_is_before_io_for_read_and_write() {
        for fail_at in [1, 2] {
            let mut channel = FnChannel {
                transport: FakeTransport::new(1),
                allocator: FailAllocation {
                    attempt: 0,
                    fail_at,
                },
            };
            assert_eq!(
                channel.read_fn_byte().unwrap_err().code,
                "windows_fn_buffer_allocation_failed"
            );
            assert!(channel.transport.calls.is_empty());
            channel.allocator.attempt = 0;
            let WriteSubmission::NotSubmitted(error) = channel.write_fn_byte(1) else {
                panic!("Allocation failure must never submit a write");
            };
            assert_eq!(error.code, "windows_fn_buffer_allocation_failed");
            assert!(channel.transport.calls.is_empty());
        }
    }
    #[test]
    fn open_errors_keep_stage_and_unknown_numeric_identity() {
        for (raw, code) in [
            (2, "windows_fn_device_not_found"),
            (3, "windows_fn_device_not_found"),
            (5, "windows_fn_device_access_denied"),
            (32, "windows_fn_device_busy"),
            (33, "windows_fn_device_busy"),
            (0xdead, "windows_fn_device_open_failed"),
        ] {
            let error = win32_error("open", raw);
            assert_eq!(error.code, code);
            assert_eq!(error.details, [format!("stage=open win32_error={raw}")]);
        }
    }
}
