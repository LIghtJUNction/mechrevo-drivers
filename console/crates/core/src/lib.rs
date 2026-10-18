//! Evidence-backed platform inspection and operating-system power policy.
//! Brand identity never authorizes proprietary EC, fan, GPU or MUX writes.
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
pub mod brightness;
pub mod command;
pub mod cpu_power;
pub mod fan_curve;
pub mod oem;
pub mod power_profiles_daemon;
pub mod privilege;
pub mod windows_oem;
pub use brightness::{BrightnessApplyResult, BrightnessDevice, BrightnessPlan};
pub use oem::{OemApplyResult, OemControl, OemControlKind, OemDomain, OemPlan, OemValue};
pub use power_profiles_daemon::{ProfileApplyReceipt, ProfileDetails};
#[cfg(any(target_os = "linux", test))]
mod linux;
pub mod registry;
#[cfg(any(target_os = "windows", test))]
mod windows;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    Available,
    Unsupported,
    Unavailable,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceSource {
    pub backend: String,
    pub path: String,
    pub device_id: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Identity {
    pub manufacturer: Option<String>,
    pub product_name: Option<String>,
    pub product_version: Option<String>,
    pub board_name: Option<String>,
    pub bios_version: Option<String>,
    pub mechrevo_brand_detected: bool,
    pub support_note: String,
}
impl Identity {
    pub fn qualify(&mut self) {
        self.mechrevo_brand_detected = [&self.manufacturer, &self.product_name]
            .iter()
            .filter_map(|v| v.as_ref())
            .any(|v| {
                let v = v.to_lowercase();
                v.contains("mechrevo") || v.contains("机械革命")
            });
        self.support_note = "DMI brand detection is identity only, not OEM hardware support. Individual controls require an exposed, bound driver interface; model-wide compatibility is not inferred.".into();
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorReading {
    pub id: String,
    pub label: String,
    pub device_id: String,
    pub kind: String,
    pub value: Option<f64>,
    pub unit: String,
    pub source: EvidenceSource,
    pub status: CapabilityStatus,
    pub reason: Option<String>,
}
/// One read-only temperature/fan collection interval on this process's monotonic
/// clock. The interval bounds our reads; it does not establish firmware freshness
/// or mean that sensors were read atomically.
#[derive(Debug, Clone)]
pub struct ThermalSample {
    pub readings: Vec<SensorReading>,
    pub diagnostics: Vec<Diagnostic>,
    pub started_at: std::time::Instant,
    pub finished_at: std::time::Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub id: String,
    pub label: String,
    pub status: CapabilityStatus,
    pub reason: String,
    pub source: Option<EvidenceSource>,
    pub options: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PowerPolicy {
    pub scope: String,
    pub current: Option<String>,
    pub options: Vec<String>,
    pub source: Option<EvidenceSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ProfileDetails>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub platform: String,
    pub identity: Identity,
    pub telemetry: Vec<SensorReading>,
    pub capabilities: Vec<Capability>,
    pub power_policy: PowerPolicy,
    pub diagnostics: Vec<Diagnostic>,
    pub brightness: Vec<BrightnessDevice>,
    #[serde(default)]
    pub oem_controls: Vec<OemControl>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PowerProfile {
    Balanced,
    Performance,
    PowerSaver,
}
impl fmt::Display for PowerProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Balanced => "balanced",
            Self::Performance => "performance",
            Self::PowerSaver => "power-saver",
        })
    }
}
impl FromStr for PowerProfile {
    type Err = ControlError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "balanced" => Ok(Self::Balanced),
            "performance" => Ok(Self::Performance),
            "power-saver" => Ok(Self::PowerSaver),
            _ => Err(ControlError::new(
                "invalid_profile",
                "Expected balanced, performance or power-saver",
            )),
        }
    }
}
fn profile_target(platform: &str, profile: PowerProfile) -> Option<&'static str> {
    match (platform, profile) {
        ("linux", PowerProfile::Balanced) => Some("balanced"),
        ("linux", PowerProfile::Performance) => Some("performance"),
        ("linux", PowerProfile::PowerSaver) => Some("low-power"),
        ("windows", PowerProfile::Balanced) => Some("381b4222-f694-41f0-9685-ff5bb260df2e"),
        ("windows", PowerProfile::Performance) => Some("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c"),
        ("windows", PowerProfile::PowerSaver) => Some("a1841308-3541-4fab-bc81-f71556f20b4a"),
        _ => None,
    }
}
/// Evaluate an existing snapshot without any further hardware query or process.
pub fn profile_available(snapshot: &Snapshot, profile: PowerProfile) -> bool {
    snapshot
        .power_policy
        .current
        .as_ref()
        .is_some_and(|current| snapshot.power_policy.options.contains(current))
        && profile_target_for_source(
            &snapshot.platform,
            snapshot.power_policy.source.as_ref(),
            profile,
        )
        .is_some_and(|target| {
            snapshot
                .power_policy
                .options
                .iter()
                .any(|option| option == target)
        })
}
fn profile_target_for_source(
    platform: &str,
    source: Option<&EvidenceSource>,
    profile: PowerProfile,
) -> Option<&'static str> {
    if platform == "linux"
        && source.is_some_and(|source| source.backend == "linux_power_profiles_daemon")
        && profile == PowerProfile::PowerSaver
    {
        Some("power-saver")
    } else {
        profile_target(platform, profile)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfilePlan {
    pub requested: PowerProfile,
    pub target: String,
    pub previous: String,
    pub scope: String,
    pub source: EvidenceSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ProfileDetails>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyResult {
    pub requested: PowerProfile,
    pub confirmed: String,
    pub previous: String,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ProfileApplyReceipt>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlError {
    pub code: String,
    pub message: String,
    pub details: Vec<String>,
}
impl ControlError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: vec![],
        }
    }
}
impl fmt::Display for ControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for ControlError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogModel {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub category: Option<String>,
    pub series: Option<String>,
    pub entry_kind: String,
    pub url: String,
    pub evidence: Vec<String>,
    pub support_note: String,
}
#[derive(Deserialize)]
struct BundledCatalog {
    schema_version: u32,
    source: CatalogSource,
    entries: Vec<CatalogEntry>,
}
#[derive(Deserialize)]
struct CatalogSource {
    url: String,
    total: usize,
}
#[derive(Deserialize)]
struct CatalogEntry {
    id: String,
    name: String,
    slug: String,
    category: Option<String>,
    series: Option<String>,
    entry_kind: String,
    url: String,
}
fn parse_catalog(input: &str) -> Result<BundledCatalog, ControlError> {
    let data: BundledCatalog = serde_json::from_str(input)
        .map_err(|e| ControlError::new("catalog_invalid", e.to_string()))?;
    if data.schema_version != 1
        || data.entries.len() != data.source.total
        || !data.source.url.starts_with("https://")
    {
        return Err(ControlError::new(
            "catalog_invalid",
            "Catalog schema, total or source is invalid",
        ));
    }
    let mut ids = std::collections::HashSet::new();
    if data.entries.iter().any(|e| {
        e.id.is_empty() || e.name.is_empty() || !e.url.starts_with("https://") || !ids.insert(&e.id)
    }) {
        return Err(ControlError::new(
            "catalog_invalid",
            "Catalog IDs/names/URLs must be valid and unique",
        ));
    }
    Ok(data)
}
#[derive(Debug)]
struct CatalogSearchEntry {
    model: CatalogModel,
    search_text: String,
}
static BUNDLED_CATALOG: std::sync::OnceLock<Result<Vec<CatalogSearchEntry>, ControlError>> =
    std::sync::OnceLock::new();

fn catalog_index(input: &str) -> Result<Vec<CatalogSearchEntry>, ControlError> {
    let data = parse_catalog(input)?;
    Ok(data.entries.into_iter().map(|v| {
        let search_text = format!("{} {} {} {} {}", v.id, v.name, v.slug,
            v.series.as_deref().unwrap_or(""), v.category.as_deref().unwrap_or("")).to_lowercase();
        CatalogSearchEntry {
            search_text,
            model: CatalogModel {id:v.id,name:v.name,slug:v.slug,category:v.category,series:v.series,
                entry_kind:v.entry_kind,evidence:vec![v.url.clone(),data.source.url.clone()],url:v.url,
                support_note:"Official product listing only; Windows/Linux hardware compatibility is not established by this entry.".into()},
        }
    }).collect())
}

fn cached_catalog<'a>(
    cache: &'a std::sync::OnceLock<Result<Vec<CatalogSearchEntry>, ControlError>>,
    input: &str,
) -> Result<&'a [CatalogSearchEntry], &'a ControlError> {
    cache
        .get_or_init(|| catalog_index(input))
        .as_ref()
        .map(Vec::as_slice)
}

/// Marketing inventory is intentionally separate from tested support.
// @mark mechrevo.model_catalog role=implementation
pub fn catalog_search(query: &str) -> Vec<CatalogModel> {
    // Cache the immutable parse result, including a real failure. Preserve the
    // existing fail-closed Vec API without turning invalid data into an Ok cache.
    let Ok(index) = cached_catalog(
        &BUNDLED_CATALOG,
        include_str!("../../../../data/mechrevo-catalog.json"),
    ) else {
        return vec![];
    };
    let query = query.to_lowercase();
    index
        .iter()
        .filter(|entry| entry.search_text.contains(&query))
        .map(|entry| entry.model.clone())
        .collect()
}

/// Blocking backend: call from a GUI worker, never its event/render thread.
/// Constructing a service does no hardware I/O; snapshots do not write hardware.
#[derive(Debug, Clone)]
pub struct Service {
    linux_root: std::path::PathBuf,
}
impl Default for Service {
    fn default() -> Self {
        Self::new()
    }
}
impl Service {
    pub fn new() -> Self {
        Self {
            linux_root: "/sys".into(),
        }
    }
    #[doc(hidden)]
    pub fn with_linux_sysfs(root: impl Into<std::path::PathBuf>) -> Self {
        Self {
            linux_root: root.into(),
        }
    }
    pub fn catalog(&self, query: &str) -> Vec<CatalogModel> {
        catalog_search(query)
    }
    /// Read only hwmon temperatures and fan speeds. This blocking call belongs
    /// on a sampling worker, including while a fan-control lease is held.
    /// It never discovers controls or launches an external command/helper.
    pub fn sample_thermal(&self) -> Result<ThermalSample, ControlError> {
        #[cfg(target_os = "linux")]
        {
            linux::sample_thermal(&self.linux_root)
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(ControlError::new(
                "thermal_sampling_unavailable",
                "Dedicated thermal sampling is only implemented for Linux hwmon",
            ))
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        #[cfg(target_os = "linux")]
        let mut snapshot = linux::snapshot(&self.linux_root);
        #[cfg(target_os = "windows")]
        let mut snapshot = windows::snapshot();
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let mut snapshot = unsupported_snapshot();
        snapshot.brightness = self.brightness_devices();
        snapshot.oem_controls = self.oem_controls();
        for control in &snapshot.oem_controls {
            snapshot.capabilities.push(Capability {
                id: control.id.clone(),
                label: control.label.clone(),
                status: control.status.clone(),
                reason: control
                    .reason
                    .clone()
                    .unwrap_or_else(|| control.readback_scope.clone()),
                source: Some(control.source.clone()),
                options: match &control.domain {
                    OemDomain::Boolean => vec!["off".into(), "on".into()],
                    OemDomain::Choices(choices) => choices.clone(),
                    _ => vec![],
                },
            });
        }
        let available = snapshot
            .brightness
            .iter()
            .any(|device| device.status == CapabilityStatus::Available && device.can_control());
        snapshot.capabilities.push(Capability {
            id: "brightness".into(),
            label: "OS screen brightness".into(),
            status: if available {
                CapabilityStatus::Available
            } else {
                CapabilityStatus::Unavailable
            },
            reason: if available {
                "Enumerated OS brightness control route; writes require a freshly validated plan and control-value readback."
            } else {
                "No usable OS brightness control route was identified; no OEM or EC fallback is attempted."
            }
            .into(),
            source: None,
            options: snapshot
                .brightness
                .iter()
                .filter(|device| device.can_control())
                .map(|device| device.id.clone())
                .collect(),
        });
        snapshot
    }
    pub fn brightness_devices(&self) -> Vec<BrightnessDevice> {
        brightness::devices(&self.linux_root)
    }
    pub fn plan_brightness(
        &self,
        percent: u8,
        device: Option<&str>,
    ) -> Result<BrightnessPlan, ControlError> {
        brightness::plan(&self.linux_root, percent, device)
    }
    pub fn apply_brightness(
        &self,
        plan: &BrightnessPlan,
    ) -> Result<BrightnessApplyResult, ControlError> {
        brightness::apply(&self.linux_root, plan)
    }
    /// Discover only controls actually exposed by the loaded platform driver.
    pub fn oem_controls(&self) -> Vec<OemControl> {
        oem::controls(&self.linux_root)
    }
    /// Read-only preview; a readable control can require separate authorization.
    pub fn plan_oem(&self, id: &str, target: OemValue) -> Result<OemPlan, ControlError> {
        oem::plan(&self.linux_root, id, target)
    }
    /// Direct backend operation. This method never launches a privileged helper.
    pub fn apply_oem(&self, plan: &OemPlan) -> Result<OemApplyResult, ControlError> {
        oem::apply(&self.linux_root, plan)
    }
    pub fn capabilities(&self) -> Vec<Capability> {
        self.snapshot().capabilities
    }
    // @mark mechrevo.os_power_policy role=interface
    pub fn plan_profile(&self, profile: PowerProfile) -> Result<ProfilePlan, ControlError> {
        #[cfg(target_os = "linux")]
        {
            linux::plan(&self.linux_root, profile)
        }
        #[cfg(target_os = "windows")]
        {
            windows::plan(profile)
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = profile;
            Err(ControlError::new(
                "unsupported_platform",
                "Only Windows and Linux backends are implemented",
            ))
        }
    }
    pub fn apply_profile(&self, plan: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        #[cfg(target_os = "linux")]
        {
            linux::apply(&self.linux_root, plan)
        }
        #[cfg(target_os = "windows")]
        {
            windows::apply(plan)
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            let _ = plan;
            Err(ControlError::new(
                "unsupported_platform",
                "Only Windows and Linux backends are implemented",
            ))
        }
    }
}
/// Restore only a still-observed value belonging to this request. Foreign or
/// unreadable profiles are preserved; matching-value ABA is not excluded.
// @mark mechrevo.os_power_policy role=implementation
fn apply_verified<R, W>(
    plan: &ProfilePlan,
    mut read: R,
    mut write: W,
) -> Result<ApplyResult, ControlError>
where
    R: FnMut() -> Result<String, ControlError>,
    W: FnMut(&str) -> Result<(), ControlError>,
{
    if read()? != plan.previous {
        return Err(ControlError::new(
            "stale_plan",
            "Current profile changed before write",
        ));
    }
    if plan.target == plan.previous {
        let confirmed = read()?;
        if confirmed != plan.previous {
            return Err(ControlError::new(
                "stale_plan",
                "Current profile changed during the no-op check",
            ));
        }
        return Ok(ApplyResult {
            requested: plan.requested,
            confirmed,
            previous: plan.previous.clone(),
            scope: plan.scope.clone(),
            receipt: None,
        });
    }
    let attempt = write(&plan.target)
        .and_then(|()| read())
        .and_then(|actual| {
            if actual == plan.target {
                Ok(actual)
            } else {
                Err(ControlError::new(
                    "readback_mismatch",
                    format!("Requested {}, read back {actual}", plan.target),
                ))
            }
        });
    match attempt {
        Ok(confirmed) => Ok(ApplyResult {
            requested: plan.requested,
            confirmed,
            previous: plan.previous.clone(),
            scope: plan.scope.clone(),
            receipt: None,
        }),
        Err(mut error) => {
            // A failed call or mismatched readback does not give us ownership
            // of another application's profile. Never overwrite it to make
            // the old value appear restored.
            let current = match read() {
                Ok(current) => current,
                Err(observation) => {
                    error.details.push(format!(
                        "restore_unknown: {observation}; no recovery write attempted"
                    ));
                    return Err(error);
                }
            };
            if current == plan.previous {
                error.details.push(format!(
                    "restore_not_needed: previous profile observed, {}",
                    plan.previous
                ));
                return Err(error);
            }
            if current != plan.target {
                error
                    .details
                    .push(format!("restore_skipped_foreign_profile: {current}"));
                return Err(error);
            }
            // Recheck immediately before recovery. There is no OS-level CAS:
            // another writer can still race after this final observation.
            match read() {
                Ok(current) if current == plan.target => {}
                Ok(current) => {
                    error
                        .details
                        .push(format!("restore_skipped_changed_profile: {current}"));
                    return Err(error);
                }
                Err(observation) => {
                    error.details.push(format!(
                        "restore_unknown: {observation}; no recovery write attempted"
                    ));
                    return Err(error);
                }
            }
            match write(&plan.previous).and_then(|()| read()) {
                Ok(actual) if actual == plan.previous => error
                    .details
                    .push(format!("restore_confirmed: {}", plan.previous)),
                Ok(actual) => error.details.push(format!(
                    "restore_unconfirmed: expected {}, read {actual}",
                    plan.previous
                )),
                Err(restore) => error.details.push(format!("restore_unknown: {restore}")),
            }
            Err(error)
        }
    }
}
fn proprietary_capabilities() -> Vec<Capability> {
    [("oem_performance","OEM performance mode"),("fan_control","OEM fan control"),("gpu_control","GPU tuning"),("mux_control","Display MUX"),("battery_charge_limit","Battery charge limit")].into_iter().map(|(id,label)|Capability{id:id.into(),label:label.into(),status:CapabilityStatus::Unsupported,reason:"No verified backend for this feature. Other driver-exposed controls, including charging mode rather than charge thresholds, are reported separately.".into(),source:None,options:vec![]}).collect()
}
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn unsupported_snapshot() -> Snapshot {
    Snapshot {
        platform: std::env::consts::OS.into(),
        identity: Identity::default(),
        telemetry: vec![],
        capabilities: proprietary_capabilities(),
        power_policy: PowerPolicy::default(),
        brightness: vec![],
        oem_controls: vec![],
        diagnostics: vec![Diagnostic {
            code: "unsupported_platform".into(),
            message: "Only Windows and Linux backends are implemented".into(),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_catalog_schema_is_validated() {
        let raw = include_str!("../../../../data/mechrevo-catalog.json");
        assert!(parse_catalog(raw).is_ok());
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        let mut unsupported = v.clone();
        unsupported["schema_version"] = 2.into();
        assert!(parse_catalog(&unsupported.to_string()).is_err());
        let mut wrong_total = v.clone();
        wrong_total["source"]["total"] = 0.into();
        assert!(parse_catalog(&wrong_total.to_string()).is_err());
    }
    #[test]
    fn immutable_catalog_cache_reuses_the_validated_index() {
        let cache = std::sync::OnceLock::new();
        let first = cached_catalog(
            &cache,
            include_str!("../../../../data/mechrevo-catalog.json"),
        )
        .unwrap();
        assert!(!first.is_empty());
        let address = first.as_ptr();
        let second = cached_catalog(&cache, "not another parse").unwrap();
        assert_eq!(address, second.as_ptr());
        assert_eq!(first.len(), second.len());
    }
    #[test]
    fn invalid_catalog_cache_retains_the_error_instead_of_a_successful_empty_list() {
        let cache = std::sync::OnceLock::new();
        let first = cached_catalog(&cache, "{}").unwrap_err();
        assert_eq!(first.code, "catalog_invalid");
        let second = cached_catalog(
            &cache,
            include_str!("../../../../data/mechrevo-catalog.json"),
        )
        .unwrap_err();
        assert!(std::ptr::eq(first, second));
        assert!(cache.get().unwrap().is_err());
    }
    #[test]
    fn profile_failure_preserves_foreign_state_and_does_not_claim_a_restore() {
        use std::cell::RefCell;
        let state = RefCell::new("balanced".to_string());
        let p = ProfilePlan {
            requested: PowerProfile::Performance,
            target: "performance".into(),
            previous: "balanced".into(),
            scope: "test".into(),
            source: EvidenceSource {
                backend: "test".into(),
                path: "test".into(),
                device_id: None,
            },
            details: None,
        };
        let r = apply_verified(
            &p,
            || Ok(state.borrow().clone()),
            |value| {
                *state.borrow_mut() = if value == "performance" {
                    "refused"
                } else {
                    value
                }
                .into();
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(r.code, "readback_mismatch");
        assert_eq!(state.borrow().as_str(), "refused");
        assert!(r
            .details
            .iter()
            .any(|d| d.starts_with("restore_skipped_foreign_profile")));
        let r = apply_verified(
            &p,
            || Ok("balanced".into()),
            |_| Err(ControlError::new("io", "write failed")),
        )
        .unwrap_err();
        assert!(r
            .details
            .iter()
            .any(|d| d.starts_with("restore_not_needed")));
    }
    fn os_profile_test_plan() -> ProfilePlan {
        ProfilePlan {
            requested: PowerProfile::Performance,
            previous: "balanced".into(),
            target: "performance".into(),
            scope: "offline OS profile transaction".into(),
            source: EvidenceSource {
                backend: "test".into(),
                path: "test".into(),
                device_id: None,
            },
            details: None,
        }
    }
    #[test]
    fn os_profile_failed_call_landed_target_is_conditionally_restored() {
        use std::cell::RefCell;
        let state = RefCell::new("balanced".to_string());
        let writes = RefCell::new(Vec::new());
        let error = apply_verified(
            &os_profile_test_plan(),
            || Ok(state.borrow().clone()),
            |value| {
                writes.borrow_mut().push(value.to_owned());
                *state.borrow_mut() = value.to_owned();
                if value == "performance" {
                    Err(ControlError::new("write_failed", "failed after submission"))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "write_failed");
        assert_eq!(&*writes.borrow(), &["performance", "balanced"]);
        assert_eq!(&*state.borrow(), "balanced");
        assert!(error
            .details
            .iter()
            .any(|detail| detail.starts_with("restore_confirmed:")));
    }
    #[test]
    fn os_profile_third_party_change_before_recovery_is_preserved() {
        use std::cell::RefCell;
        use std::collections::VecDeque;
        let observations = RefCell::new(VecDeque::from(["balanced", "performance", "power-saver"]));
        let writes = RefCell::new(Vec::new());
        let error = apply_verified(
            &os_profile_test_plan(),
            || Ok(observations.borrow_mut().pop_front().unwrap().into()),
            |value| {
                writes.borrow_mut().push(value.to_owned());
                Err(ControlError::new("write_failed", "failed after submission"))
            },
        )
        .unwrap_err();
        assert_eq!(&*writes.borrow(), &["performance"]);
        assert!(error
            .details
            .iter()
            .any(|detail| detail == "restore_skipped_changed_profile: power-saver"));
    }
    #[test]
    fn os_profile_unknown_state_never_causes_a_recovery_write() {
        use std::cell::{Cell, RefCell};
        let reads = Cell::new(0);
        let writes = RefCell::new(Vec::new());
        let error = apply_verified(
            &os_profile_test_plan(),
            || {
                reads.set(reads.get() + 1);
                if reads.get() == 1 {
                    Ok("balanced".into())
                } else {
                    Err(ControlError::new("read_failed", "unavailable"))
                }
            },
            |value| {
                writes.borrow_mut().push(value.to_owned());
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "read_failed");
        assert_eq!(&*writes.borrow(), &["performance"]);
        assert!(error
            .details
            .iter()
            .any(|detail| detail.starts_with("restore_unknown:")));
    }
    #[test]
    fn os_profile_noop_rechecks_without_any_setter() {
        use std::cell::Cell;
        let writes = Cell::new(0);
        let mut plan = os_profile_test_plan();
        plan.target = plan.previous.clone();
        plan.requested = PowerProfile::Balanced;
        let result = apply_verified(
            &plan,
            || Ok("balanced".into()),
            |_| {
                writes.set(writes.get() + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(result.confirmed, "balanced");
        assert_eq!(writes.get(), 0);
        let reads = Cell::new(0);
        let error = apply_verified(
            &plan,
            || {
                reads.set(reads.get() + 1);
                Ok(if reads.get() == 1 {
                    "balanced"
                } else {
                    "performance"
                }
                .into())
            },
            |_| {
                writes.set(writes.get() + 1);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error.code, "stale_plan");
        assert_eq!(writes.get(), 0);
    }
    #[test]
    fn os_profile_failed_restore_never_claims_success_even_if_it_landed() {
        use std::cell::RefCell;
        let state = RefCell::new("balanced".to_string());
        let error = apply_verified(
            &os_profile_test_plan(),
            || Ok(state.borrow().clone()),
            |value| {
                *state.borrow_mut() = value.to_owned();
                Err(ControlError::new("write_failed", "failed after submission"))
            },
        )
        .unwrap_err();
        assert_eq!(&*state.borrow(), "balanced");
        assert!(error
            .details
            .iter()
            .any(|detail| detail.starts_with("restore_unknown:")));
        assert!(!error
            .details
            .iter()
            .any(|detail| detail.starts_with("restore_confirmed:")));
    }
    #[test]
    fn supported_profile_checks_are_platform_specific_and_require_known_current() {
        let mut snapshot = Snapshot {
            platform: "linux".into(),
            identity: Identity::default(),
            telemetry: vec![],
            capabilities: vec![],
            power_policy: PowerPolicy {
                scope: "test".into(),
                current: Some("balanced".into()),
                options: vec!["balanced".into(), "performance".into()],
                source: None,
                details: None,
            },
            diagnostics: vec![],
            brightness: vec![],
            oem_controls: vec![],
        };
        assert!(profile_available(&snapshot, PowerProfile::Performance));
        assert!(!profile_available(&snapshot, PowerProfile::PowerSaver));
        snapshot.power_policy.options = vec!["balanced".into(), "power-saver".into()];
        snapshot.power_policy.source = Some(EvidenceSource {
            backend: "linux_power_profiles_daemon".into(),
            path: "system-bus".into(),
            device_id: None,
        });
        assert!(profile_available(&snapshot, PowerProfile::PowerSaver));
        snapshot.power_policy.source.as_mut().unwrap().backend = "linux_sysfs".into();
        assert!(!profile_available(&snapshot, PowerProfile::PowerSaver));
        snapshot.platform = "windows".into();
        assert!(!profile_available(&snapshot, PowerProfile::Balanced));
        let guid = profile_target("windows", PowerProfile::PowerSaver)
            .unwrap()
            .to_string();
        snapshot.power_policy.current = Some(guid.clone());
        snapshot.power_policy.options = vec![guid];
        assert!(profile_available(&snapshot, PowerProfile::PowerSaver));
        snapshot.power_policy.current = None;
        assert!(!profile_available(&snapshot, PowerProfile::PowerSaver));
    }
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn dedicated_thermal_sampling_is_explicitly_unavailable_off_linux() {
        assert_eq!(
            Service::new().sample_thermal().unwrap_err().code,
            "thermal_sampling_unavailable"
        );
    }
    #[test]
    fn profiles_are_explicit() {
        assert!("silent".parse::<PowerProfile>().is_err());
        assert_eq!(
            "power-saver".parse::<PowerProfile>().unwrap(),
            PowerProfile::PowerSaver
        );
    }
    #[test]
    fn brand_detection_does_not_grant_support() {
        let mut id = Identity {
            manufacturer: Some("MECHREVO".into()),
            ..Default::default()
        };
        id.qualify();
        assert!(id.mechrevo_brand_detected);
        assert!(proprietary_capabilities()
            .iter()
            .all(|c| c.status == CapabilityStatus::Unsupported));
    }
    #[test]
    fn catalog_search_is_unicode_and_case_insensitive() {
        let all = catalog_search("");
        assert!(!all.is_empty());
        let first = &all[0];
        assert!(catalog_search(&first.id.to_uppercase())
            .iter()
            .any(|m| m.id == first.id));
        assert!(all
            .iter()
            .all(|m| m.evidence.iter().all(|u| u.starts_with("https://"))));
        assert!(catalog_search("definitely-nonexistent-device-928381").is_empty());
    }
    #[test]
    fn no_serial_fields_in_identity_json() {
        let value = serde_json::to_string(&Identity::default()).unwrap();
        assert!(!value.contains("serial"));
        assert!(!value.contains("uuid"));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod thermal_tests;
