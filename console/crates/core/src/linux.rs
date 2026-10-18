use crate::*;
use std::{
    collections::HashSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
};

pub(crate) fn read(path: &Path) -> Option<String> {
    let mut value = String::new();
    File::open(path)
        .ok()?
        .take(8193)
        .read_to_string(&mut value)
        .ok()?;
    if value.len() > 8192 {
        return None;
    }
    let value = value.trim_matches(|c: char| c.is_whitespace() || c == '\0');
    (!value.is_empty()).then(|| value.to_owned())
}
fn try_paths(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(root)?
        .filter_map(Result::ok)
        .take(256)
        .map(|x| x.path())
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}
fn paths(root: &Path) -> Vec<PathBuf> {
    try_paths(root).unwrap_or_default()
}

fn source(path: &Path, device: &str) -> EvidenceSource {
    EvidenceSource {
        backend: "linux_sysfs".into(),
        path: path.to_string_lossy().into_owned(),
        device_id: Some(device.into()),
    }
}
fn device_id(path: &Path) -> String {
    let path = fs::canonicalize(path.join("device"))
        .unwrap_or_else(|_| fs::canonicalize(path).unwrap_or_else(|_| path.into()));
    path.to_string_lossy().into_owned()
}
fn sensor_id(path: &Path) -> String {
    // A physical device can expose several hwmon interfaces with the same
    // channel names. Resolving only its device link would merge those sensors.
    // Canonicalization inspects paths, not the input value, including while the
    // device is suspended. Fall back to its resolved interface path if an input
    // disappears between directory enumeration and canonicalization.
    let canonical = fs::canonicalize(path).unwrap_or_else(|_| {
        path.parent()
            .and_then(|parent| fs::canonicalize(parent).ok())
            .zip(path.file_name())
            .map(|(parent, name)| parent.join(name))
            .unwrap_or_else(|| path.into())
    });
    canonical.to_string_lossy().into_owned()
}
fn reading(
    path: &Path,
    device: &str,
    id: &str,
    label: String,
    spec: (&str, &str, f64, f64, f64),
) -> SensorReading {
    let (kind, unit, scale, min, max) = spec;
    let value = read(path)
        .and_then(|s| s.parse::<f64>().ok())
        .map(|v| v / scale)
        .filter(|v| v.is_finite() && *v >= min && *v <= max);
    SensorReading {
        id: id.into(),
        label,
        device_id: device.into(),
        kind: kind.into(),
        value,
        unit: unit.into(),
        source: source(path, device),
        status: if value.is_some() {
            CapabilityStatus::Available
        } else {
            CapabilityStatus::Unavailable
        },
        reason: value
            .is_none()
            .then(|| "Missing, unreadable or invalid sensor value; not reported as zero".into()),
    }
}
/// Only hwmon temperature/fan inputs; no model, policy, OEM or helper discovery.
fn collect_hwmon(
    root: &Path,
    telemetry: &mut Vec<SensorReading>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<(), ControlError> {
    let mut sensor_sources = HashSet::new();
    for hwmon in try_paths(&root.join("class/hwmon")).map_err(|error| {
        ControlError::new(
            "thermal_sampling_unavailable",
            format!("Cannot enumerate hwmon sensors: {error}"),
        )
    })? {
        let chip = read(&hwmon.join("name")).unwrap_or_else(|| "unknown hwmon".into());
        let device = device_id(&hwmon);
        // This is a pre-read observation, not a runtime-PM lock: the device
        // may change state after it. Never request a PM transition here.
        let runtime_status = read(&hwmon.join("device/power/runtime_status"));
        let skip_state = runtime_status
            .as_deref()
            .filter(|state| matches!(*state, "suspended" | "suspending" | "resuming" | "error"));
        for input in paths(&hwmon) {
            let filename = input.file_name().unwrap_or_default().to_string_lossy();
            let (prefix, kind, unit, scale, min, max) =
                if filename.starts_with("temp") && filename.ends_with("_input") {
                    ("temp", "temperature", "°C", 1000., -273.15, 300.)
                } else if filename.starts_with("fan") && filename.ends_with("_input") {
                    ("fan", "fan_speed", "RPM", 1., 0., 100000.)
                } else {
                    continue;
                };
            let stem = filename.trim_end_matches("_input");
            if stem[prefix.len()..].is_empty()
                || !stem[prefix.len()..].chars().all(|c| c.is_ascii_digit())
            {
                continue;
            }
            let id = sensor_id(&input);
            // Class aliases of one canonical input describe one observation.
            // Keep the first sorted class path as provenance; equal readings
            // from different canonical inputs remain separate sensors.
            if !sensor_sources.insert(id.clone()) {
                diagnostics.push(Diagnostic {
                    code: "sensor_source_alias".into(),
                    message: format!(
                        "Skipped duplicate hwmon source {}; canonical input={id}",
                        input.display()
                    ),
                });
                continue;
            }
            if let Some(state) = skip_state {
                telemetry.push(SensorReading {
                    id,
                    label: format!("{chip}: {stem}"),
                    device_id: device.clone(),
                    kind: kind.into(),
                    value: None,
                    unit: unit.into(),
                    source: source(&input, &device),
                    status: CapabilityStatus::Unavailable,
                    reason: Some(format!(
                        "Device runtime PM state is {state}; sensor was not read during this state"
                    )),
                });
                continue;
            }
            let label = read(&hwmon.join(format!("{stem}_label"))).unwrap_or_else(|| stem.into());
            // Keep the chip and its own label. An EC temperature is not a CPU core.
            telemetry.push(reading(
                &input,
                &device,
                &id,
                format!("{chip}: {label}"),
                (kind, unit, scale, min, max),
            ));
        }
    }
    Ok(())
}

pub(crate) fn sample_thermal(root: &Path) -> Result<ThermalSample, ControlError> {
    let started_at = std::time::Instant::now();
    let mut readings = Vec::new();
    let mut diagnostics = Vec::new();
    collect_hwmon(root, &mut readings, &mut diagnostics)?;
    Ok(ThermalSample {
        readings,
        diagnostics,
        started_at,
        finished_at: std::time::Instant::now(),
    })
}

pub(crate) fn policy(root: &Path) -> PowerPolicy {
    let path = root.join("firmware/acpi/platform_profile");
    let choices = root.join("firmware/acpi/platform_profile_choices");
    let options = read(&choices)
        .map(|v| v.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default();
    PowerPolicy {
        scope: "Linux ACPI platform profile (not OEM preset or GPU tuning)".into(),
        current: read(&path),
        options,
        source: Some(source(&path, "acpi_platform_profile")),
    }
}
// @mark mechrevo.telemetry role=implementation
pub(crate) fn snapshot(root: &Path) -> Snapshot {
    let dmi = root.join("class/dmi/id");
    let mut identity = Identity {
        manufacturer: read(&dmi.join("sys_vendor")),
        product_name: read(&dmi.join("product_name")),
        product_version: read(&dmi.join("product_version")),
        board_name: read(&dmi.join("board_name")),
        bios_version: read(&dmi.join("bios_version")),
        ..Default::default()
    };
    identity.qualify();
    let mut telemetry = Vec::new();
    let mut diagnostics = Vec::new();
    if identity.product_name.is_none() {
        diagnostics.push(Diagnostic {
            code: "identity_unavailable".into(),
            message: "DMI product identity is missing or unreadable; no model match assumed".into(),
        });
    }
    // Keep snapshot fallback/diagnostic behavior when hwmon is unavailable.
    let _ = collect_hwmon(root, &mut telemetry, &mut diagnostics);
    let mut batteries = 0;
    for supply in paths(&root.join("class/power_supply")) {
        let device = device_id(&supply);
        let name = supply.file_name().unwrap_or_default().to_string_lossy();
        match read(&supply.join("type")).as_deref() {
            Some("Battery") => {
                batteries += 1;
                for (file, kind, unit, scale, min, max) in [
                    ("capacity", "battery_capacity", "%", 1., 0., 100.),
                    ("energy_now", "battery_energy", "Wh", 1_000_000., 0., 1000.),
                    ("power_now", "battery_power", "W", 1_000_000., 0., 1000.),
                    ("voltage_now", "battery_voltage", "V", 1_000_000., 0., 100.),
                    ("current_now", "battery_current", "A", 1_000_000., 0., 100.),
                ] {
                    if file == "capacity" || supply.join(file).exists() {
                        telemetry.push(reading(
                            &supply.join(file),
                            &device,
                            &format!("{device}/{file}"),
                            format!("{name}: {file}"),
                            (kind, unit, scale, min, max),
                        ));
                    }
                }
                // Status is a separate observation; external power does not imply charging.
                if let Some(status) = read(&supply.join("status")) {
                    diagnostics.push(Diagnostic {
                        code: "battery_status".into(),
                        message: format!(
                            "{name}: {status}; source={}",
                            supply.join("status").display()
                        ),
                    });
                }
            }
            Some("Mains" | "USB" | "USB_C" | "USB_PD") => {
                telemetry.push(reading(
                    &supply.join("online"),
                    &device,
                    &format!("{device}/online"),
                    format!("{name}: external power online"),
                    ("external_power_online", "boolean", 1., 0., 1.),
                ));
            }
            _ => {}
        }
    }
    let power_policy = policy(root);
    let profile_available = power_policy
        .current
        .as_ref()
        .is_some_and(|v| power_policy.options.contains(v))
        && !power_policy.options.is_empty();
    let mut capabilities = proprietary_capabilities();
    capabilities.push(Capability{id:"power_profile".into(),label:"OS platform power profile".into(),status:if profile_available{CapabilityStatus::Available}else{CapabilityStatus::Unavailable},reason:if profile_available{"Kernel exposes current platform profile and enumerated choices. Write permission and readback are checked on apply."}else{"Missing/unreadable current platform_profile or choices; no OEM profile guessed."}.into(),source:power_policy.source.clone(),options:power_policy.options.clone()});
    capabilities.push(Capability{id:"telemetry".into(),label:"OS sensor telemetry".into(),status:if telemetry.iter().any(|r|r.value.is_some()){CapabilityStatus::Available}else{CapabilityStatus::Unavailable},reason:"Each sensor retains its device identity, label, unit and sysfs source; missing values are unknown.".into(),source:None,options:vec![]});
    if batteries == 0 {
        diagnostics.push(Diagnostic {
            code: "battery_unavailable".into(),
            message: "No battery power_supply was found; this is unknown, not 0%".into(),
        });
    }
    Snapshot {
        platform: "linux".into(),
        identity,
        telemetry,
        capabilities,
        power_policy,
        diagnostics,
        brightness: vec![],
        oem_controls: vec![],
    }
}
// @mark mechrevo.os_power_policy role=implementation
pub(crate) fn plan(root: &Path, requested: PowerProfile) -> Result<ProfilePlan, ControlError> {
    let state = policy(root);
    let previous = state.current.ok_or_else(|| {
        ControlError::new(
            "profile_unavailable",
            "Linux platform_profile is missing or unreadable",
        )
    })?;
    let target = profile_target("linux", requested).unwrap();
    if !state.options.iter().any(|v| v == target) || !state.options.contains(&previous) {
        return Err(ControlError::new(
            "profile_unsupported",
            format!(
                "Requested {target} or current profile is not in kernel choices: {}",
                state.options.join(", ")
            ),
        ));
    }
    Ok(ProfilePlan {
        requested,
        target: target.into(),
        previous,
        scope: state.scope,
        source: state.source.unwrap(),
    })
}
pub(crate) fn apply(root: &Path, requested: &ProfilePlan) -> Result<ApplyResult, ControlError> {
    let fresh = plan(root, requested.requested)?;
    if fresh.target != requested.target
        || fresh.previous != requested.previous
        || fresh.scope != requested.scope
        || fresh.source != requested.source
    {
        return Err(ControlError::new(
            "stale_plan",
            "Profile state, backend or target changed; create a new plan",
        ));
    }
    let path = root.join("firmware/acpi/platform_profile");
    apply_verified(
        requested,
        || {
            read(&path).ok_or_else(|| {
                ControlError::new("readback_failed", "Cannot read current platform profile")
            })
        },
        |value| {
            fs::write(&path, format!("{value}\n")).map_err(|e| {
                ControlError::new(
                    "profile_write_failed",
                    format!(
                        "{}: {e}; run with the required file permission (no automatic sudo)",
                        path.display()
                    ),
                )
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "mechrevo-core-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn put(root: &Path, path: &str, value: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, value).unwrap();
    }
    #[cfg(unix)]
    fn linked_hwmon(root: &Path, class_name: &str, interface: &str) -> PathBuf {
        use std::os::unix::fs::symlink;
        let physical = root.join("devices/pci0000:00/0000:00:02.0");
        let hwmon = physical.join("hwmon").join(interface);
        fs::create_dir_all(&hwmon).unwrap();
        fs::create_dir_all(root.join("class/hwmon")).unwrap();
        symlink(&physical, hwmon.join("device")).unwrap();
        symlink(&hwmon, root.join("class/hwmon").join(class_name)).unwrap();
        fs::write(hwmon.join("name"), "fixture_chip").unwrap();
        fs::write(hwmon.join("temp1_input"), "48000").unwrap();
        fs::write(hwmon.join("fan1_input"), "2400").unwrap();
        fs::canonicalize(hwmon).unwrap()
    }
    #[test]
    fn empty_machine_is_diagnostic_unknown_not_mock_zero() {
        let root = fixture();
        let s = snapshot(&root);
        assert!(s.identity.product_name.is_none());
        assert!(s.telemetry.is_empty());
        assert!(s
            .diagnostics
            .iter()
            .any(|d| d.code == "battery_unavailable"));
        assert_eq!(
            s.capabilities
                .iter()
                .find(|c| c.id == "power_profile")
                .unwrap()
                .status,
            CapabilityStatus::Unavailable
        );
        assert!(plan(&root, PowerProfile::Performance).is_err());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn sensors_keep_chip_identity_unit_and_missing_value() {
        let root = fixture();
        put(&root, "class/hwmon/hwmon0/name", "ec_driver");
        put(&root, "class/hwmon/hwmon0/temp1_input", "48000");
        put(&root, "class/hwmon/hwmon1/name", "amdgpu");
        put(&root, "class/hwmon/hwmon1/temp1_input", "71000");
        put(&root, "class/power_supply/BAT0/type", "Battery");
        put(&root, "class/power_supply/BAT0/capacity", "broken");
        put(&root, "class/power_supply/AC/type", "Mains");
        put(&root, "class/power_supply/AC/online", "1");
        let s = snapshot(&root);
        assert_eq!(s.telemetry.len(), 4);
        assert_eq!(s.telemetry[0].value, Some(48.));
        assert_eq!(s.telemetry[1].value, Some(71.));
        assert_ne!(s.telemetry[0].device_id, s.telemetry[1].device_id);
        assert!(s.telemetry[0].label.starts_with("ec_driver:"));
        assert!(!s.telemetry[0].label.contains("CPU"));
        assert_eq!(
            s.telemetry
                .iter()
                .find(|r| r.kind == "battery_capacity")
                .unwrap()
                .value,
            None
        );
        assert_eq!(
            s.telemetry
                .iter()
                .find(|r| r.kind == "external_power_online")
                .unwrap()
                .unit,
            "boolean"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn one_physical_device_keeps_same_channels_from_distinct_hwmon_interfaces() {
        let root = fixture();
        let first = linked_hwmon(&root, "hwmon0", "hwmon0");
        let second = linked_hwmon(&root, "hwmon1", "hwmon1");
        let s = snapshot(&root);
        assert_eq!(s.telemetry.len(), 4);
        let ids: HashSet<_> = s.telemetry.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(
            ids.len(),
            4,
            "same device/channel is not a unique sensor ID"
        );
        let physical = fs::canonicalize(root.join("devices/pci0000:00/0000:00:02.0"))
            .unwrap()
            .to_string_lossy()
            .into_owned();
        for reading in &s.telemetry {
            assert_eq!(reading.device_id, physical);
            assert_eq!(reading.source.device_id.as_deref(), Some(physical.as_str()));
            assert_eq!(
                reading.id,
                fs::canonicalize(&reading.source.path)
                    .unwrap()
                    .to_string_lossy()
            );
        }
        // Equal values do not establish that two interfaces are aliases.
        for (kind, value, unit) in [("temperature", 48.0, "°C"), ("fan_speed", 2400.0, "RPM")] {
            let readings: Vec<_> = s.telemetry.iter().filter(|r| r.kind == kind).collect();
            assert_eq!(readings.len(), 2);
            assert!(readings
                .iter()
                .all(|r| r.value == Some(value) && r.unit == unit));
            assert!(readings
                .iter()
                .any(|r| r.id.starts_with(first.to_str().unwrap())));
            assert!(readings
                .iter()
                .any(|r| r.id.starts_with(second.to_str().unwrap())));
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn aliases_of_one_canonical_hwmon_input_are_reported_once() {
        use std::os::unix::fs::symlink;
        let root = fixture();
        let hwmon = linked_hwmon(&root, "hwmon0", "hwmon7");
        symlink(&hwmon, root.join("class/hwmon/hwmon1")).unwrap();
        let s = snapshot(&root);
        assert_eq!(
            s.telemetry.len(),
            2,
            "class aliases must not duplicate sensors"
        );
        assert!(s
            .telemetry
            .iter()
            .all(|r| Path::new(&r.source.path).starts_with(root.join("class/hwmon/hwmon0"))));
        assert_eq!(
            s.diagnostics
                .iter()
                .filter(|d| d.code == "sensor_source_alias")
                .count(),
            2
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn sensor_identity_survives_invalid_values_and_runtime_suspend() {
        let root = fixture();
        let hwmon = linked_hwmon(&root, "hwmon0", "hwmon7");
        let active = snapshot(&root);
        fs::write(hwmon.join("temp1_input"), "").unwrap();
        fs::write(hwmon.join("fan1_input"), "invalid").unwrap();
        let invalid = snapshot(&root);
        let physical = root.join("devices/pci0000:00/0000:00:02.0");
        fs::create_dir_all(physical.join("power")).unwrap();
        fs::write(physical.join("power/runtime_status"), "suspended").unwrap();
        // A suspended input must not become available even if it holds a valid
        // cached value. The branch resolves its identity without reading it.
        fs::write(hwmon.join("temp1_input"), "71000").unwrap();
        fs::write(hwmon.join("fan1_input"), "3100").unwrap();
        let suspended = snapshot(&root);
        assert_eq!(active.telemetry.len(), 2);
        assert_eq!(invalid.telemetry.len(), 2);
        assert_eq!(suspended.telemetry.len(), 2);
        for ((active, invalid), suspended) in active
            .telemetry
            .iter()
            .zip(&invalid.telemetry)
            .zip(&suspended.telemetry)
        {
            assert_eq!(active.id, invalid.id);
            assert_eq!(active.id, suspended.id);
            assert_eq!(active.device_id, suspended.device_id);
            assert_eq!(active.source, invalid.source);
            assert_eq!(active.source, suspended.source);
            assert!(active.value.is_some());
            assert_eq!(invalid.value, None);
            assert_eq!(suspended.value, None);
            assert_eq!(suspended.status, CapabilityStatus::Unavailable);
            assert!(suspended.reason.as_deref().unwrap().contains("not read"));
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn suspended_fifo_input_is_not_opened() {
        use std::{
            os::unix::fs::FileTypeExt,
            process::{Command, Stdio},
            time::{Duration, Instant},
        };
        const CHILD_ROOT: &str = "MECHREVO_TEST_SUSPENDED_FIFO_ROOT";
        const COMPLETED: &str = "SUSPENDED_FIFO_SNAPSHOT_COMPLETED";
        if let Some(root) = std::env::var_os(CHILD_ROOT) {
            let root = PathBuf::from(root);
            for input in ["temp1_input", "fan1_input"] {
                assert!(fs::metadata(root.join("class/hwmon/hwmon0").join(input))
                    .unwrap()
                    .file_type()
                    .is_fifo());
            }
            // No writer opens either FIFO. Opening an input for reading would
            // block this child, rather than merely return a cached/invalid value.
            let s = snapshot(&root);
            assert_eq!(s.telemetry.len(), 2);
            for reading in s.telemetry {
                assert_eq!(reading.value, None);
                assert_eq!(reading.status, CapabilityStatus::Unavailable);
                assert!(reading.reason.as_deref().unwrap().contains("not read"));
                assert_eq!(
                    reading.id,
                    fs::canonicalize(&reading.source.path)
                        .unwrap()
                        .to_string_lossy()
                );
            }
            println!("{COMPLETED}");
            return;
        }
        let root = fixture();
        let hwmon = linked_hwmon(&root, "hwmon0", "hwmon7");
        for input in ["temp1_input", "fan1_input"] {
            fs::remove_file(hwmon.join(input)).unwrap();
        }
        let status = Command::new("mkfifo")
            .arg(hwmon.join("temp1_input"))
            .arg(hwmon.join("fan1_input"))
            .status()
            .expect("Linux FIFO regression requires the mkfifo utility");
        assert!(status.success(), "mkfifo failed: {status}");
        put(
            &root,
            "devices/pci0000:00/0000:00:02.0/power/runtime_status",
            "suspended",
        );
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "linux::tests::suspended_fifo_input_is_not_opened",
                "--nocapture",
            ])
            .env(CHILD_ROOT, &root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Cannot start the bounded FIFO regression child");
        let deadline = Instant::now() + Duration::from_secs(5);
        let completed = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) => break Err("suspended snapshot blocked on a FIFO for 5 seconds".into()),
                Err(error) => break Err(format!("Cannot wait for the FIFO child: {error}")),
            }
        };
        if completed.is_err() {
            let _ = child.kill();
        }
        let output = child.wait_with_output().unwrap();
        fs::remove_dir_all(root).unwrap();
        assert!(completed.is_ok(), "{completed:?}");
        assert!(
            output.status.success(),
            "FIFO child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(COMPLETED),
            "FIFO child did not run the exact regression: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    #[test]
    fn suspended_gpu_is_not_read_as_zero() {
        let root = fixture();
        put(&root, "class/hwmon/hwmon0/name", "amdgpu");
        put(&root, "class/hwmon/hwmon0/temp1_input", "70000");
        put(
            &root,
            "class/hwmon/hwmon0/device/power/runtime_status",
            "suspended",
        );
        let s = snapshot(&root);
        assert_eq!(s.telemetry[0].value, None);
        assert!(s.telemetry[0]
            .reason
            .as_ref()
            .unwrap()
            .contains("suspended"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn profile_enumeration_dry_plan_apply_readback_and_stale_rejection() {
        let root = fixture();
        put(&root, "firmware/acpi/platform_profile", "balanced");
        put(
            &root,
            "firmware/acpi/platform_profile_choices",
            "low-power balanced performance",
        );
        let p = plan(&root, PowerProfile::Performance).unwrap();
        assert_eq!(
            read(&root.join("firmware/acpi/platform_profile")).unwrap(),
            "balanced"
        );
        let r = apply(&root, &p).unwrap();
        assert_eq!(r.confirmed, "performance");
        assert_eq!(apply(&root, &p).unwrap_err().code, "stale_plan");
        put(&root, "firmware/acpi/platform_profile_choices", "balanced");
        assert!(plan(&root, PowerProfile::PowerSaver).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
