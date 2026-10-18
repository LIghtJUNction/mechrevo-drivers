//! Dedicated production API tests. FIFO children are bounded so an accidental
//! full snapshot or suspended-input read fails instead of hanging the test run.
use super::*;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mechrevo-thermal-api-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(path.join("class/hwmon")).unwrap();
        Self(path)
    }
    fn service(&self) -> Service {
        Service::with_linux_sysfs(&self.0)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn put(root: &Path, path: &str, value: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}
fn hwmon(root: &Path, class_name: &str, interface: &str) -> PathBuf {
    use std::os::unix::fs::symlink;
    let physical = root.join("devices/pci0000:00/0000:01:00.0");
    let hwmon = physical.join("hwmon").join(interface);
    fs::create_dir_all(&hwmon).unwrap();
    symlink(&physical, hwmon.join("device")).unwrap();
    symlink(&hwmon, root.join("class/hwmon").join(class_name)).unwrap();
    fs::write(hwmon.join("name"), "fixture_chip").unwrap();
    fs::write(hwmon.join("temp1_input"), "48000").unwrap();
    fs::write(hwmon.join("temp1_label"), "Package id 0").unwrap();
    fs::write(hwmon.join("fan1_input"), "2400").unwrap();
    hwmon
}
fn fifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    // SAFETY: NUL-terminated fixture pathname; no real sysfs/device is accessed.
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
}

/// OEM/backlight readers deliberately reject FIFOs after a nonblocking open.
/// Watch opens as well as bounding the child: a quick rejection is still an
/// unwanted control query from the thermal-only entry point.
struct OpenWatch(std::fs::File);
impl OpenWatch {
    fn new(root: &Path, case: &str) -> Self {
        use std::os::{fd::FromRawFd, unix::ffi::OsStrExt};
        // SAFETY: flags only; returns a new owned Linux inotify descriptor.
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        assert!(fd >= 0, "inotify is required by the no-open regression");
        // SAFETY: fd is new and has exactly one owner.
        let watch = Self(unsafe { std::fs::File::from_raw_fd(fd) });
        let paths: Vec<PathBuf> = match case {
            "oem" => vec![root.join("devices/platform/tuxedo_keyboard/fn_lock")],
            "brightness" => vec![root.join("class/backlight/panel/max_brightness")],
            "policy" => vec![root.join("firmware/acpi/platform_profile_choices")],
            "identity" => vec![root.join("class/dmi/id/product_name")],
            "suspended" | "suspending" | "resuming" | "error" => vec![
                root.join("devices/pci0000:00/0000:01:00.0/hwmon/hwmon7/temp1_input"),
                root.join("devices/pci0000:00/0000:01:00.0/hwmon/hwmon7/fan1_input"),
                root.join("devices/pci0000:00/0000:01:00.0/hwmon/hwmon7/temp1_label"),
            ],
            _ => unreachable!(),
        };
        for path in paths {
            let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
            // SAFETY: live inotify fd and NUL-terminated fixture pathname.
            assert!(unsafe { libc::inotify_add_watch(fd, path.as_ptr(), libc::IN_OPEN) } >= 0);
        }
        watch
    }
    fn opened(&mut self) -> bool {
        use std::io::Read;
        let mut events = [0u8; 4096];
        match self.0.read(&mut events) {
            Ok(bytes) => bytes != 0,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(error) => panic!("Cannot inspect no-open guard: {error}"),
        }
    }
}

#[test]
fn sample_thermal_returns_only_live_hwmon_values_and_a_local_read_interval() {
    let fixture = Fixture::new();
    hwmon(&fixture.0, "hwmon0", "hwmon7");
    put(&fixture.0, "class/power_supply/BAT0/type", "Battery");
    put(&fixture.0, "class/power_supply/BAT0/capacity", "93");
    let before = Instant::now();
    let sample = fixture.service().sample_thermal().unwrap();
    let after = Instant::now();
    assert!(before <= sample.started_at);
    assert!(sample.started_at <= sample.finished_at);
    assert!(sample.finished_at <= after);
    assert_eq!(sample.readings.len(), 2);
    assert!(sample.diagnostics.is_empty());
    let temperature = sample
        .readings
        .iter()
        .find(|r| r.kind == "temperature")
        .unwrap();
    assert_eq!(temperature.value, Some(48.0));
    assert_eq!(temperature.unit, "°C");
    assert_eq!(temperature.label, "fixture_chip: Package id 0");
    let fan = sample
        .readings
        .iter()
        .find(|r| r.kind == "fan_speed")
        .unwrap();
    assert_eq!(fan.value, Some(2400.0));
    assert_eq!(fan.unit, "RPM");
    assert!(sample
        .readings
        .iter()
        .all(|r| r.status == CapabilityStatus::Available));
    let snapshot = fixture.service().snapshot();
    let thermal: Vec<_> = snapshot
        .telemetry
        .into_iter()
        .filter(|r| matches!(r.kind.as_str(), "temperature" | "fan_speed"))
        .collect();
    assert_eq!(
        serde_json::to_value(&sample.readings).unwrap(),
        serde_json::to_value(thermal).unwrap()
    );
    assert_eq!(
        snapshot
            .diagnostics
            .iter()
            .filter(|d| d.code == "identity_unavailable")
            .count(),
        1
    );
}

#[test]
fn sample_thermal_reads_active_unsupported_and_absent_runtime_status() {
    let fixture = Fixture::new();
    let path = hwmon(&fixture.0, "hwmon0", "hwmon7");
    for (index, state) in [None, Some("active"), Some("unsupported")]
        .into_iter()
        .enumerate()
    {
        if let Some(state) = state {
            put(
                &fixture.0,
                "devices/pci0000:00/0000:01:00.0/power/runtime_status",
                state,
            );
        }
        // Different readings prove that each allowed state still opens the
        // current input, rather than returning a prior/cached observation.
        let milli_celsius = 48_000 + index * 1_000;
        let rpm = 2_400 + index * 100;
        fs::write(path.join("temp1_input"), milli_celsius.to_string()).unwrap();
        fs::write(path.join("fan1_input"), rpm.to_string()).unwrap();
        let sample = fixture.service().sample_thermal().unwrap();
        assert_eq!(sample.readings.len(), 2);
        assert!(sample
            .readings
            .iter()
            .all(|r| r.status == CapabilityStatus::Available));
        assert_eq!(
            sample
                .readings
                .iter()
                .find(|r| r.kind == "temperature")
                .unwrap()
                .value,
            Some(milli_celsius as f64 / 1000.0)
        );
        assert_eq!(
            sample
                .readings
                .iter()
                .find(|r| r.kind == "fan_speed")
                .unwrap()
                .value,
            Some(rpm as f64)
        );
    }
}

#[test]
fn sample_thermal_deduplicates_aliases_without_merging_distinct_interfaces() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let first = hwmon(&fixture.0, "hwmon0", "hwmon7");
    let second = hwmon(&fixture.0, "hwmon1", "hwmon8");
    symlink(&first, fixture.0.join("class/hwmon/hwmon2")).unwrap();
    let sample = fixture.service().sample_thermal().unwrap();
    assert_eq!(sample.readings.len(), 4);
    assert_eq!(
        sample
            .diagnostics
            .iter()
            .filter(|d| d.code == "sensor_source_alias")
            .count(),
        2
    );
    let ids: std::collections::HashSet<_> = sample.readings.iter().map(|r| &r.id).collect();
    assert_eq!(ids.len(), 4);
    assert_eq!(
        sample
            .readings
            .iter()
            .map(|r| &r.device_id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        1
    );
    assert!(sample
        .readings
        .iter()
        .any(|r| r.id == first.join("temp1_input").to_string_lossy()));
    assert!(sample
        .readings
        .iter()
        .any(|r| r.id == second.join("temp1_input").to_string_lossy()));
}

#[test]
fn sample_thermal_invalid_and_removed_inputs_never_become_zero_or_cached_values() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new();
    let path = hwmon(&fixture.0, "hwmon0", "hwmon7");
    let first = fixture.service().sample_thermal().unwrap();
    fs::write(path.join("temp1_input"), "NaN").unwrap();
    fs::write(path.join("fan1_input"), "-1").unwrap();
    let invalid = fixture.service().sample_thermal().unwrap();
    assert_eq!(invalid.readings.len(), 2);
    for (old, invalid) in first.readings.iter().zip(&invalid.readings) {
        assert_eq!(old.id, invalid.id);
        assert_eq!(invalid.value, None);
        assert_eq!(invalid.status, CapabilityStatus::Unavailable);
        assert!(invalid
            .reason
            .as_deref()
            .unwrap()
            .contains("not reported as zero"));
    }
    fs::remove_file(path.join("temp1_input")).unwrap();
    symlink(path.join("removed-by-driver"), path.join("temp1_input")).unwrap();
    let removed_input = fixture.service().sample_thermal().unwrap();
    let removed = removed_input
        .readings
        .iter()
        .find(|r| r.kind == "temperature")
        .unwrap();
    assert_eq!(
        removed.id,
        first
            .readings
            .iter()
            .find(|r| r.kind == "temperature")
            .unwrap()
            .id
    );
    assert_eq!(removed.value, None);
    assert_eq!(removed.status, CapabilityStatus::Unavailable);
    fs::remove_dir_all(&path).unwrap();
    assert!(fixture
        .service()
        .sample_thermal()
        .unwrap()
        .readings
        .is_empty());
}

#[test]
fn sample_thermal_missing_or_non_directory_hwmon_is_an_error_without_changing_snapshot_fallback() {
    let fixture = Fixture::new();
    assert!(fixture
        .service()
        .sample_thermal()
        .unwrap()
        .readings
        .is_empty());
    fs::remove_dir(fixture.0.join("class/hwmon")).unwrap();
    assert_eq!(
        fixture.service().sample_thermal().unwrap_err().code,
        "thermal_sampling_unavailable"
    );
    assert!(fixture.service().snapshot().telemetry.is_empty());
    fs::write(fixture.0.join("class/hwmon"), "not a directory").unwrap();
    assert_eq!(
        fixture.service().sample_thermal().unwrap_err().code,
        "thermal_sampling_unavailable"
    );
    assert!(fixture.service().snapshot().telemetry.is_empty());
}

#[test]
fn sample_thermal_bypasses_oem_brightness_policy_and_suspended_input_fifos() {
    use std::{
        process::{Command, Stdio},
        time::Duration,
    };
    const CHILD_ROOT: &str = "MECHREVO_THERMAL_ONLY_FIFO_ROOT";
    const CHILD_CASE: &str = "MECHREVO_THERMAL_ONLY_FIFO_CASE";
    const COMPLETED: &str = "THERMAL_ONLY_FIFO_COMPLETED";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let case = std::env::var(CHILD_CASE).unwrap();
        let mut watched = OpenWatch::new(&root, &case);
        let service = Service::with_linux_sysfs(&root);
        let sample = service.sample_thermal().unwrap();
        assert!(
            !watched.opened(),
            "thermal-only sampling opened a {case} FIFO"
        );
        assert_eq!(sample.readings.len(), 2);
        if matches!(
            case.as_str(),
            "suspended" | "suspending" | "resuming" | "error"
        ) {
            assert!(sample.readings.iter().all(|r| r.value.is_none()
                && r.status == CapabilityStatus::Unavailable
                && r.reason.as_deref().unwrap().contains("not read")
                && r.reason.as_deref().unwrap().contains(&case)));
        } else {
            assert!(sample.readings.iter().all(|r| r.value.is_some()));
        }
        // Positive controls prove the fixture really reaches the production
        // control reader, whose nonblocking FIFO rejection cannot hide an open.
        match case.as_str() {
            "oem" => {
                let _ = service.oem_controls();
                assert!(
                    watched.opened(),
                    "OEM fixture did not reach the real reader"
                );
            }
            "brightness" => {
                let _ = service.brightness_devices();
                assert!(
                    watched.opened(),
                    "brightness fixture did not reach the real reader"
                );
            }
            _ => {}
        }
        println!("{COMPLETED}");
        return;
    }
    for case in [
        "oem",
        "brightness",
        "policy",
        "identity",
        "suspended",
        "suspending",
        "resuming",
        "error",
    ] {
        let fixture = Fixture::new();
        let path = hwmon(&fixture.0, "hwmon0", "hwmon7");
        match case {
            "oem" => {
                put(&fixture.0, "module/tuxedo_keyboard/srcversion", "ABC123");
                fifo(&fixture.0.join("devices/platform/tuxedo_keyboard/fn_lock"));
                fifo(
                    &fixture
                        .0
                        .join("devices/platform/tuxedo_keyboard/charging_profile"),
                );
            }
            "brightness" => {
                for attribute in ["brightness", "actual_brightness", "max_brightness", "type"] {
                    fifo(&fixture.0.join("class/backlight/panel").join(attribute));
                }
            }
            "policy" => {
                fifo(&fixture.0.join("firmware/acpi/platform_profile"));
                fifo(&fixture.0.join("firmware/acpi/platform_profile_choices"));
            }
            "identity" => fifo(&fixture.0.join("class/dmi/id/product_name")),
            "suspended" | "suspending" | "resuming" | "error" => {
                for input in ["temp1_input", "fan1_input", "temp1_label"] {
                    let input = path.join(input);
                    if input.exists() {
                        fs::remove_file(&input).unwrap();
                    }
                    fifo(&input);
                }
                put(
                    &fixture.0,
                    "devices/pci0000:00/0000:01:00.0/power/runtime_status",
                    case,
                );
            }
            _ => unreachable!(),
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "thermal_tests::sample_thermal_bypasses_oem_brightness_policy_and_suspended_input_fifos", "--nocapture"])
            .env(CHILD_ROOT, &fixture.0).env(CHILD_CASE, case)
            .env("PATH", "/no-external-commands")
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let completion = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                other => {
                    break Err(format!(
                        "thermal-only {case} blocked or wait failed: {other:?}"
                    ))
                }
            }
        };
        if completion.is_err() {
            let _ = child.kill();
        }
        let output = child.wait_with_output().unwrap();
        assert!(completion.is_ok(), "{completion:?}");
        assert!(
            output.status.success(),
            "{case}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(COMPLETED),
            "{case}: exact regression child did not finish"
        );
    }
}
