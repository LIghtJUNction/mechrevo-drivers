//! Platform-independent rules used by the native Windows IPC implementation.
//! These decisions do not open files, pipes, tokens, processes or hardware.
use super::ClientHelperLifetime;
use crate::ControlError;
use std::sync::atomic::{AtomicBool, Ordering};

// Specific pipe client rights only: data read/write, read attributes, synchronize.
// FILE_APPEND_DATA is also FILE_CREATE_PIPE_INSTANCE and must never be granted.
pub(crate) const PIPE_CLIENT_ACCESS: u32 = 0x0010_0083;

fn rejected(message: &str) -> ControlError {
    ControlError::new("windows_oem_ipc", message)
}

pub(crate) fn valid_nonce(nonce: &str) -> bool {
    nonce.len() == 64
        && nonce
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}

pub(crate) fn pipe_name(nonce: &str) -> Result<String, ControlError> {
    if !valid_nonce(nonce) {
        return Err(rejected("Invalid fixed OEM session nonce"));
    }
    Ok(format!(r"\\.\pipe\MechrevoConsole.Oem.v1.{nonce}"))
}

pub(crate) fn frame_length(prefix: [u8; 4], limit: usize) -> Result<usize, ControlError> {
    let length = u32::from_le_bytes(prefix) as usize;
    if length == 0 || length > limit {
        return Err(rejected(
            "Pipe frame length is outside the fixed byte bound",
        ));
    }
    Ok(length)
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LaunchDecision {
    Tracked,
    TrackedFailure,
    CancelledBeforeStart,
    Untracked,
}

/// `explicitly_cancelled` is evidence supplied by the native adapter from
/// ERROR_CANCELLED. It never overrides an actual returned process handle or
/// turns successful launch without a handle into confirmed cancellation.
pub(crate) fn launch_decision(
    api_succeeded: bool,
    has_process: bool,
    explicitly_cancelled: bool,
) -> LaunchDecision {
    match (api_succeeded, has_process, explicitly_cancelled) {
        (true, true, _) => LaunchDecision::Tracked,
        (false, true, _) => LaunchDecision::TrackedFailure,
        (false, false, true) => LaunchDecision::CancelledBeforeStart,
        _ => LaunchDecision::Untracked,
    }
}

/// A process-wide preparation claim. Transferring it hands the already-held
/// gate to the pending exchange, its orphan, and its adopter without a gap.
/// Only actual Reaped / known NotStarted may release a transferred gate.
pub(crate) struct Reservation<'a> {
    gate: &'a AtomicBool,
    transferred: bool,
}
impl<'a> Reservation<'a> {
    pub(crate) fn acquire(gate: &'a AtomicBool) -> Option<Self> {
        gate.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self {
                gate,
                transferred: false,
            })
    }
    pub(crate) fn transfer(&mut self) {
        self.transferred = true;
    }
}
impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        if !self.transferred {
            self.gate.store(false, Ordering::Release);
        }
    }
}

pub(crate) fn unowned_lifetime(active: bool) -> ClientHelperLifetime {
    if active {
        ClientHelperLifetime::Unknown
    } else {
        ClientHelperLifetime::Reaped
    }
}

/// Return true only after completion evidence permits releasing the launch
/// gate. Running and Unknown retain the existing process-wide reservation.
pub(crate) fn release_after_completion(gate: &AtomicBool, lifetime: ClientHelperLifetime) -> bool {
    if matches!(
        lifetime,
        ClientHelperLifetime::Reaped | ClientHelperLifetime::NotStarted
    ) {
        gate.store(false, Ordering::Release);
        true
    } else {
        false
    }
}

/// Native installation paths must begin at a direct fixed-volume root. The
/// native adapter separately obtains this name from the held root file handle.
pub(crate) fn require_real_volume_root(native: &[u16]) -> Result<(), ControlError> {
    fn reject(message: &str) -> ControlError {
        ControlError::new("windows_oem_installation", message)
    }
    let text = String::from_utf16(native).map_err(|_| reject("Invalid local volume root name"))?;
    let volume = text
        .trim_end_matches('\\')
        .strip_prefix("\\Device\\HarddiskVolume")
        .ok_or_else(|| reject("The installation drive is not a direct local disk volume"))?;
    if volume.is_empty() || !volume.bytes().all(|v| v.is_ascii_digit()) {
        return Err(reject(
            "A redirected or SUBST drive cannot host this installation",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows_oem::protocol::{MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};
    use std::sync::{atomic::AtomicUsize, Arc, Barrier};

    #[test]
    fn shell_failure_with_a_real_handle_requires_close_and_wait() {
        assert_eq!(launch_decision(true, true, false), LaunchDecision::Tracked);
        assert_eq!(launch_decision(true, true, true), LaunchDecision::Tracked);
        assert_eq!(
            launch_decision(false, true, false),
            LaunchDecision::TrackedFailure
        );
        assert_eq!(
            launch_decision(false, true, true),
            LaunchDecision::TrackedFailure
        );
        assert_eq!(
            launch_decision(false, false, true),
            LaunchDecision::CancelledBeforeStart
        );
        assert_eq!(
            launch_decision(false, false, false),
            LaunchDecision::Untracked
        );
        assert_eq!(
            launch_decision(true, false, false),
            LaunchDecision::Untracked
        );
        assert_eq!(
            launch_decision(true, false, true),
            LaunchDecision::Untracked
        );
    }

    #[test]
    fn frame_limits_reject_empty_and_oversized_prefixes_before_allocation() {
        for maximum in [MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES] {
            assert_eq!(frame_length(1u32.to_le_bytes(), maximum).unwrap(), 1);
            assert_eq!(
                frame_length((maximum as u32).to_le_bytes(), maximum).unwrap(),
                maximum
            );
            for invalid in [0, maximum as u32 + 1, u32::MAX] {
                assert!(frame_length(invalid.to_le_bytes(), maximum).is_err());
            }
        }
        // The frame prefix is explicitly little endian, not native endian.
        assert_eq!(frame_length([0, 1, 0, 0], MAX_REQUEST_BYTES).unwrap(), 256);
    }

    #[test]
    fn nonce_is_exact_lowercase_csprng_encoding_without_path_or_argument_syntax() {
        let valid = "01abcdef".repeat(8);
        assert!(valid_nonce(&valid));
        assert_eq!(
            pipe_name(&valid).unwrap(),
            format!(r"\\.\pipe\MechrevoConsole.Oem.v1.{valid}")
        );
        for invalid in [
            "a".repeat(63),
            "a".repeat(65),
            "A".repeat(64),
            format!("{} ", "a".repeat(63)),
            format!("{}\\", "a".repeat(63)),
            format!("{}\t", "a".repeat(63)),
            "\0".repeat(64),
            "а".repeat(32),
        ] {
            assert!(!valid_nonce(&invalid));
            assert!(pipe_name(&invalid).is_err());
        }
    }

    #[test]
    fn client_permissions_cannot_create_another_server_or_change_its_acl() {
        assert_eq!(PIPE_CLIENT_ACCESS, 0x0010_0083);
        assert_eq!(
            PIPE_CLIENT_ACCESS & (0x4 | 0x0004_0000 | 0x0008_0000 | 0x4000_0000),
            0
        );
    }

    #[test]
    fn reservation_survives_orphan_adoption_until_actual_reap() {
        let gate = AtomicBool::new(false);
        let mut owner = Reservation::acquire(&gate).unwrap();
        assert!(Reservation::acquire(&gate).is_none());
        owner.transfer();
        drop(owner); // Pending -> orphan -> adopter keeps the same reservation.
        assert!(Reservation::acquire(&gate).is_none());
        assert_eq!(
            unowned_lifetime(gate.load(Ordering::Acquire)),
            ClientHelperLifetime::Unknown
        );
        assert!(!release_after_completion(
            &gate,
            ClientHelperLifetime::Unknown
        ));
        assert!(Reservation::acquire(&gate).is_none());
        assert!(release_after_completion(
            &gate,
            ClientHelperLifetime::Reaped
        ));
        assert_eq!(
            unowned_lifetime(gate.load(Ordering::Acquire)),
            ClientHelperLifetime::Reaped
        );
        let preparation = Reservation::acquire(&gate).unwrap();
        drop(preparation); // Failure before launch releases only this preparation.
        assert!(!gate.load(Ordering::Acquire));
    }

    #[test]
    fn only_actual_reap_or_confirmed_no_launch_releases_the_reservation() {
        for lifetime in [ClientHelperLifetime::Running, ClientHelperLifetime::Unknown] {
            let gate = AtomicBool::new(true);
            assert!(!release_after_completion(&gate, lifetime));
            assert!(gate.load(Ordering::Acquire));
            assert!(Reservation::acquire(&gate).is_none());
        }
        for lifetime in [
            ClientHelperLifetime::Reaped,
            ClientHelperLifetime::NotStarted,
        ] {
            let gate = AtomicBool::new(true);
            assert!(release_after_completion(&gate, lifetime));
            assert!(!gate.load(Ordering::Acquire));
            assert!(Reservation::acquire(&gate).is_some());
        }
    }

    #[test]
    fn concurrent_transports_cannot_reserve_two_helper_launches() {
        let gate = Arc::new(AtomicBool::new(false));
        let start = Arc::new(Barrier::new(9));
        let claims = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let (gate, start, claims) =
                    (Arc::clone(&gate), Arc::clone(&start), Arc::clone(&claims));
                scope.spawn(move || {
                    start.wait();
                    if let Some(mut claim) = Reservation::acquire(&gate) {
                        claims.fetch_add(1, Ordering::SeqCst);
                        claim.transfer();
                    }
                });
            }
            start.wait();
        });
        assert_eq!(claims.load(Ordering::SeqCst), 1);
        assert_eq!(
            unowned_lifetime(gate.load(Ordering::Acquire)),
            ClientHelperLifetime::Unknown
        );
    }

    #[test]
    fn installation_volume_rejects_network_and_redirected_subdirectory_paths() {
        for accepted in [r"\Device\HarddiskVolume1\", r"\Device\HarddiskVolume123"] {
            assert!(require_real_volume_root(&accepted.encode_utf16().collect::<Vec<_>>()).is_ok());
        }
        for rejected in [
            r"\Device\HarddiskVolume1\subdir",
            r"\Device\HarddiskVolume",
            r"\Device\Mup\server\share",
            r"C:\",
        ] {
            assert!(
                require_real_volume_root(&rejected.encode_utf16().collect::<Vec<_>>()).is_err()
            );
        }
        assert!(require_real_volume_root(&[0xd800]).is_err());
    }
}
