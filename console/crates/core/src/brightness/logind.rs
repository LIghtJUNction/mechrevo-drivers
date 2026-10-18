//! Unprivileged display brightness through the system logind authority.
//!
//! The selected session is the calling user's primary display, not an
//! environment-provided session ID. logind repeats its own authorization at
//! SetBrightness. An unacknowledged call may leave a writer running in logind;
//! reaping busctl alone cannot make rollback safe.

use super::{LogindSessionBinding as Binding, WriteFailure};
use crate::command::CommandOutput;
use crate::ControlError;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

const BUSCTL: &str = "/usr/bin/busctl";
const UDEVADM: &str = "/usr/bin/udevadm";
const SERVICE: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
const USER: &str = "org.freedesktop.login1.User";
const SEAT: &str = "org.freedesktop.login1.Seat";
const SESSION_PREFIX: &str = "/org/freedesktop/login1/session/";
const SEAT_PREFIX: &str = "/org/freedesktop/login1/seat/";
const OUTPUT_LIMIT: usize = 64 * 1024;
const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

// --system alone still observes DBUS_SYSTEM_BUS_ADDRESS. The explicit address
// and absolute executables prevent environment-selected buses/helpers.
const BUS_ARGS: &[&str] = &[
    "--system",
    "--address=unix:path=/run/dbus/system_bus_socket",
    "--json=short",
    "--no-pager",
    "--auto-start=no",
    "--allow-interactive-authorization=no",
];

trait Runtime {
    fn effective_uid(&self) -> u32;
    fn canonicalize(&mut self, path: &Path) -> Result<PathBuf, ControlError>;
    fn is_sysfs(&mut self, path: &Path) -> Result<bool, ControlError>;
    fn read_number(&mut self, path: &Path) -> Result<u32, ControlError>;
    fn query(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, ControlError>;
    fn control(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, ControlError>;
}

struct System;

impl Runtime for System {
    fn effective_uid(&self) -> u32 {
        // SAFETY: geteuid takes no arguments or pointers and does not mutate
        // process identity. Environment UID/SUDO_UID values are not used.
        unsafe { libc::geteuid() }
    }

    fn canonicalize(&mut self, path: &Path) -> Result<PathBuf, ControlError> {
        std::fs::canonicalize(path).map_err(|error| {
            ControlError::new(
                "invalid_brightness_device",
                format!("Cannot resolve the selected system backlight: {error}"),
            )
        })
    }

    fn is_sysfs(&mut self, path: &Path) -> Result<bool, ControlError> {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|_| {
            ControlError::new("invalid_brightness_device", "Invalid system backlight path")
        })?;
        let mut stat = std::mem::MaybeUninit::<libc::statfs>::uninit();
        // SAFETY: path is NUL terminated and stat points to writable storage for
        // one statfs. We inspect the structure only after a successful call.
        if unsafe { libc::statfs(path.as_ptr(), stat.as_mut_ptr()) } != 0 {
            return Err(ControlError::new(
                "invalid_brightness_device",
                format!(
                    "Cannot verify the system backlight filesystem: {}",
                    std::io::Error::last_os_error()
                ),
            ));
        }
        // SAFETY: successful statfs initialized the whole output structure.
        Ok(unsafe { stat.assume_init() }.f_type == 0x6265_6572)
    }

    fn query(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, ControlError> {
        crate::command::run_command(program, args, QUERY_TIMEOUT, OUTPUT_LIMIT)
    }

    fn read_number(&mut self, path: &Path) -> Result<u32, ControlError> {
        use std::io::Read;
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|error| {
                ControlError::new(
                    "brightness_read_failed",
                    format!("Cannot recheck the selected backlight: {error}"),
                )
            })?;
        let mut bytes = Vec::new();
        file.take(33).read_to_end(&mut bytes).map_err(|error| {
            ControlError::new(
                "brightness_read_failed",
                format!("Cannot recheck the selected backlight: {error}"),
            )
        })?;
        parse_number(&bytes)
    }

    fn control(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, ControlError> {
        crate::command::run_control_helper(program, args, OUTPUT_LIMIT)
    }
}

fn invalid_reply() -> ControlError {
    ControlError::new(
        "brightness_logind_invalid_reply",
        "logind did not return the expected typed session information",
    )
}

fn unavailable(message: &str) -> ControlError {
    ControlError::new("brightness_logind_session_unavailable", message)
}

fn parse_number(bytes: &[u8]) -> Result<u32, ControlError> {
    let invalid = || {
        ControlError::new(
            "brightness_read_failed",
            "The selected backlight did not return a bounded unsigned value",
        )
    };
    if bytes.len() > 32 {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| invalid())?.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    text.parse().map_err(|_| invalid())
}

fn name_valid(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.starts_with('-')
        && name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':' | b'+')
        })
}

fn object_valid(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix).is_some_and(|label| {
        !label.is_empty()
            && label.len() <= 1024
            && label != "self"
            && label != "auto"
            && label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    })
}

fn validate_device(
    runtime: &mut impl Runtime,
    device_id: &str,
    canonical: &Path,
) -> Result<(), ControlError> {
    let invalid = || {
        ControlError::new(
            "invalid_brightness_device",
            "logind brightness requires the selected canonical /sys backlight device",
        )
    };
    if !name_valid(device_id)
        || !canonical.starts_with("/sys/devices")
        || canonical.file_name().and_then(|name| name.to_str()) != Some(device_id)
    {
        return Err(invalid());
    }
    let resolved = runtime.canonicalize(canonical)?;
    let class_path = Path::new("/sys/class/backlight").join(device_id);
    let class_resolved = runtime.canonicalize(&class_path)?;
    // Compare bytes, not normalized Path components: the supplied path must
    // already be canonical, with no parent traversal or redundant separators.
    if resolved.as_os_str() != canonical.as_os_str()
        || class_resolved.as_os_str() != canonical.as_os_str()
        || !runtime.is_sysfs(canonical)?
    {
        return Err(invalid());
    }
    Ok(())
}

fn query(
    runtime: &mut impl Runtime,
    program: &str,
    args: &[&str],
) -> Result<CommandOutput, ControlError> {
    let output = runtime.query(program, args)?;
    if output.status != 0 {
        return Err(ControlError::new(
            "brightness_logind_unavailable",
            "Cannot query the system brightness authority",
        ));
    }
    Ok(output)
}

fn bus_json(runtime: &mut impl Runtime, tail: &[&str]) -> Result<Value, ControlError> {
    let mut args = BUS_ARGS.to_vec();
    args.extend_from_slice(tail);
    let output = query(runtime, BUSCTL, &args)?;
    serde_json::from_str(&output.stdout).map_err(|_| invalid_reply())
}

fn typed<'a>(value: &'a Value, signature: &str) -> Result<&'a Value, ControlError> {
    if value.get("type").and_then(Value::as_str) != Some(signature) {
        return Err(invalid_reply());
    }
    value.get("data").ok_or_else(invalid_reply)
}

fn tuple(value: &Value, length: usize) -> Result<&[Value], ControlError> {
    let values = value.as_array().ok_or_else(invalid_reply)?;
    if values.len() != length {
        return Err(invalid_reply());
    }
    Ok(values)
}

fn string(value: &Value) -> Result<&str, ControlError> {
    value.as_str().ok_or_else(invalid_reply)
}

fn pair(value: &Value) -> Result<(&str, &str), ControlError> {
    let values = tuple(value, 2)?;
    Ok((string(&values[0])?, string(&values[1])?))
}

fn properties(
    runtime: &mut impl Runtime,
    path: &str,
    interface: &str,
) -> Result<Map<String, Value>, ControlError> {
    let reply = bus_json(
        runtime,
        &[
            "call",
            SERVICE,
            path,
            "org.freedesktop.DBus.Properties",
            "GetAll",
            "s",
            interface,
        ],
    )?;
    let values = tuple(typed(&reply, "a{sv}")?, 1)?;
    values[0].as_object().cloned().ok_or_else(invalid_reply)
}

fn property<'a>(
    properties: &'a Map<String, Value>,
    name: &str,
    signature: &str,
) -> Result<&'a Value, ControlError> {
    typed(properties.get(name).ok_or_else(invalid_reply)?, signature)
}

fn device_seat(runtime: &mut impl Runtime, canonical: &Path) -> Result<String, ControlError> {
    let path = canonical.to_str().ok_or_else(invalid_reply)?;
    let output = query(
        runtime,
        UDEVADM,
        &[
            "info",
            "--query=property",
            "--property=ID_SEAT",
            "--path",
            path,
            "--no-pager",
        ],
    )?;
    let mut seat = None;
    for line in output.stdout.lines().filter(|line| !line.is_empty()) {
        let value = line.strip_prefix("ID_SEAT=").ok_or_else(invalid_reply)?;
        if seat.is_some() || (!value.is_empty() && !name_valid(value)) {
            return Err(invalid_reply());
        }
        seat = Some(value);
    }
    // Matches systemd's device_get_seat(): absent AND empty ID_SEAT mean seat0.
    Ok(seat
        .filter(|value| !value.is_empty())
        .unwrap_or("seat0")
        .into())
}

// @mark mechrevo.display_brightness role=authority
// Protocol and completion semantics:
// https://github.com/systemd/systemd/blob/v262/src/login/logind-session-dbus.c
// https://github.com/systemd/systemd/blob/v262/src/login/logind-brightness.c
fn session_with(
    runtime: &mut impl Runtime,
    device_id: &str,
    canonical: &Path,
) -> Result<Binding, ControlError> {
    validate_device(runtime, device_id, canonical)?;
    let uid = runtime.effective_uid();
    let uid_string = uid.to_string();
    let expected_user = format!("/org/freedesktop/login1/user/_{uid}");
    let user = bus_json(
        runtime,
        &[
            "call",
            SERVICE,
            MANAGER_PATH,
            MANAGER,
            "GetUser",
            "u",
            &uid_string,
        ],
    )?;
    let user_data = tuple(typed(&user, "o")?, 1)?;
    let user_path = string(&user_data[0])?;
    if user_path != expected_user {
        return Err(invalid_reply());
    }
    let display = bus_json(
        runtime,
        &["get-property", SERVICE, user_path, USER, "Display"],
    )?;
    let (session_id, session_path) = pair(typed(&display, "(so)")?)?;
    if !name_valid(session_id) || !object_valid(session_path, SESSION_PREFIX) {
        return Err(unavailable(
            "The current user has no primary local display session",
        ));
    }
    let session = properties(runtime, session_path, SESSION)?;
    let fresh_id = string(property(&session, "Id", "s")?)?;
    let owner = tuple(property(&session, "User", "(uo)")?, 2)?;
    let owner_uid = owner[0]
        .as_u64()
        .and_then(|uid| u32::try_from(uid).ok())
        .ok_or_else(invalid_reply)?;
    if owner_uid != uid || string(&owner[1])? != user_path || fresh_id != session_id {
        return Err(unavailable(
            "The display session does not belong to the current user",
        ));
    }
    if property(&session, "Active", "b")?.as_bool() != Some(true)
        || property(&session, "Remote", "b")?.as_bool() != Some(false)
        || string(property(&session, "State", "s")?)? != "active"
        || !matches!(
            string(property(&session, "Type", "s")?)?,
            "x11" | "wayland" | "tty"
        )
    {
        return Err(unavailable(
            "Brightness control requires an active local display session",
        ));
    }
    let (seat_id, seat_path) = pair(property(&session, "Seat", "(so)")?)?;
    if !name_valid(seat_id) || !object_valid(seat_path, SEAT_PREFIX) {
        return Err(unavailable("The display session has no local seat"));
    }
    let seat = properties(runtime, seat_path, SEAT)?;
    let (active_id, active_path) = pair(property(&seat, "ActiveSession", "(so)")?)?;
    if string(property(&seat, "Id", "s")?)? != seat_id
        || active_id != session_id
        || active_path != session_path
    {
        return Err(unavailable(
            "The selected display session is no longer in the foreground",
        ));
    }
    if device_seat(runtime, canonical)? != seat_id {
        return Err(ControlError::new(
            "brightness_logind_device_mismatch",
            "The selected backlight does not belong to the display session's seat",
        ));
    }
    Ok(Binding {
        session_id: session_id.into(),
        object_path: session_path.into(),
        uid,
        seat_id: seat_id.into(),
        seat_path: seat_path.into(),
    })
}

pub(super) fn session(device_id: &str, canonical: &Path) -> Result<Binding, ControlError> {
    session_with(&mut System, device_id, canonical)
}

fn revalidate_with(
    runtime: &mut impl Runtime,
    binding: &Binding,
    device_id: &str,
    canonical: &Path,
) -> Result<(), ControlError> {
    if session_with(runtime, device_id, canonical)? != *binding {
        return Err(ControlError::new(
            "brightness_logind_session_changed",
            "The display session changed since the brightness plan was created",
        ));
    }
    Ok(())
}

pub(super) fn revalidate(
    binding: &Binding,
    device_id: &str,
    canonical: &Path,
) -> Result<(), ControlError> {
    revalidate_with(&mut System, binding, device_id, canonical)
}

fn unknown(mut cause: ControlError) -> WriteFailure {
    cause.details.push(
        "The logind brightness writer may still be pending; do not rollback or retry control writes".into(),
    );
    WriteFailure::Unknown(cause)
}

fn set_with(
    runtime: &mut impl Runtime,
    binding: &Binding,
    device_id: &str,
    canonical: &Path,
    expected_maximum: u32,
    expected_previous: u32,
    value: u32,
) -> Result<(), WriteFailure> {
    revalidate_with(runtime, binding, device_id, canonical).map_err(WriteFailure::NotStarted)?;
    let maximum = runtime
        .read_number(&canonical.join("max_brightness"))
        .map_err(WriteFailure::NotStarted)?;
    let previous = runtime
        .read_number(&canonical.join("brightness"))
        .map_err(WriteFailure::NotStarted)?;
    if maximum == 0 || maximum != expected_maximum || value > maximum || previous > maximum {
        return Err(WriteFailure::NotStarted(ControlError::new(
            "invalid_brightness_plan",
            "The selected backlight's maximum or target changed before the write",
        )));
    }
    if previous != expected_previous {
        return Err(WriteFailure::NotStarted(ControlError::new(
            "brightness_plan_stale",
            "Brightness changed during session validation; create a new plan",
        )));
    }
    let value = value.to_string();
    let mut args = BUS_ARGS.to_vec();
    args.extend_from_slice(&[
        "--expect-reply=yes",
        "--timeout=infinity",
        "call",
        SERVICE,
        &binding.object_path,
        SESSION,
        "SetBrightness",
        "ssu",
        "backlight",
        device_id,
        &value,
    ]);
    // The caller keeps its transaction blocked while this waits. Success is an
    // acknowledged void method reply, not merely a reaped helper or readback.
    match runtime.control(BUSCTL, &args) {
        Ok(output) if output.status == 0 && output.stdout.trim().is_empty() => Ok(()),
        Ok(output) => {
            let mut error = ControlError::new(
                "brightness_logind_no_acknowledgment",
                "The brightness authority did not acknowledge completion",
            );
            error
                .details
                .push(format!("helper_status: {}", output.status));
            error.details.push(format!("stdout: {}", output.stdout));
            error.details.push(format!("stderr: {}", output.stderr));
            Err(unknown(error))
        }
        Err(error) if error.code == "command_unavailable" => Err(WriteFailure::NotStarted(error)),
        Err(error) => Err(unknown(error)),
    }
}

pub(super) fn set(
    binding: &Binding,
    device_id: &str,
    canonical: &Path,
    expected_maximum: u32,
    expected_previous: u32,
    value: u32,
) -> Result<(), WriteFailure> {
    set_with(
        &mut System,
        binding,
        device_id,
        canonical,
        expected_maximum,
        expected_previous,
        value,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::VecDeque;

    const DEVICE: &str = "intel_backlight";
    const CANONICAL: &str = "/sys/devices/pci0000:00/0000:00:02.0/backlight/intel_backlight";
    const USER_PATH: &str = "/org/freedesktop/login1/user/_1000";
    const SESSION_PATH: &str = "/org/freedesktop/login1/session/_33";
    const SEAT_PATH: &str = "/org/freedesktop/login1/seat/seat0";

    fn output(stdout: impl Into<String>) -> CommandOutput {
        CommandOutput {
            stdout: stdout.into(),
            stderr: String::new(),
            status: 0,
        }
    }

    fn session_properties() -> Value {
        json!({"type":"a{sv}","data":[{
            "Id":{"type":"s","data":"3"},
            "User":{"type":"(uo)","data":[1000,USER_PATH]},
            "Active":{"type":"b","data":true},
            "Remote":{"type":"b","data":false},
            "State":{"type":"s","data":"active"},
            "Type":{"type":"s","data":"wayland"},
            "Seat":{"type":"(so)","data":["seat0",SEAT_PATH]}
        }]})
    }

    fn seat_properties() -> Value {
        json!({"type":"a{sv}","data":[{
            "Id":{"type":"s","data":"seat0"},
            "ActiveSession":{"type":"(so)","data":["3",SESSION_PATH]}
        }]})
    }

    fn binding() -> Binding {
        Binding {
            session_id: "3".into(),
            object_path: SESSION_PATH.into(),
            uid: 1000,
            seat_id: "seat0".into(),
            seat_path: SEAT_PATH.into(),
        }
    }

    #[derive(Debug)]
    struct Call {
        program: String,
        args: Vec<String>,
        control: bool,
    }

    struct Mock {
        uid: u32,
        canonical: PathBuf,
        class_canonical: Option<PathBuf>,
        sysfs: bool,
        maximum: Result<u32, ControlError>,
        previous: Result<u32, ControlError>,
        reads: Vec<PathBuf>,
        replies: VecDeque<Result<CommandOutput, ControlError>>,
        control_result: Option<Result<CommandOutput, ControlError>>,
        calls: Vec<Call>,
    }

    impl Mock {
        fn new(session: Value, seat: Value, udev: &str) -> Self {
            Self {
                uid: 1000,
                canonical: CANONICAL.into(),
                class_canonical: None,
                sysfs: true,
                maximum: Ok(100),
                previous: Ok(50),
                reads: vec![],
                replies: [
                    Ok(output(json!({"type":"o","data":[USER_PATH]}).to_string())),
                    Ok(output(
                        json!({"type":"(so)","data":["3",SESSION_PATH]}).to_string(),
                    )),
                    Ok(output(session.to_string())),
                    Ok(output(seat.to_string())),
                    Ok(output(udev)),
                ]
                .into(),
                control_result: Some(Ok(output(""))),
                calls: vec![],
            }
        }

        fn standard() -> Self {
            Self::new(session_properties(), seat_properties(), "")
        }

        fn controls(&self) -> usize {
            self.calls.iter().filter(|call| call.control).count()
        }
    }

    impl Runtime for Mock {
        fn effective_uid(&self) -> u32 {
            self.uid
        }
        fn canonicalize(&mut self, path: &Path) -> Result<PathBuf, ControlError> {
            if path == Path::new("/sys/class/backlight/intel_backlight") {
                Ok(self
                    .class_canonical
                    .as_ref()
                    .unwrap_or(&self.canonical)
                    .clone())
            } else if path == Path::new(CANONICAL) {
                Ok(self.canonical.clone())
            } else {
                Err(ControlError::new(
                    "fixture_path_missing",
                    "The fixture has no such path",
                ))
            }
        }
        fn is_sysfs(&mut self, _: &Path) -> Result<bool, ControlError> {
            Ok(self.sysfs)
        }
        fn read_number(&mut self, path: &Path) -> Result<u32, ControlError> {
            self.reads.push(path.into());
            match path.file_name().and_then(|name| name.to_str()) {
                Some("max_brightness") => self.maximum.clone(),
                Some("brightness") => self.previous.clone(),
                _ => panic!("unexpected numeric read"),
            }
        }
        fn query(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, ControlError> {
            self.calls.push(Call {
                program: program.into(),
                args: args.iter().map(|arg| (*arg).into()).collect(),
                control: false,
            });
            self.replies.pop_front().expect("unexpected inspection")
        }
        fn control(&mut self, program: &str, args: &[&str]) -> Result<CommandOutput, ControlError> {
            self.calls.push(Call {
                program: program.into(),
                args: args.iter().map(|arg| (*arg).into()).collect(),
                control: true,
            });
            self.control_result
                .take()
                .expect("unexpected additional control write or rollback")
        }
    }

    fn discover(mock: &mut Mock) -> Result<Binding, ControlError> {
        session_with(mock, DEVICE, Path::new(CANONICAL))
    }

    // @mark mechrevo.display_brightness role=test
    #[test]
    fn primary_display_identity_and_missing_or_empty_seat_defaults_are_verified() {
        for udev in ["", "\n", "ID_SEAT=\n", "ID_SEAT=seat0\n"] {
            let mut mock = Mock::new(session_properties(), seat_properties(), udev);
            assert_eq!(discover(&mut mock).unwrap(), binding());
            assert_eq!(mock.controls(), 0);
        }
    }

    #[test]
    fn all_helpers_and_bus_addresses_are_fixed_and_uid_is_supplied_by_runtime() {
        let mut mock = Mock::standard();
        set_with(
            &mut mock,
            &binding(),
            DEVICE,
            Path::new(CANONICAL),
            100,
            50,
            42,
        )
        .unwrap();
        assert_eq!(mock.controls(), 1);
        assert_eq!(mock.calls[0].args.last().unwrap(), "1000");
        for call in &mock.calls {
            if call.program == UDEVADM {
                assert_eq!(
                    call.args,
                    [
                        "info",
                        "--query=property",
                        "--property=ID_SEAT",
                        "--path",
                        CANONICAL,
                        "--no-pager"
                    ]
                );
            } else {
                assert_eq!(call.program, "/usr/bin/busctl");
                assert_eq!(&call.args[..BUS_ARGS.len()], BUS_ARGS);
                assert!(call
                    .args
                    .contains(&"--address=unix:path=/run/dbus/system_bus_socket".into()));
            }
        }
        let setter = mock.calls.last().unwrap();
        assert!(setter.args.contains(&"--timeout=infinity".into()));
        assert!(setter.args.contains(&"--expect-reply=yes".into()));
        assert_eq!(
            &setter.args[setter.args.len() - 4..],
            ["ssu", "backlight", DEVICE, "42"]
        );
    }

    #[test]
    fn uid_mismatch_is_rejected_without_a_setter() {
        for owner in [
            json!([1001, USER_PATH]),
            json!([1000, "/org/freedesktop/login1/user/_1001"]),
        ] {
            let mut session = session_properties();
            session["data"][0]["User"]["data"] = owner;
            let mut mock = Mock::new(session, seat_properties(), "");
            assert!(matches!(
                set_with(
                    &mut mock,
                    &binding(),
                    DEVICE,
                    Path::new(CANONICAL),
                    100,
                    50,
                    42
                ),
                Err(WriteFailure::NotStarted(_))
            ));
            assert_eq!(mock.controls(), 0);
        }
    }

    #[test]
    fn forged_user_object_is_rejected() {
        let mut mock = Mock::standard();
        mock.replies[0] = Ok(output(
            json!({"type":"o","data":["/org/freedesktop/login1/user/_1001"]}).to_string(),
        ));
        assert!(discover(&mut mock).is_err());
        assert_eq!(mock.calls.len(), 1);
    }

    #[test]
    fn empty_aliased_or_malformed_display_is_rejected() {
        for reply in [
            json!({"type":"(so)","data":["","/"]}),
            json!({"type":"(so)","data":["3","/org/freedesktop/login1/session/self"]}),
            json!({"type":"(so)","data":["3","/org/freedesktop/login1/session/auto"]}),
            json!({"type":"(so)","data":["3",SESSION_PATH,"extra"]}),
            json!({"type":"s","data":["3",SESSION_PATH]}),
        ] {
            let mut mock = Mock::standard();
            mock.replies[1] = Ok(output(reply.to_string()));
            assert!(discover(&mut mock).is_err());
            assert_eq!(mock.controls(), 0);
        }
    }

    #[test]
    fn inactive_remote_closing_or_non_display_sessions_are_rejected() {
        for (key, value) in [
            ("Active", json!(false)),
            ("Remote", json!(true)),
            ("State", json!("closing")),
            ("Type", json!("unspecified")),
            ("Id", json!("4")),
        ] {
            let mut session = session_properties();
            session["data"][0][key]["data"] = value;
            let mut mock = Mock::new(session, seat_properties(), "");
            assert!(discover(&mut mock).is_err());
            assert_eq!(mock.controls(), 0);
        }
    }

    #[test]
    fn x11_wayland_and_local_tty_are_supported() {
        for kind in ["x11", "wayland", "tty"] {
            let mut session = session_properties();
            session["data"][0]["Type"]["data"] = json!(kind);
            assert!(discover(&mut Mock::new(session, seat_properties(), "")).is_ok());
        }
    }

    #[test]
    fn seat_must_identify_the_same_foreground_session() {
        for (id, path) in [
            ("4", SESSION_PATH),
            ("3", "/org/freedesktop/login1/session/_34"),
        ] {
            let mut seat = seat_properties();
            seat["data"][0]["ActiveSession"]["data"] = json!([id, path]);
            let mut mock = Mock::new(session_properties(), seat, "");
            assert!(matches!(
                set_with(
                    &mut mock,
                    &binding(),
                    DEVICE,
                    Path::new(CANONICAL),
                    100,
                    50,
                    42
                ),
                Err(WriteFailure::NotStarted(_))
            ));
            assert_eq!(mock.controls(), 0);
        }
        let mut seat = seat_properties();
        seat["data"][0]["Id"]["data"] = json!("seat1");
        assert!(discover(&mut Mock::new(session_properties(), seat, "")).is_err());
    }

    #[test]
    fn absent_or_hostile_seat_and_untyped_properties_are_rejected() {
        for value in [
            json!(["", "/"]),
            json!(["seat0", "/org/freedesktop/login1/seat/../session/_33"]),
        ] {
            let mut session = session_properties();
            session["data"][0]["Seat"]["data"] = value;
            assert!(discover(&mut Mock::new(session, seat_properties(), "")).is_err());
        }
        for key in ["User", "Active", "Remote", "Seat", "State", "Type", "Id"] {
            let mut session = session_properties();
            session["data"][0][key]["type"] = json!("v");
            assert!(discover(&mut Mock::new(session, seat_properties(), "")).is_err());
        }
    }

    #[test]
    fn udev_seat_mismatch_error_or_malformed_output_never_defaults_to_seat0() {
        for output in [
            "ID_SEAT=seat1\n",
            "ID_SEAT=seat0\nID_SEAT=seat0\n",
            "OTHER=seat0\n",
            "ID_SEAT=seat0 extra\n",
        ] {
            let mut mock = Mock::new(session_properties(), seat_properties(), output);
            assert!(discover(&mut mock).is_err());
            assert_eq!(mock.controls(), 0);
        }
        let mut mock = Mock::standard();
        mock.replies[4] = Err(ControlError::new("command_failed", "udevadm failed"));
        assert!(discover(&mut mock).is_err());
    }

    #[test]
    fn arbitrary_non_sysfs_or_retargeted_paths_never_reach_a_helper() {
        for (device, path) in [
            ("../intel_backlight", CANONICAL),
            (DEVICE, "/tmp/intel_backlight"),
            (DEVICE, "/sys/devices-evil/intel_backlight"),
            ("other", CANONICAL),
        ] {
            let mut mock = Mock::standard();
            assert!(session_with(&mut mock, device, Path::new(path)).is_err());
            assert!(mock.calls.is_empty());
        }
        let mut mock = Mock::standard();
        mock.sysfs = false;
        assert!(discover(&mut mock).is_err());
        assert!(mock.calls.is_empty());
        let mut mock = Mock::standard();
        mock.canonical = "/sys/devices/other/intel_backlight".into();
        assert!(discover(&mut mock).is_err());
        assert!(mock.calls.is_empty());
        let mut mock = Mock::standard();
        mock.class_canonical = Some("/sys/devices/other/intel_backlight".into());
        assert!(discover(&mut mock).is_err());
        assert!(mock.calls.is_empty());
    }

    #[test]
    fn session_change_since_planning_prevents_a_write() {
        for changed in [
            Binding {
                uid: 1001,
                ..binding()
            },
            Binding {
                session_id: "4".into(),
                ..binding()
            },
            Binding {
                object_path: "/org/freedesktop/login1/session/_34".into(),
                ..binding()
            },
            Binding {
                seat_id: "seat1".into(),
                ..binding()
            },
            Binding {
                seat_path: "/org/freedesktop/login1/seat/seat1".into(),
                ..binding()
            },
        ] {
            let mut mock = Mock::standard();
            assert!(matches!(
                set_with(
                    &mut mock,
                    &changed,
                    DEVICE,
                    Path::new(CANONICAL),
                    100,
                    50,
                    42
                ),
                Err(WriteFailure::NotStarted(_))
            ));
            assert_eq!(mock.controls(), 0);
        }
    }

    #[test]
    fn lost_reply_and_post_spawn_capture_failure_are_unknown_with_no_rollback() {
        for failure in [
            "command_failed",
            "command_timeout",
            "oem_outcome_unknown",
            "oem_helper_liveness_unknown",
        ] {
            let mut mock = Mock::standard();
            mock.control_result = Some(Err(ControlError::new(
                failure,
                "No completion acknowledgment",
            )));
            let result = set_with(
                &mut mock,
                &binding(),
                DEVICE,
                Path::new(CANONICAL),
                100,
                50,
                42,
            )
            .unwrap_err();
            let WriteFailure::Unknown(error) = result else {
                panic!("lost acknowledgment was not unknown")
            };
            assert_eq!(error.code, failure);
            assert_eq!(error.message, "No completion acknowledgment");
            assert_eq!(mock.controls(), 1);
            assert!(mock.control_result.is_none());
        }
    }

    #[test]
    fn nonzero_or_unexpected_stdout_is_unknown_with_no_rollback() {
        for (status, stdout) in [(1, ""), (-1, ""), (0, "unexpected reply")] {
            let mut mock = Mock::standard();
            mock.control_result = Some(Ok(CommandOutput {
                status,
                ..output(stdout)
            }));
            assert!(matches!(
                set_with(
                    &mut mock,
                    &binding(),
                    DEVICE,
                    Path::new(CANONICAL),
                    100,
                    50,
                    42
                ),
                Err(WriteFailure::Unknown(_))
            ));
            assert_eq!(mock.controls(), 1);
        }
    }

    #[test]
    fn only_a_proven_spawn_failure_is_not_started() {
        let mut mock = Mock::standard();
        mock.control_result = Some(Err(ControlError::new(
            "command_unavailable",
            "No executable",
        )));
        assert!(matches!(
            set_with(
                &mut mock,
                &binding(),
                DEVICE,
                Path::new(CANONICAL),
                100,
                50,
                42
            ),
            Err(WriteFailure::NotStarted(_))
        ));
        assert_eq!(mock.controls(), 1);
    }

    #[test]
    fn final_maximum_and_previous_recheck_rejects_stale_or_out_of_range_values() {
        for (maximum, previous, target, expected_code) in [
            (101, 50, 42, "invalid_brightness_plan"),
            (0, 0, 0, "invalid_brightness_plan"),
            (100, 101, 42, "invalid_brightness_plan"),
            (100, 50, 101, "invalid_brightness_plan"),
            (100, 51, 42, "brightness_plan_stale"),
        ] {
            let mut mock = Mock::standard();
            mock.maximum = Ok(maximum);
            mock.previous = Ok(previous);
            let error = set_with(
                &mut mock,
                &binding(),
                DEVICE,
                Path::new(CANONICAL),
                100,
                50,
                target,
            )
            .unwrap_err();
            let WriteFailure::NotStarted(error) = error else {
                panic!("invalid final numeric state started a setter")
            };
            assert_eq!(error.code, expected_code);
            assert_eq!(mock.controls(), 0);
            assert_eq!(
                mock.reads,
                [
                    Path::new(CANONICAL).join("max_brightness"),
                    Path::new(CANONICAL).join("brightness")
                ]
            );
        }
    }

    #[test]
    fn final_numeric_read_failure_prevents_a_setter() {
        for failing_maximum in [true, false] {
            let mut mock = Mock::standard();
            let failure = Err(ControlError::new(
                "brightness_read_failed",
                "Device disappeared",
            ));
            if failing_maximum {
                mock.maximum = failure;
            } else {
                mock.previous = failure;
            }
            assert!(matches!(
                set_with(
                    &mut mock,
                    &binding(),
                    DEVICE,
                    Path::new(CANONICAL),
                    100,
                    50,
                    42
                ),
                Err(WriteFailure::NotStarted(_))
            ));
            assert_eq!(mock.controls(), 0);
        }
    }

    #[test]
    fn numeric_boundaries_are_supported_without_overflow() {
        for (maximum, previous, target) in [(100, 50, 0), (100, 50, 100), (u32::MAX, 0, u32::MAX)] {
            let mut mock = Mock::standard();
            mock.maximum = Ok(maximum);
            mock.previous = Ok(previous);
            set_with(
                &mut mock,
                &binding(),
                DEVICE,
                Path::new(CANONICAL),
                maximum,
                previous,
                target,
            )
            .unwrap();
            assert_eq!(mock.controls(), 1);
            assert_eq!(
                mock.calls.last().unwrap().args.last().unwrap(),
                &target.to_string()
            );
        }
    }

    #[test]
    fn numeric_sysfs_parser_rejects_overflow_junk_and_oversized_reads() {
        assert_eq!(parse_number(b"0\n").unwrap(), 0);
        assert_eq!(parse_number(b"4294967295\n").unwrap(), u32::MAX);
        for invalid in [
            b"".as_slice(),
            b"-1",
            b"+1",
            b"4294967296",
            b"12 13",
            b"0xff",
            b"42\0",
            b"\xff",
        ] {
            assert!(parse_number(invalid).is_err());
        }
        assert!(parse_number(&[b'0'; 33]).is_err());
    }

    #[test]
    fn query_failure_does_not_fall_back_to_an_environment_session() {
        let mut mock = Mock::standard();
        mock.replies[0] = Err(ControlError::new("command_failed", "No system logind"));
        assert!(matches!(
            set_with(
                &mut mock,
                &binding(),
                DEVICE,
                Path::new(CANONICAL),
                100,
                50,
                42
            ),
            Err(WriteFailure::NotStarted(_))
        ));
        assert_eq!(mock.calls.len(), 1);
        assert_eq!(mock.controls(), 0);
        let mut seat = seat_properties();
        seat["data"][0]["ActiveSession"]["type"] = json!("s");
        assert!(discover(&mut Mock::new(session_properties(), seat, "")).is_err());
    }

    #[test]
    fn failed_helper_evidence_is_retained_without_replacing_the_cause() {
        let mut mock = Mock::standard();
        mock.control_result = Some(Ok(CommandOutput {
            status: 7,
            stdout: "partial reply\n".into(),
            stderr: "Call failed: connection lost\n".into(),
        }));
        let error = set_with(
            &mut mock,
            &binding(),
            DEVICE,
            Path::new(CANONICAL),
            100,
            50,
            42,
        )
        .unwrap_err();
        let WriteFailure::Unknown(error) = error else {
            panic!("failed helper lost its unknown outcome")
        };
        assert!(error.details.contains(&"helper_status: 7".into()));
        assert!(error.details.contains(&"stdout: partial reply\n".into()));
        assert!(error
            .details
            .contains(&"stderr: Call failed: connection lost\n".into()));
        assert_eq!(mock.controls(), 1);
    }
}
