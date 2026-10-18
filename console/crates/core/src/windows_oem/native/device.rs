//! Win32 adapter for the one qualified Fn-lock device. No DLL, subprocess,
//! driver installation, alternate device path, arbitrary address or IOCTL.
use super::identity::QualifiedDriverGuard;
use crate::{
    windows_oem::device::{
        win32_error, CallCompletion, FnByteRead, FnChannel, FnDevice, FnOperation, GuardedBuffer,
        Transport, WriteSubmission, BUFFER_BYTES, DEVICE_PATH_WIDE,
    },
    ControlError,
};
use std::{ptr, sync::Arc};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    },
    System::IO::DeviceIoControl,
};

pub(super) struct Device {
    // Field order closes the device before releasing its protected driver file
    // and qualified identity. There is no constructor without this private guard.
    channel: FnChannel<WindowsTransport>,
    _guard: Arc<QualifiedDriverGuard>,
}
impl Device {
    pub(super) fn open(guard: Arc<QualifiedDriverGuard>) -> Result<Self, ControlError> {
        // Fixed ASCII device path encoded without allocation or environment input.
        // SAFETY: PATH is fixed and NUL-terminated; null SECURITY_ATTRIBUTES
        // prevents inheritance; OPEN_EXISTING never creates or installs a driver.
        // No OVERLAPPED flag is used. Sharing is the documented normal-driver
        // choice, not a lock against OEM/firmware writers; no fallback is tried.
        let handle = unsafe {
            CreateFileW(
                DEVICE_PATH_WIDE.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            // SAFETY: capture on this thread immediately after failed CreateFile.
            return Err(win32_error("open", unsafe { GetLastError() }));
        }
        Ok(Self {
            channel: FnChannel::new(WindowsTransport {
                handle: Some(handle),
            }),
            _guard: guard,
        })
    }
    /// The identity owner can preserve close failures separately from the
    /// transaction's primary/readback/restore errors. Never retries a failed close.
    pub(super) fn close(&mut self) -> Result<(), ControlError> {
        self.channel.transport.close()
    }
    fn require_open(&self) -> Result<(), ControlError> {
        if self.channel.transport.handle.is_some() {
            Ok(())
        } else {
            Err(ControlError::new(
                "windows_fn_device_closed",
                "The held Fn device handle has already been closed",
            ))
        }
    }
}
impl FnDevice for Device {
    fn read_fn_byte(&mut self) -> Result<FnByteRead, ControlError> {
        self.require_open()?;
        self.channel.read_fn_byte()
    }
    fn write_fn_byte(&mut self, value: u8) -> WriteSubmission {
        if let Err(error) = self.require_open() {
            return WriteSubmission::NotSubmitted(error);
        }
        self.channel.write_fn_byte(value)
    }
}

struct WindowsTransport {
    handle: Option<HANDLE>,
}
impl WindowsTransport {
    fn close(&mut self) -> Result<(), ControlError> {
        if let Some(handle) = self.handle.take() {
            // SAFETY: this object alone owns the valid non-inherited handle;
            // take() ensures at most one CloseHandle attempt, including Drop.
            if unsafe { CloseHandle(handle) } == 0 {
                // SAFETY: immediate same-thread failure capture.
                return Err(win32_error("close", unsafe { GetLastError() }));
            }
        }
        Ok(())
    }
}
impl Drop for WindowsTransport {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
impl Transport for WindowsTransport {
    fn invoke(
        &mut self,
        operation: FnOperation,
        input: &mut GuardedBuffer,
        output: &mut GuardedBuffer,
    ) -> CallCompletion {
        let handle = self
            .handle
            .expect("Device checks its held handle before transport");
        let mut returned_bytes = 0_u32;
        // SAFETY: both initialized, separately owned payloads are exactly 4MiB,
        // remain allocated until this synchronous call returns, and have checked
        // user-side canaries. METHOD_BUFFERED copies the initialized input image
        // to its shared system buffer. Canaries do not prove kernel/source safety.
        // A nonnull BytesReturned and null OVERLAPPED are required for sync I/O.
        let succeeded = unsafe {
            DeviceIoControl(
                handle,
                operation.ioctl(),
                input.payload().as_ptr().cast(),
                BUFFER_BYTES as u32,
                output.payload_mut().as_mut_ptr().cast(),
                BUFFER_BYTES as u32,
                &mut returned_bytes,
                ptr::null_mut(),
            )
        } != 0;
        // GetLastError is not meaningful on TRUE; never retrieve a stale value.
        let win32_error = if succeeded {
            None
        } else {
            Some(unsafe { GetLastError() })
        };
        CallCompletion {
            succeeded,
            returned_bytes,
            win32_error,
        }
    }
}
