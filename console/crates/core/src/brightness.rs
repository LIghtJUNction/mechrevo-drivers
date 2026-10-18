//! Operating-system display brightness. No EC, GPU or panel firmware writes.
//!
//! Linux distinguishes the requested `brightness` from `actual_brightness`:
//! readback of the former confirms a request, not a physical luminance claim.
use crate::{CapabilityStatus, ControlError, EvidenceSource};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrightnessDevice {
    pub id: String,
    pub label: String,
    pub current_percent: Option<f64>,
    pub control_value: Option<u32>,
    pub actual_value: Option<u32>,
    pub maximum: Option<u32>,
    pub source: EvidenceSource,
    pub status: CapabilityStatus,
    pub readable: bool,
    /// Direct OS file permission, never inferred from a delegated write route.
    pub writable: bool,
    pub write_backend: Option<BrightnessBackend>,
    pub logind_session: Option<LogindSessionBinding>,
    /// Empty on Linux, where percentages are quantized to raw backlight steps.
    pub supported_percentages: Vec<u8>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrightnessBackend {
    LinuxSysfs,
    LinuxLogind,
    WindowsWmi,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogindSessionBinding {
    pub session_id: String,
    pub object_path: String,
    pub uid: u32,
    pub seat_id: String,
    pub seat_path: String,
}

impl BrightnessDevice {
    /// An observed route is eligible for preview, not a promise that a future
    /// setter will be authorized. Apply rechecks all session/device state.
    pub fn can_control(&self) -> bool {
        self.readable && self.write_backend.is_some()
    }
}

/// Serializable for review, but never trusted without revalidation by `apply`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrightnessPlan {
    pub device_id: String,
    pub requested_percent: u8,
    pub target: u32,
    pub previous: u32,
    pub maximum: u32,
    pub source: EvidenceSource,
    pub backend: BrightnessBackend,
    pub logind_session: Option<LogindSessionBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrightnessApplyResult {
    pub device_id: String,
    pub requested_percent: u8,
    pub confirmed: u32,
    pub previous: u32,
    pub actual_value: Option<u32>,
    pub source: EvidenceSource,
    pub transaction: BrightnessTransaction,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrightnessOutcome {
    Confirmed,
    NotApplied,
    Conflict,
    Restored,
    RecoveryFailed,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrightnessRecovery {
    NotNeeded,
    Skipped,
    Restored,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrightnessTransaction {
    pub outcome: BrightnessOutcome,
    pub write_attempted: bool,
    pub writer_may_continue: bool,
    pub observed_control: Option<u32>,
    pub primary_error: Option<ControlError>,
    pub rollback_error: Option<ControlError>,
    pub readback_error: Option<ControlError>,
    pub recovery: BrightnessRecovery,
    pub recovery_reason: Option<String>,
}

impl BrightnessTransaction {
    pub fn blocks_further_writes(&self) -> bool {
        self.writer_may_continue
            || matches!(
                self.outcome,
                BrightnessOutcome::Unknown | BrightnessOutcome::RecoveryFailed
            )
    }
}

pub const BRIGHTNESS_TRANSACTION_MARKER: &str = "brightness_transaction=";

pub fn transaction_from_error(error: &ControlError) -> Option<BrightnessTransaction> {
    error.details.iter().find_map(|detail| {
        detail
            .strip_prefix(BRIGHTNESS_TRANSACTION_MARKER)
            .and_then(|json| serde_json::from_str(json).ok())
    })
}

#[cfg(target_os = "linux")]
#[path = "brightness/logind.rs"]
mod logind;

fn unsupported() -> ControlError {
    ControlError::new(
        "brightness_unavailable",
        "No operating-system brightness device is available",
    )
}

fn target(percent: u8, maximum: u32) -> Result<u32, ControlError> {
    if percent > 100 {
        return Err(ControlError::new(
            "invalid_brightness",
            "Brightness must be between 0 and 100 percent",
        ));
    }
    if maximum == 0 {
        return Err(ControlError::new(
            "invalid_brightness_device",
            "Backlight maximum must be positive",
        ));
    }
    Ok(((u64::from(maximum) * u64::from(percent) + 50) / 100) as u32)
}

fn select(
    devices: Vec<BrightnessDevice>,
    selected: Option<&str>,
) -> Result<BrightnessDevice, ControlError> {
    if let Some(id) = selected {
        return devices.into_iter().find(|d| d.id == id).ok_or_else(|| {
            ControlError::new(
                "brightness_device_missing",
                "The selected brightness device is unavailable",
            )
        });
    }
    if devices.len() > 1 {
        return Err(ControlError::new(
            "brightness_device_ambiguous",
            "Multiple brightness devices exist; select an explicit device ID",
        ));
    }
    devices.into_iter().next().ok_or_else(unsupported)
}

fn make_plan(
    device: BrightnessDevice,
    percent: u8,
    backend: BrightnessBackend,
) -> Result<BrightnessPlan, ControlError> {
    // Validate even when discovery fails, so invalid user input is never hidden.
    let requested = target(percent, device.maximum.unwrap_or(1))?;
    if !device.can_control() || device.write_backend != Some(backend) {
        return Err(ControlError::new(
            "brightness_unavailable",
            device.reason.unwrap_or_else(|| {
                "The brightness device cannot be read and written by this user".into()
            }),
        ));
    }
    if backend == BrightnessBackend::WindowsWmi && !device.supported_percentages.contains(&percent)
    {
        return Err(ControlError::new(
            "brightness_level_unsupported",
            format!(
                "The display does not advertise {percent}% brightness; available levels: {:?}",
                device.supported_percentages
            ),
        ));
    }
    Ok(BrightnessPlan {
        device_id: device.id,
        requested_percent: percent,
        target: requested,
        previous: device.control_value.ok_or_else(unsupported)?,
        maximum: device.maximum.ok_or_else(unsupported)?,
        source: device.source,
        backend,
        logind_session: device.logind_session,
    })
}

fn validate_plan(plan: &BrightnessPlan, fresh: &BrightnessPlan) -> Result<(), ControlError> {
    if plan.backend != fresh.backend
        || plan.device_id != fresh.device_id
        || plan.maximum != fresh.maximum
        || plan.source != fresh.source
        || plan.target != fresh.target
        || plan.logind_session != fresh.logind_session
    {
        return Err(ControlError::new(
            "invalid_brightness_plan",
            "The brightness plan does not match the selected device and validated target",
        ));
    }
    if plan.previous != fresh.previous {
        return Err(ControlError::new(
            "brightness_plan_stale",
            "Brightness changed since this plan was created; create a new plan",
        ));
    }
    Ok(())
}

/// Enumerate only OS backlight interfaces; no EDID serial, DMI serial or UUID is read.
pub fn devices(sysfs_root: &Path) -> Vec<BrightnessDevice> {
    #[cfg(target_os = "linux")]
    {
        linux::devices(sysfs_root)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = sysfs_root;
        windows::devices()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = sysfs_root;
        vec![]
    }
}

/// Dry-run: reads device state and permissions, and never writes brightness.
// @mark mechrevo.display_brightness role=implementation
pub fn plan(
    sysfs_root: &Path,
    percent: u8,
    device: Option<&str>,
) -> Result<BrightnessPlan, ControlError> {
    target(percent, 1)?;
    #[cfg(target_os = "linux")]
    {
        let selected = select(linux::devices(sysfs_root), device)?;
        let backend = selected
            .write_backend
            .unwrap_or(BrightnessBackend::LinuxSysfs);
        make_plan(selected, percent, backend)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = sysfs_root;
        make_plan(
            select(windows::devices(), device)?,
            percent,
            BrightnessBackend::WindowsWmi,
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = (sysfs_root, device);
        Err(unsupported())
    }
}

/// Revalidate the device, perform the request and verify its control value.
/// Unknown delegated writers and third-party values never authorize rollback.
/// Physical light output can lag behind the OS control value.
pub fn apply(
    sysfs_root: &Path,
    plan: &BrightnessPlan,
) -> Result<BrightnessApplyResult, ControlError> {
    let result = {
        #[cfg(target_os = "linux")]
        {
            linux::apply(sysfs_root, plan)
        }
        #[cfg(target_os = "windows")]
        {
            let _ = sysfs_root;
            windows::apply(plan)
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = (sysfs_root, plan);
            Err(unsupported())
        }
    };
    result.map_err(|error| {
        if transaction_from_error(&error).is_some() {
            error
        } else {
            transaction_error(
                error,
                BrightnessTransaction::new(BrightnessOutcome::NotApplied),
            )
        }
    })
}

#[derive(Debug)]
enum WriteFailure {
    NotStarted(ControlError),
    // Only Linux's synchronous sysfs store can fence its writer on return.
    // Windows provider failures remain Unknown; tests exercise the shared
    // local-store recovery policy on either platform.
    #[cfg_attr(all(target_os = "windows", not(test)), allow(dead_code))]
    Terminal(ControlError),
    Unknown(ControlError),
}

trait BrightnessIo {
    fn read_control(&mut self) -> Result<u32, ControlError>;
    fn write_control(&mut self, value: u32) -> Result<(), WriteFailure>;
    fn permits_conditional_recovery(&self) -> bool {
        false
    }
}

impl BrightnessTransaction {
    fn new(outcome: BrightnessOutcome) -> Self {
        Self {
            outcome,
            write_attempted: false,
            writer_may_continue: false,
            observed_control: None,
            primary_error: None,
            rollback_error: None,
            readback_error: None,
            recovery: BrightnessRecovery::NotNeeded,
            recovery_reason: None,
        }
    }
    fn read_failed(&mut self, error: ControlError) {
        if let Some(previous) = &mut self.readback_error {
            previous.details.push(format!(
                "additional_readback_error={}",
                serde_json::to_string(&error).expect("ControlError is JSON serializable")
            ));
        } else {
            self.readback_error = Some(error);
        }
    }
}

fn transaction_error(
    mut primary: ControlError,
    mut transaction: BrightnessTransaction,
) -> ControlError {
    if transaction.primary_error.is_none() {
        transaction.primary_error = Some(primary.clone());
    }
    if transaction.outcome == BrightnessOutcome::Unknown {
        primary.code = "brightness_outcome_unknown".into();
    } else if transaction.outcome == BrightnessOutcome::RecoveryFailed {
        primary.code = "brightness_recovery_failed".into();
    }
    primary.details.push(format!(
        "{BRIGHTNESS_TRANSACTION_MARKER}{}",
        serde_json::to_string(&transaction).expect("BrightnessTransaction is JSON serializable")
    ));
    primary
}

fn transact(
    io: &mut impl BrightnessIo,
    plan: &BrightnessPlan,
) -> Result<BrightnessTransaction, ControlError> {
    let mut tx = BrightnessTransaction::new(BrightnessOutcome::NotApplied);
    let current = io
        .read_control()
        .map_err(|error| transaction_error(error, tx.clone()))?;
    tx.observed_control = Some(current);
    if current != plan.previous {
        return Err(transaction_error(
            ControlError::new(
                "brightness_plan_stale",
                "Brightness changed before the write; create a new plan",
            ),
            tx,
        ));
    }
    if plan.target == plan.previous {
        tx.outcome = BrightnessOutcome::Confirmed;
        return Ok(tx);
    }
    tx.write_attempted = true;
    let primary = match io.write_control(plan.target) {
        Err(WriteFailure::NotStarted(error)) => {
            tx.write_attempted = false;
            return Err(transaction_error(error, tx));
        }
        Err(WriteFailure::Unknown(error)) => {
            tx.outcome = BrightnessOutcome::Unknown;
            tx.writer_may_continue = true;
            tx.recovery = BrightnessRecovery::Skipped;
            tx.recovery_reason = Some("A delegated writer may still run; an old read cannot prove cancellation, so no rollback was attempted".into());
            match io.read_control() {
                Ok(value) => tx.observed_control = Some(value),
                Err(read) => {
                    tx.observed_control = None;
                    tx.read_failed(read);
                }
            }
            return Err(transaction_error(error, tx));
        }
        Err(WriteFailure::Terminal(error)) => error,
        Ok(()) => match io.read_control() {
            Ok(value) if value == plan.target => {
                tx.observed_control = Some(value);
                tx.outcome = BrightnessOutcome::Confirmed;
                return Ok(tx);
            }
            Ok(value) => {
                tx.observed_control = Some(value);
                tx.outcome = BrightnessOutcome::Conflict;
                tx.recovery = BrightnessRecovery::Skipped;
                tx.recovery_reason = Some("Current brightness differs from this transaction's target; third-party state must not be overwritten".into());
                return Err(transaction_error(
                    ControlError::new(
                        "brightness_readback_mismatch",
                        format!(
                            "Requested raw value {}, but readback was {value}",
                            plan.target
                        ),
                    ),
                    tx,
                ));
            }
            Err(error) => {
                tx.observed_control = None;
                tx.read_failed(error.clone());
                error
            }
        },
    };
    tx.primary_error = Some(primary.clone());
    if !io.permits_conditional_recovery() {
        tx.outcome = BrightnessOutcome::Unknown;
        tx.recovery = BrightnessRecovery::Skipped;
        tx.recovery_reason = Some("The backend cannot fence remote/provider writers; automatic rollback was not attempted".into());
        return Err(transaction_error(primary, tx));
    }
    // A completed local syscall permits observation, not unconditional restore.
    // Never overwrite an unrecognized value or use a read failure as permission.
    match io.read_control() {
        Ok(value) if value == plan.target => tx.observed_control = Some(value),
        Ok(value) => {
            tx.observed_control = Some(value);
            tx.outcome = if value == plan.previous {
                BrightnessOutcome::Unknown
            } else {
                BrightnessOutcome::Conflict
            };
            tx.recovery = BrightnessRecovery::Skipped;
            tx.recovery_reason = Some(
                "Current brightness is not the transaction target; no recovery write was issued"
                    .into(),
            );
            return Err(transaction_error(primary, tx));
        }
        Err(error) => {
            tx.read_failed(error);
            tx.observed_control = None;
            tx.outcome = BrightnessOutcome::Unknown;
            tx.recovery = BrightnessRecovery::Skipped;
            tx.recovery_reason =
                Some("Cannot establish current brightness, so recovery cannot safely write".into());
            return Err(transaction_error(primary, tx));
        }
    }
    // Sysfs offers no compare-and-swap. A second immediate guard catches changes
    // observed before restoration; this does not claim atomic hardware ownership.
    match io.read_control() {
        Ok(value) if value == plan.target => {}
        Ok(value) => {
            tx.observed_control = Some(value);
            tx.outcome = BrightnessOutcome::Conflict;
            tx.recovery = BrightnessRecovery::Skipped;
            tx.recovery_reason =
                Some("Brightness changed before recovery; third-party state was preserved".into());
            return Err(transaction_error(primary, tx));
        }
        Err(error) => {
            tx.read_failed(error);
            tx.observed_control = None;
            tx.outcome = BrightnessOutcome::Unknown;
            tx.recovery = BrightnessRecovery::Skipped;
            return Err(transaction_error(primary, tx));
        }
    }
    let rollback = io.write_control(plan.previous);
    let rollback_ok = rollback.is_ok();
    match rollback {
        Err(WriteFailure::NotStarted(error) | WriteFailure::Terminal(error)) => {
            tx.rollback_error = Some(error)
        }
        Err(WriteFailure::Unknown(error)) => {
            tx.rollback_error = Some(error);
            tx.writer_may_continue = true;
        }
        Ok(()) => {}
    }
    let restored = match io.read_control() {
        Ok(value) => {
            tx.observed_control = Some(value);
            value == plan.previous
        }
        Err(error) => {
            tx.observed_control = None;
            tx.read_failed(error);
            false
        }
    };
    if rollback_ok && restored {
        tx.outcome = BrightnessOutcome::Restored;
        tx.recovery = BrightnessRecovery::Restored;
    } else {
        tx.outcome = BrightnessOutcome::RecoveryFailed;
        tx.recovery = if tx.observed_control.is_none() || tx.writer_may_continue {
            BrightnessRecovery::Unknown
        } else {
            BrightnessRecovery::Failed
        };
        tx.recovery_reason = Some("Recovery needs a successful write and matching readback; a write error cannot be cleared by an old cached value".into());
    }
    Err(transaction_error(primary, tx))
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::{
        ffi::CString,
        fs::{self, File, OpenOptions},
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::OpenOptionsExt,
        },
        path::{Component, PathBuf},
    };

    const O_NOFOLLOW: i32 = 0x20000;
    const O_DIRECTORY: i32 = 0x10000;
    const O_CLOEXEC: i32 = 0x80000;
    unsafe extern "C" {
        fn openat(dirfd: i32, pathname: *const std::ffi::c_char, flags: i32, ...) -> i32;
        fn faccessat(dirfd: i32, pathname: *const std::ffi::c_char, mode: i32, flags: i32) -> i32;
    }
    fn error(action: &str, cause: impl std::fmt::Display) -> ControlError {
        ControlError::new("brightness_io", format!("{action}: {cause}"))
    }
    fn valid_id(id: &str) -> bool {
        !id.is_empty()
            && id != "."
            && id != ".."
            && id.len() <= 255
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.:+".contains(&b))
    }

    struct Device {
        dir: File,
        canonical: PathBuf,
    }
    impl Device {
        fn open(root: &Path, id: &str) -> Result<Self, ControlError> {
            if !valid_id(id) {
                return Err(ControlError::new(
                    "invalid_brightness_device",
                    "A Linux brightness device must be an exact backlight basename",
                ));
            }
            let root = fs::canonicalize(root).map_err(|e| error("Cannot resolve sysfs root", e))?;
            let canonical = fs::canonicalize(root.join("class/backlight").join(id))
                .map_err(|e| error("Cannot resolve selected backlight", e))?;
            let relative = canonical.strip_prefix(&root).map_err(|_| {
                ControlError::new(
                    "invalid_brightness_device",
                    "Backlight symlink escapes the selected sysfs root",
                )
            })?;
            if relative.as_os_str().is_empty() {
                return Err(ControlError::new(
                    "invalid_brightness_device",
                    "Backlight cannot be the sysfs root",
                ));
            }
            let mut dir = OpenOptions::new()
                .read(true)
                .custom_flags(O_NOFOLLOW | O_DIRECTORY | O_CLOEXEC)
                .open(&root)
                .map_err(|e| error("Cannot open sysfs root", e))?;
            // Walk from a held root fd without following links. This preserves a
            // legitimate /sys/class/backlight -> /sys/devices link, while a
            // concurrent replacement of a resolved parent by a symlink fails.
            for component in relative.components() {
                let Component::Normal(name) = component else {
                    return Err(ControlError::new(
                        "invalid_brightness_device",
                        "Invalid backlight path component",
                    ));
                };
                use std::os::unix::ffi::OsStrExt;
                let name = CString::new(name.as_bytes())
                    .map_err(|e| error("Invalid backlight path", e))?;
                // SAFETY: live dir fd and NUL-terminated basename; no creation.
                let fd = unsafe {
                    openat(
                        dir.as_raw_fd(),
                        name.as_ptr(),
                        O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC,
                    )
                };
                if fd < 0 {
                    return Err(error(
                        "Cannot open selected backlight directory",
                        std::io::Error::last_os_error(),
                    ));
                }
                // SAFETY: openat returned a new owned file descriptor.
                dir = unsafe { File::from_raw_fd(fd) };
            }
            Ok(Self { dir, canonical })
        }
        fn attribute(&self, name: &str, write: bool) -> Result<File, ControlError> {
            // Only these fixed attributes are accepted; never open a caller path.
            if !matches!(name, "brightness" | "actual_brightness" | "max_brightness")
                || (write && name != "brightness")
            {
                return Err(ControlError::new(
                    "invalid_brightness_device",
                    "Invalid backlight attribute",
                ));
            }
            let name = CString::new(name).expect("fixed attribute");
            // O_TRUNC also makes regular-file fixtures behave like sysfs stores.
            let flags = O_NOFOLLOW | O_CLOEXEC | 0x800 | if write { 1 | 0x200 } else { 0 };
            // SAFETY: held directory fd and fixed NUL-terminated name.
            let fd = unsafe { openat(self.dir.as_raw_fd(), name.as_ptr(), flags) };
            if fd < 0 {
                return Err(error(
                    "Cannot open backlight attribute",
                    std::io::Error::last_os_error(),
                ));
            }
            // SAFETY: openat returned a new owned descriptor.
            let file = unsafe { File::from_raw_fd(fd) };
            if !file
                .metadata()
                .map_err(|e| error("Cannot inspect backlight attribute", e))?
                .is_file()
            {
                return Err(ControlError::new(
                    "invalid_brightness_device",
                    "Backlight attribute is not a regular sysfs file",
                ));
            }
            Ok(file)
        }
        fn read(&self, name: &str) -> Result<u32, ControlError> {
            let mut text = String::new();
            self.attribute(name, false)?
                .take(65)
                .read_to_string(&mut text)
                .map_err(|e| error("Cannot read backlight value", e))?;
            if text.len() > 64
                || text.trim().is_empty()
                || !text.trim().bytes().all(|b| b.is_ascii_digit())
            {
                return Err(ControlError::new(
                    "invalid_brightness_device",
                    format!("Invalid numeric {name}"),
                ));
            }
            text.trim()
                .parse()
                .map_err(|e| error("Invalid backlight numeric range", e))
        }
        fn writable(&self) -> bool {
            let name = c"brightness";
            // AT_EACCESS checks effective credentials and ACLs without writing.
            // The actual write still handles revoked permissions independently.
            unsafe { faccessat(self.dir.as_raw_fd(), name.as_ptr(), 2, 0x200 | 0x100) == 0 }
        }
    }
    impl BrightnessIo for Device {
        fn read_control(&mut self) -> Result<u32, ControlError> {
            self.read("brightness")
        }
        fn write_control(&mut self, value: u32) -> Result<(), WriteFailure> {
            self.attribute("brightness", true)
                .map_err(WriteFailure::NotStarted)?
                .write_all(format!("{value}\n").as_bytes())
                .map_err(|e| WriteFailure::Terminal(error("Cannot write brightness", e)))
        }
        fn permits_conditional_recovery(&self) -> bool {
            true
        }
    }
    fn unavailable(id: String, source: EvidenceSource, reason: String) -> BrightnessDevice {
        BrightnessDevice {
            label: id.clone(),
            id,
            current_percent: None,
            control_value: None,
            actual_value: None,
            maximum: None,
            source,
            status: CapabilityStatus::Unavailable,
            readable: false,
            writable: false,
            write_backend: None,
            logind_session: None,
            supported_percentages: vec![],
            reason: Some(reason),
        }
    }
    pub(super) fn devices(root: &Path) -> Vec<BrightnessDevice> {
        let entries = match fs::read_dir(root.join("class/backlight")) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return vec![],
            Err(e) => {
                return vec![unavailable(
                    "backlight".into(),
                    EvidenceSource {
                        backend: "linux_sysfs_backlight".into(),
                        path: root.join("class/backlight").display().to_string(),
                        device_id: None,
                    },
                    format!("Cannot enumerate OS backlights: {e}"),
                )]
            }
        };
        let mut result = vec![];
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let mut source = EvidenceSource {
                backend: "linux_sysfs_backlight".into(),
                path: entry.path().display().to_string(),
                device_id: Some(id.clone()),
            };
            let device = match Device::open(root, &id) {
                Ok(device) => device,
                Err(e) => {
                    result.push(unavailable(id, source, e.to_string()));
                    continue;
                }
            };
            source.path = device.canonical.display().to_string();
            let maximum = match device.read("max_brightness") {
                Ok(value) if value > 0 => value,
                Ok(_) => {
                    result.push(unavailable(id, source, "Backlight maximum is zero".into()));
                    continue;
                }
                Err(e) => {
                    result.push(unavailable(id, source, e.to_string()));
                    continue;
                }
            };
            let control = match device.read("brightness") {
                Ok(value) if value <= maximum => value,
                Ok(_) => {
                    result.push(unavailable(
                        id,
                        source,
                        "Requested brightness exceeds its advertised maximum".into(),
                    ));
                    continue;
                }
                Err(e) => {
                    result.push(unavailable(id, source, e.to_string()));
                    continue;
                }
            };
            let actual = device
                .read("actual_brightness")
                .ok()
                .filter(|value| *value <= maximum);
            let writable = device.writable();
            let delegated = if !writable && root == Path::new("/sys") {
                Some(logind::session(&id, &device.canonical))
            } else {
                None
            };
            let logind_session = delegated
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .cloned();
            let write_backend = if writable {
                Some(BrightnessBackend::LinuxSysfs)
            } else if logind_session.is_some() {
                Some(BrightnessBackend::LinuxLogind)
            } else {
                None
            };
            let route_reason = match delegated {
                Some(Ok(ref session)) => Some(format!("Direct sysfs permission is unavailable; logind session {} on {} is eligible and will be revalidated before applying", session.session_id, session.seat_id)),
                Some(Err(error)) => Some(format!("Direct sysfs permission is unavailable; logind route unavailable: {error}")),
                None if !writable => Some("OS permissions or mount state prevent this user from writing brightness".into()),
                None => None,
            };
            result.push(BrightnessDevice {
                id: id.clone(),
                label: id,
                current_percent: Some(
                    f64::from(actual.unwrap_or(control)) * 100.0 / f64::from(maximum),
                ),
                control_value: Some(control),
                actual_value: actual,
                maximum: Some(maximum),
                source,
                status: if write_backend.is_some() {
                    CapabilityStatus::Available
                } else {
                    CapabilityStatus::Unavailable
                },
                readable: true,
                writable,
                write_backend,
                logind_session,
                supported_percentages: vec![],
                reason: if route_reason.is_some() {
                    route_reason
                } else if actual.is_none() {
                    Some("actual_brightness is unavailable; the percentage is the OS requested value, not measured light output".into())
                } else {
                    None
                },
            });
        }
        result.sort_by(|a, b| a.id.cmp(&b.id));
        result
    }
    pub(super) fn apply(
        root: &Path,
        plan: &BrightnessPlan,
    ) -> Result<BrightnessApplyResult, ControlError> {
        let fresh = super::plan(root, plan.requested_percent, Some(&plan.device_id))?;
        validate_plan(plan, &fresh)?;
        let mut device = Device::open(root, &plan.device_id)?;
        if device.canonical.display().to_string() != plan.source.path
            || device.read("max_brightness")? != plan.maximum
        {
            return Err(ControlError::new(
                "brightness_plan_stale",
                "The selected backlight changed since planning",
            ));
        }
        let transaction = match plan.backend {
            BrightnessBackend::LinuxSysfs => transact(&mut device, plan)?,
            BrightnessBackend::LinuxLogind => {
                let binding = plan.logind_session.clone().ok_or_else(|| {
                    ControlError::new(
                        "invalid_brightness_plan",
                        "Logind plan has no session binding",
                    )
                })?;
                logind::revalidate(&binding, &plan.device_id, &device.canonical)?;
                let mut delegated = Delegated {
                    device,
                    id: plan.device_id.clone(),
                    binding,
                    maximum: plan.maximum,
                    previous: plan.previous,
                };
                let transaction = transact(&mut delegated, plan)?;
                device = delegated.device;
                transaction
            }
            _ => {
                return Err(ControlError::new(
                    "invalid_brightness_plan",
                    "Windows brightness plan cannot be used on Linux",
                ))
            }
        };
        Ok(BrightnessApplyResult {
            device_id: plan.device_id.clone(),
            requested_percent: plan.requested_percent,
            confirmed: transaction
                .observed_control
                .expect("Confirmed transaction has a control readback"),
            previous: plan.previous,
            actual_value: device
                .read("actual_brightness")
                .ok()
                .filter(|v| *v <= plan.maximum),
            source: plan.source.clone(),
            transaction,
        })
    }

    struct Delegated {
        device: Device,
        id: String,
        binding: LogindSessionBinding,
        maximum: u32,
        previous: u32,
    }
    impl BrightnessIo for Delegated {
        fn read_control(&mut self) -> Result<u32, ControlError> {
            self.device.read("brightness")
        }
        fn write_control(&mut self, value: u32) -> Result<(), WriteFailure> {
            let current =
                Device::open(Path::new("/sys"), &self.id).map_err(WriteFailure::NotStarted)?;
            use std::os::unix::fs::MetadataExt;
            let old =
                self.device.dir.metadata().map_err(|e| {
                    WriteFailure::NotStarted(error("Cannot inspect held backlight", e))
                })?;
            let new = current.dir.metadata().map_err(|e| {
                WriteFailure::NotStarted(error("Cannot inspect current backlight", e))
            })?;
            if old.dev() != new.dev()
                || old.ino() != new.ino()
                || current.canonical != self.device.canonical
                || current
                    .read("max_brightness")
                    .map_err(WriteFailure::NotStarted)?
                    != self.maximum
            {
                return Err(WriteFailure::NotStarted(ControlError::new(
                    "brightness_plan_stale",
                    "Backlight identity or maximum changed before the delegated write",
                )));
            }
            logind::set(
                &self.binding,
                &self.id,
                &self.device.canonical,
                self.maximum,
                self.previous,
                value,
            )
        }
    }
}

// The parser and safe script builder are tested on Linux as well as Windows.
#[cfg(any(target_os = "windows", test))]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod windows {
    use super::*;
    use std::time::Duration;
    const QUERY: &str = "$ErrorActionPreference='Stop'; $b=@(Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness -ErrorAction Stop); $m=@(Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightnessMethods -ErrorAction Stop); $rows=@($b | ForEach-Object { $n=$_.InstanceName; $methods=@($m | Where-Object { [string]::Equals([string]$_.InstanceName,[string]$n,[StringComparison]::Ordinal) -and $_.Active }); [pscustomobject]@{ id=[string]$n; current=$_.CurrentBrightness; active=[bool]$_.Active; methods=$methods.Count; levels=@($_.Level) } }); ConvertTo-Json -InputObject $rows -Depth 4 -Compress";
    #[derive(Deserialize)]
    struct Row {
        id: String,
        current: u32,
        active: bool,
        methods: usize,
        levels: Vec<u8>,
    }

    fn base64(bytes: &[u8]) -> String {
        const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut result = String::new();
        for bytes in bytes.chunks(3) {
            let a = bytes[0];
            let b = *bytes.get(1).unwrap_or(&0);
            let c = *bytes.get(2).unwrap_or(&0);
            result.push(TABLE[(a >> 2) as usize] as char);
            result.push(TABLE[(((a & 3) << 4) | (b >> 4)) as usize] as char);
            result.push(if bytes.len() > 1 {
                TABLE[(((b & 15) << 2) | (c >> 6)) as usize] as char
            } else {
                '='
            });
            result.push(if bytes.len() > 2 {
                TABLE[(c & 63) as usize] as char
            } else {
                '='
            });
        }
        result
    }
    fn parse(text: &str) -> Result<Vec<BrightnessDevice>, ControlError> {
        let rows: Vec<Row> = serde_json::from_str(text.trim().trim_start_matches('\u{feff}'))
            .map_err(|e| {
                ControlError::new(
                    "brightness_wmi_invalid",
                    format!("Invalid WMI brightness response: {e}"),
                )
            })?;
        let mut counts = std::collections::HashMap::new();
        for row in &rows {
            *counts.entry(row.id.clone()).or_insert(0usize) += 1;
        }
        Ok(rows
            .into_iter()
            .map(|row| {
                let valid = !row.id.is_empty()
                    && row.id.len() <= 4096
                    && row.current <= 100
                    && !row.levels.is_empty()
                    && row.levels.iter().all(|v| *v <= 100)
                    && row.levels.contains(&(row.current as u8))
                    && counts[&row.id] == 1;
                let readable = valid && row.active;
                let writable = readable && row.methods == 1;
                let reason = if !valid {
                    Some(
                        "WMI returned invalid or duplicate display identity/brightness levels"
                            .into(),
                    )
                } else if !row.active {
                    Some("The WMI brightness display is inactive".into())
                } else if row.methods != 1 {
                    Some("No unique active WMI brightness method matches this display".into())
                } else {
                    None
                };
                BrightnessDevice {
                    label: "Windows WMI display".into(),
                    id: row.id.clone(),
                    current_percent: readable.then_some(f64::from(row.current)),
                    control_value: readable.then_some(row.current),
                    actual_value: None,
                    maximum: valid.then_some(100),
                    source: EvidenceSource {
                        backend: "windows_wmi_brightness".into(),
                        path: "root/WMI:WmiMonitorBrightness".into(),
                        device_id: Some(row.id),
                    },
                    status: if writable {
                        CapabilityStatus::Available
                    } else {
                        CapabilityStatus::Unavailable
                    },
                    readable,
                    writable,
                    write_backend: writable.then_some(BrightnessBackend::WindowsWmi),
                    logind_session: None,
                    supported_percentages: row.levels,
                    reason,
                }
            })
            .collect())
    }
    fn run(script: &str) -> Result<String, ControlError> {
        crate::command::run_command(
            "powershell.exe",
            &[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ],
            Duration::from_secs(8),
            128 * 1024,
        )
        .map(|output| output.stdout)
    }
    pub(super) fn devices() -> Vec<BrightnessDevice> {
        match run(QUERY).and_then(|text| parse(&text)) {
            Ok(devices) => devices,
            Err(error) => vec![BrightnessDevice {
                id: "wmi-unavailable".into(),
                label: "Windows WMI brightness".into(),
                current_percent: None,
                control_value: None,
                actual_value: None,
                maximum: None,
                source: EvidenceSource {
                    backend: "windows_wmi_brightness".into(),
                    path: "root/WMI:WmiMonitorBrightness".into(),
                    device_id: None,
                },
                status: CapabilityStatus::Unavailable,
                readable: false,
                writable: false,
                write_backend: None,
                logind_session: None,
                supported_percentages: vec![],
                reason: Some(format!(
                    "Windows did not expose its WMI brightness provider: {error}"
                )),
            }],
        }
    }
    fn set_script(id: &str, value: u32) -> Result<String, ControlError> {
        if id.is_empty() || id.len() > 4096 || value > 100 {
            return Err(ControlError::new(
                "invalid_brightness_plan",
                "Invalid WMI display identity or brightness",
            ));
        }
        let json = serde_json::to_vec(&serde_json::json!({"id": id,"value":value}))
            .map_err(|e| ControlError::new("invalid_brightness_plan", e.to_string()))?;
        // The only inserted text is base64 data, never a device identifier,
        // WQL expression, PowerShell token or interpolated string from the user.
        Ok(format!("$ErrorActionPreference='Stop'; $r=([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}')) | ConvertFrom-Json); $m=@(Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightnessMethods -ErrorAction Stop | Where-Object {{ [string]::Equals([string]$_.InstanceName,[string]$r.id,[StringComparison]::Ordinal) -and $_.Active }}); if ($m.Count -ne 1) {{ throw 'No unique active brightness method for selected display' }}; $result=Invoke-CimMethod -InputObject $m[0] -MethodName WmiSetBrightness -Arguments @{{Timeout=[uint32]0;Brightness=[byte]$r.value}} -ErrorAction Stop; if ($null -eq $result -or $null -eq $result.ReturnValue) {{ throw 'WmiSetBrightness returned no status' }}; if ([uint32]$result.ReturnValue -ne 0) {{ throw ('WmiSetBrightness failed: '+$result.ReturnValue) }}; [pscustomobject]@{{return_value=$result.ReturnValue}} | ConvertTo-Json -Compress", base64(&json)))
    }
    struct Wmi {
        id: String,
    }
    fn parse_write_status(text: &str) -> Result<(), ControlError> {
        let response: serde_json::Value =
            serde_json::from_str(text.trim().trim_start_matches('\u{feff}')).map_err(|e| {
                ControlError::new(
                    "brightness_wmi_invalid",
                    format!("Invalid WmiSetBrightness response: {e}"),
                )
            })?;
        if response["return_value"].as_u64() != Some(0) {
            return Err(ControlError::new(
                "brightness_write_failed",
                "WmiSetBrightness did not report success",
            ));
        }
        Ok(())
    }
    impl BrightnessIo for Wmi {
        fn read_control(&mut self) -> Result<u32, ControlError> {
            let matches: Vec<_> = devices().into_iter().filter(|d| d.id == self.id).collect();
            if matches.len() != 1 {
                return Err(unsupported());
            }
            matches[0].control_value.ok_or_else(unsupported)
        }
        fn write_control(&mut self, value: u32) -> Result<(), WriteFailure> {
            let script = set_script(&self.id, value).map_err(WriteFailure::NotStarted)?;
            let output = crate::command::run_control_helper(
                "powershell.exe",
                &[
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    &script,
                ],
                128 * 1024,
            )
            .map_err(|error| {
                if error.code == "command_unavailable" {
                    WriteFailure::NotStarted(error)
                } else {
                    WriteFailure::Unknown(error)
                }
            })?;
            if output.status != 0 {
                let mut error = ControlError::new(
                    "brightness_wmi_write_unknown",
                    format!(
                        "PowerShell exited with status {}; a WMI provider write may remain",
                        output.status
                    ),
                );
                error.details.push(format!("stdout: {}", output.stdout));
                error.details.push(format!("stderr: {}", output.stderr));
                return Err(WriteFailure::Unknown(error));
            }
            parse_write_status(&output.stdout).map_err(WriteFailure::Unknown)
        }
    }
    pub(super) fn apply(plan: &BrightnessPlan) -> Result<BrightnessApplyResult, ControlError> {
        let fresh = make_plan(
            select(devices(), Some(&plan.device_id))?,
            plan.requested_percent,
            BrightnessBackend::WindowsWmi,
        )?;
        validate_plan(plan, &fresh)?;
        let transaction = transact(
            &mut Wmi {
                id: plan.device_id.clone(),
            },
            plan,
        )?;
        Ok(BrightnessApplyResult {
            device_id: plan.device_id.clone(),
            requested_percent: plan.requested_percent,
            confirmed: transaction
                .observed_control
                .expect("Confirmed transaction has a control readback"),
            previous: plan.previous,
            actual_value: None,
            source: plan.source.clone(),
            transaction,
        })
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn missing_or_null_wmi_values_never_become_zero() {
            for text in [
                r#"[{"id":"a","current":null,"active":true,"methods":1,"levels":[0]}]"#,
                r#"[{"id":"a","current":0,"active":true,"methods":1,"levels":[null]}]"#,
            ] {
                assert!(parse(text).is_err());
            }
            let unsupported =
                parse(r#"[{"id":"a","current":51,"active":true,"methods":1,"levels":[0,50,100]}]"#)
                    .unwrap();
            assert!(!unsupported[0].writable);
            assert!(parse_write_status(r#"{"return_value":0}"#).is_ok());
            for text in [
                "{}",
                "null",
                r#"{"return_value":null}"#,
                r#"{"return_value":"0"}"#,
                r#"{"return_value":1}"#,
            ] {
                assert!(parse_write_status(text).is_err());
            }
            assert!(!QUERY.contains("current=[int]"));
            assert!(set_script("display", 50)
                .unwrap()
                .contains("$null -eq $result.ReturnValue"));
        }
        #[test]
        fn parses_empty_and_exact_multi_display_rows() {
            assert!(parse("[]").unwrap().is_empty());
            assert!(parse("null").is_err());
            let d=parse(r#"[{"id":"DISPLAY\\A","current":50,"active":true,"methods":1,"levels":[0,50,100]},{"id":"DISPLAY\\a","current":20,"active":true,"methods":0,"levels":[20]}]"#).unwrap();
            assert_eq!(d.len(), 2);
            assert!(d[0].writable);
            assert!(!d[1].writable);
            assert_eq!(
                make_plan(d[0].clone(), 51, BrightnessBackend::WindowsWmi)
                    .unwrap_err()
                    .code,
                "brightness_level_unsupported"
            );
        }
        #[test]
        fn duplicate_wmi_id_is_never_writable() {
            let d=parse(r#"[{"id":"a","current":50,"active":true,"methods":1,"levels":[50]},{"id":"a","current":50,"active":true,"methods":1,"levels":[50]}]"#).unwrap();
            assert!(d.iter().all(|d| !d.writable));
        }
        #[test]
        fn hostile_identifier_is_data_not_script() {
            let script = set_script("a');throw 'injected';#", 50).unwrap();
            assert!(!script.contains("injected"));
            assert!(!script.contains("-Filter"));
            assert!(script.contains("StringComparison]::Ordinal"));
            assert_eq!(base64(b"f"), "Zg==");
            assert_eq!(base64(b"fo"), "Zm8=");
            assert_eq!(base64(b"foo"), "Zm9v");
        }
    }
}

#[cfg(test)]
// @mark mechrevo.display_brightness role=verification
mod tests {
    use super::*;
    #[test]
    fn percent_boundaries_and_large_raw_values() {
        assert_eq!(target(0, u32::MAX).unwrap(), 0);
        assert_eq!(target(100, u32::MAX).unwrap(), u32::MAX);
        assert_eq!(target(50, 3).unwrap(), 2);
        assert!(target(101, 100).is_err());
        assert!(target(255, 100).is_err());
        assert!(target(50, 0).is_err());
    }
    use std::collections::VecDeque;
    enum Step {
        Ack,
        NotStarted,
        Terminal(bool),
        Unknown(bool),
    }
    struct Fake {
        value: u32,
        writes: Vec<u32>,
        steps: VecDeque<Step>,
        reads: VecDeque<Result<u32, ControlError>>,
        local: bool,
    }
    fn detail_error(code: &str) -> ControlError {
        let mut error = ControlError::new(code, format!("original {code} message"));
        error.details.push(format!("original {code} detail"));
        error
    }
    impl Fake {
        fn new(steps: impl IntoIterator<Item = Step>) -> Self {
            Self {
                value: 20,
                writes: vec![],
                steps: steps.into_iter().collect(),
                reads: VecDeque::new(),
                local: true,
            }
        }
        fn observed(mut self, reads: impl IntoIterator<Item = Result<u32, ControlError>>) -> Self {
            self.reads = reads.into_iter().collect();
            self
        }
    }
    impl BrightnessIo for Fake {
        fn read_control(&mut self) -> Result<u32, ControlError> {
            let result = self.reads.pop_front().unwrap_or(Ok(self.value));
            if let Ok(value) = result {
                self.value = value;
            }
            result
        }
        fn write_control(&mut self, value: u32) -> Result<(), WriteFailure> {
            let step = self.steps.pop_front().unwrap_or(Step::Ack);
            if matches!(step, Step::NotStarted) {
                return Err(WriteFailure::NotStarted(detail_error("not_started")));
            }
            self.writes.push(value);
            match step {
                Step::Ack => {
                    self.value = value;
                    Ok(())
                }
                Step::Terminal(landed) => {
                    if landed {
                        self.value = value;
                    }
                    Err(WriteFailure::Terminal(detail_error(
                        if self.writes.len() > 1 {
                            "rollback_error"
                        } else {
                            "primary_error"
                        },
                    )))
                }
                Step::Unknown(landed) => {
                    if landed {
                        self.value = value;
                    }
                    Err(WriteFailure::Unknown(detail_error("transport_lost")))
                }
                Step::NotStarted => unreachable!(),
            }
        }
        fn permits_conditional_recovery(&self) -> bool {
            self.local
        }
    }
    fn fixture_plan() -> BrightnessPlan {
        BrightnessPlan {
            device_id: "test".into(),
            requested_percent: 80,
            target: 80,
            previous: 20,
            maximum: 100,
            source: EvidenceSource {
                backend: "test".into(),
                path: "test".into(),
                device_id: Some("test".into()),
            },
            backend: BrightnessBackend::LinuxSysfs,
            logind_session: None,
        }
    }
    fn failed(io: &mut Fake) -> BrightnessTransaction {
        transaction_from_error(&transact(io, &fixture_plan()).unwrap_err()).unwrap()
    }
    #[test]
    fn success_and_already_current_confirm_control_without_unneeded_setter() {
        let mut io = Fake::new([Step::Ack]);
        let result = transact(&mut io, &fixture_plan()).unwrap();
        assert_eq!(result.outcome, BrightnessOutcome::Confirmed);
        assert_eq!(io.writes, vec![80]);
        assert!(!result.blocks_further_writes());
        let mut p = fixture_plan();
        p.target = 20;
        let result = transact(&mut Fake::new([]), &p).unwrap();
        assert!(!result.write_attempted);
    }
    #[test]
    fn stale_value_is_rejected_before_any_write() {
        let mut io = Fake::new([]).observed([Ok(21)]);
        let error = transact(&mut io, &fixture_plan()).unwrap_err();
        assert_eq!(error.code, "brightness_plan_stale");
        assert!(io.writes.is_empty());
        assert_eq!(
            transaction_from_error(&error).unwrap().outcome,
            BrightnessOutcome::NotApplied
        );
    }
    #[test]
    fn not_started_has_zero_writes_and_preserves_primary_error() {
        let mut io = Fake::new([Step::NotStarted]);
        let tx = failed(&mut io);
        assert!(io.writes.is_empty());
        assert!(!tx.write_attempted);
        assert_eq!(tx.outcome, BrightnessOutcome::NotApplied);
        assert_eq!(
            tx.primary_error.unwrap().details,
            vec!["original not_started detail"]
        );
    }
    #[test]
    fn delegated_write_lands_then_lost_reply_is_unknown_with_zero_rollback() {
        let mut io = Fake::new([Step::Unknown(true)]);
        let tx = failed(&mut io);
        assert_eq!(io.value, 80);
        assert_eq!(io.writes, vec![80]);
        assert_eq!(tx.outcome, BrightnessOutcome::Unknown);
        assert!(tx.writer_may_continue && tx.blocks_further_writes());
        assert_eq!(tx.recovery, BrightnessRecovery::Skipped);
        assert!(tx.rollback_error.is_none());
    }
    #[test]
    fn lost_reply_and_old_cached_value_never_claims_not_applied_or_restored() {
        let mut io = Fake::new([Step::Unknown(false)]);
        let tx = failed(&mut io);
        assert_eq!(tx.observed_control, Some(20));
        assert_eq!(tx.outcome, BrightnessOutcome::Unknown);
        assert!(tx.writer_may_continue);
        assert_eq!(io.writes, vec![80]);
    }
    #[test]
    fn third_party_change_not_overwritten() {
        let mut io = Fake::new([Step::Ack]).observed([Ok(20), Ok(77)]);
        let tx = failed(&mut io);
        assert_eq!(io.value, 77);
        assert_eq!(io.writes, vec![80]);
        assert_eq!(tx.outcome, BrightnessOutcome::Conflict);
        assert_eq!(tx.recovery, BrightnessRecovery::Skipped);
    }
    #[test]
    fn third_party_change_during_recovery_guard_not_overwritten() {
        let mut io = Fake::new([Step::Terminal(true)]).observed([Ok(20), Ok(80), Ok(79)]);
        let tx = failed(&mut io);
        assert_eq!(io.value, 79);
        assert_eq!(io.writes, vec![80]);
        assert_eq!(tx.outcome, BrightnessOutcome::Conflict);
    }
    #[test]
    fn local_terminal_failure_restores_only_observed_own_target() {
        let mut io = Fake::new([Step::Terminal(true), Step::Ack]);
        let tx = failed(&mut io);
        assert_eq!(io.writes, vec![80, 20]);
        assert_eq!(io.value, 20);
        assert_eq!(tx.outcome, BrightnessOutcome::Restored);
        assert_eq!(tx.recovery, BrightnessRecovery::Restored);
        assert_eq!(
            tx.primary_error.unwrap().details,
            vec!["original primary_error detail"]
        );
    }
    #[test]
    fn failed_rollback_old_cached_value_never_confirmed() {
        let mut io = Fake::new([Step::Terminal(true), Step::Terminal(true)]);
        let tx = failed(&mut io);
        assert_eq!(tx.observed_control, Some(20));
        assert_eq!(tx.outcome, BrightnessOutcome::RecoveryFailed);
        assert_ne!(tx.recovery, BrightnessRecovery::Restored);
        assert_eq!(
            tx.primary_error.as_ref().unwrap().details,
            vec!["original primary_error detail"]
        );
        assert_eq!(
            tx.rollback_error.as_ref().unwrap().details,
            vec!["original rollback_error detail"]
        );
        assert!(tx.blocks_further_writes());
    }
    #[test]
    fn recovery_lost_reply_keeps_writer_possible_even_with_old_readback() {
        let mut io = Fake::new([Step::Terminal(true), Step::Unknown(true)]);
        let tx = failed(&mut io);
        assert_eq!(tx.observed_control, Some(20));
        assert_eq!(tx.outcome, BrightnessOutcome::RecoveryFailed);
        assert_eq!(tx.recovery, BrightnessRecovery::Unknown);
        assert!(tx.writer_may_continue && tx.blocks_further_writes());
        assert!(tx.rollback_error.is_some());
    }
    #[test]
    fn local_failed_write_with_old_value_remains_unknown_and_does_not_rewrite_it() {
        let mut io = Fake::new([Step::Terminal(false)]);
        let tx = failed(&mut io);
        assert_eq!(io.writes, vec![80]);
        assert_eq!(tx.outcome, BrightnessOutcome::Unknown);
        assert_eq!(tx.recovery, BrightnessRecovery::Skipped);
    }
    #[test]
    fn unknown_readback_never_authorizes_a_recovery_write() {
        let mut io = Fake::new([Step::Ack]).observed([
            Ok(20),
            Err(detail_error("first_read")),
            Err(detail_error("second_read")),
        ]);
        let tx = failed(&mut io);
        assert_eq!(io.writes, vec![80]);
        assert_eq!(tx.outcome, BrightnessOutcome::Unknown);
        assert!(tx.blocks_further_writes());
        let read = tx.readback_error.unwrap();
        assert_eq!(read.code, "first_read");
        assert!(read
            .details
            .iter()
            .any(|d| d.contains("second_read") && d.contains("original second_read detail")));
    }
    #[test]
    fn failed_restoration_read_preserves_primary_and_readback_errors() {
        let mut io = Fake::new([Step::Terminal(true), Step::Ack]).observed([
            Ok(20),
            Ok(80),
            Ok(80),
            Err(detail_error("recovery_read")),
        ]);
        let tx = failed(&mut io);
        assert_eq!(tx.outcome, BrightnessOutcome::RecoveryFailed);
        assert_eq!(tx.recovery, BrightnessRecovery::Unknown);
        assert_eq!(
            tx.readback_error.unwrap().details,
            vec!["original recovery_read detail"]
        );
        assert!(tx.primary_error.is_some());
    }
    #[test]
    fn remote_ack_with_failed_read_never_uses_local_rollback_policy() {
        let mut io = Fake::new([Step::Ack]).observed([Ok(20), Err(detail_error("read"))]);
        io.local = false;
        let tx = failed(&mut io);
        assert_eq!(io.writes, vec![80]);
        assert_eq!(tx.outcome, BrightnessOutcome::Unknown);
        assert!(!tx.writer_may_continue);
    }
    #[cfg(target_os = "linux")]
    mod linux_fixtures {
        use super::*;
        use std::{
            fs,
            path::PathBuf,
            sync::atomic::{AtomicUsize, Ordering},
        };
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        fn root() -> PathBuf {
            let base = std::env::var_os("TMPDIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("../../.local-tests/display-brightness")
                });
            let path = base.join(format!(
                "brightness-fixture-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("class/backlight")).unwrap();
            path
        }
        fn add(root: &Path, id: &str, max: &str, control: &str, actual: Option<&str>) {
            let p = root.join("class/backlight").join(id);
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("max_brightness"), max).unwrap();
            fs::write(p.join("brightness"), control).unwrap();
            if let Some(actual) = actual {
                fs::write(p.join("actual_brightness"), actual).unwrap();
            }
        }
        #[test]
        fn dry_run_and_success_separate_request_from_actual() {
            let r = root();
            add(&r, "intel_backlight", "1000", "250", Some("200"));
            let p = plan(&r, 70, None).unwrap();
            assert_eq!(p.target, 700);
            assert_eq!(
                fs::read_to_string(r.join("class/backlight/intel_backlight/brightness")).unwrap(),
                "250"
            );
            let result = apply(&r, &p).unwrap();
            assert_eq!(result.confirmed, 700);
            assert_eq!(result.actual_value, Some(200));
            assert_eq!(devices(&r)[0].current_percent, Some(20.));
        }
        #[test]
        fn multi_display_requires_selection_and_writes_one() {
            let r = root();
            add(&r, "one", "100", "20", None);
            add(&r, "two", "100", "30", None);
            assert_eq!(
                plan(&r, 50, None).unwrap_err().code,
                "brightness_device_ambiguous"
            );
            apply(&r, &plan(&r, 80, Some("two")).unwrap()).unwrap();
            assert_eq!(
                fs::read_to_string(r.join("class/backlight/one/brightness")).unwrap(),
                "20"
            );
            assert_eq!(
                devices(&r)
                    .iter()
                    .find(|d| d.id == "two")
                    .unwrap()
                    .control_value,
                Some(80)
            );
        }
        #[test]
        fn absent_invalid_and_stale_devices_cannot_write() {
            let r = root();
            assert_eq!(
                plan(&r, 20, None).unwrap_err().code,
                "brightness_unavailable"
            );
            assert_eq!(plan(&r, 101, None).unwrap_err().code, "invalid_brightness");
            add(&r, "zero", "0", "0", None);
            assert!(!devices(&r)[0].writable);
            assert!(plan(&r, 50, Some("../zero")).is_err());
            add(&r, "good", "100", "20", None);
            let mut p = plan(&r, 50, Some("good")).unwrap();
            p.target = 99;
            assert_eq!(apply(&r, &p).unwrap_err().code, "invalid_brightness_plan");
            p.target = 50;
            fs::write(r.join("class/backlight/good/brightness"), "21").unwrap();
            assert_eq!(apply(&r, &p).unwrap_err().code, "brightness_plan_stale");
        }
        #[test]
        fn class_symlinks_work_but_device_and_attribute_escapes_fail() {
            use std::os::unix::fs::symlink;
            let r = root();
            let outside = root();
            add(&outside, "external", "100", "20", None);
            symlink(
                outside.join("class/backlight/external"),
                r.join("class/backlight/escape"),
            )
            .unwrap();
            assert!(!devices(&r)[0].writable);
            fs::create_dir_all(r.join("devices/panel")).unwrap();
            fs::write(r.join("devices/panel/max_brightness"), "100").unwrap();
            fs::write(r.join("devices/panel/brightness"), "20").unwrap();
            symlink(r.join("devices/panel"), r.join("class/backlight/panel")).unwrap();
            assert!(plan(&r, 50, Some("panel")).is_ok());
            fs::remove_file(r.join("devices/panel/brightness")).unwrap();
            symlink(
                outside.join("class/backlight/external/brightness"),
                r.join("devices/panel/brightness"),
            )
            .unwrap();
            assert!(plan(&r, 50, Some("panel")).is_err());
            assert_eq!(
                fs::read_to_string(outside.join("class/backlight/external/brightness")).unwrap(),
                "20"
            );
        }
    }
}
