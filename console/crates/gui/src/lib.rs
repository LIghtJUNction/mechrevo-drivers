//! The UI never performs blocking platform calls or speculative hardware writes.
//! A single worker and a one-item channel bound refresh requests and results.
use mechrevo_console_core::brightness::{
    self, BrightnessOutcome, BrightnessRecovery, BrightnessTransaction,
};
use mechrevo_console_core::cpu_power::{
    self, Action as CpuAction, Client as CpuPowerClient, CpuPowerUpdate,
    Management as CpuManagement, Outcome as CpuOutcome, Plan as CpuPlan, Policy as CpuPolicy,
    Status as CpuStatus,
};
use mechrevo_console_core::fan_curve::{
    broker::{BrokerPhase, BrokerState},
    client::{FanBrokerClient, FanClientUpdate},
};
use mechrevo_console_core::fan_curve::{FanCurve, ValidatedFanCurve};
use mechrevo_console_core::windows_oem::{
    self, Client as WindowsOemClient, ClientHelperLifetime as WindowsFnLifetime,
    Outcome as WindowsFnOutcome, Receipt as WindowsFnReceipt, Recovery as WindowsFnRecovery,
    Status as WindowsFnStatus, Update as WindowsFnUpdate,
};
use mechrevo_console_core::{
    ApplyResult, BrightnessApplyResult, BrightnessPlan, CapabilityStatus, CatalogModel,
    ControlError, OemApplyResult, OemControl, OemDomain, OemPlan, OemValue, PowerProfile,
    ProfilePlan, SensorReading, Service, Snapshot, ThermalSample,
};
use serde::{Deserialize, Serialize};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError},
        Arc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[cfg(feature = "gui")]
mod appearance;
mod cpu_power_controller;
#[cfg(feature = "gui")]
pub mod desktop;
#[cfg(feature = "gui")]
pub mod fan_editor;
mod windows_fn_controller;

pub trait Backend: Send + 'static {
    fn snapshot(&mut self) -> Snapshot;
    /// Narrow read-only temperature/RPM provider. Never fall back to a full
    /// snapshot: that can enumerate OEM controls while a fan lease is held.
    fn sample_thermal(&mut self) -> Result<ThermalSample, ControlError> {
        Err(ControlError::new(
            "thermal_unavailable",
            "此后端没有独立的温度与转速采样接口。",
        ))
    }
    fn catalog(&mut self) -> Vec<CatalogModel>;
    fn plan(&mut self, profile: PowerProfile) -> Result<ProfilePlan, ControlError>;
    fn apply(&mut self, plan: &ProfilePlan) -> Result<ApplyResult, ControlError>;
    fn plan_brightness(
        &mut self,
        _percent: u8,
        _device: &str,
    ) -> Result<BrightnessPlan, ControlError> {
        Err(ControlError::new(
            "brightness_unavailable",
            "No brightness backend is available",
        ))
    }
    fn apply_brightness(
        &mut self,
        _plan: &BrightnessPlan,
    ) -> Result<BrightnessApplyResult, ControlError> {
        Err(ControlError::new(
            "brightness_unavailable",
            "No brightness backend is available",
        ))
    }
    fn plan_oem(&mut self, _control: &str, _target: OemValue) -> Result<OemPlan, ControlError> {
        Err(ControlError::new(
            "oem_unavailable",
            "No OEM control backend is available",
        ))
    }
    fn apply_oem(&mut self, _plan: &OemPlan) -> Result<OemApplyResult, ControlError> {
        Err(ControlError::new(
            "oem_unavailable",
            "No OEM control backend is available",
        ))
    }
    /// CPU authorization is opt-in and never falls back to discovery or an OS profile.
    fn cpu_power_probe(&mut self) -> CpuPowerUpdate {
        unavailable_cpu_power()
    }
    fn cpu_power_execute(&mut self, _plan: &CpuPlan) -> CpuPowerUpdate {
        unavailable_cpu_power()
    }
    /// Explicit Windows-only authorization; never used by snapshot or sampling.
    fn windows_fn_probe(&mut self) -> WindowsFnUpdate {
        unavailable_windows_fn()
    }
    fn windows_fn_apply(&mut self, _plan: &OemPlan) -> WindowsFnUpdate {
        unavailable_windows_fn()
    }
    /// Drain/wait only the existing helper. This must never spawn or replay.
    fn windows_fn_finish_pending(&mut self) -> WindowsFnUpdate {
        unavailable_windows_fn()
    }
    fn fan_probe(&mut self) -> FanClientUpdate {
        unavailable_fan()
    }
    fn fan_refresh(&mut self) -> FanClientUpdate {
        unavailable_fan()
    }
    fn fan_apply(&mut self, _curve: &ValidatedFanCurve) -> FanClientUpdate {
        unavailable_fan()
    }
    fn fan_restore(&mut self) -> FanClientUpdate {
        unavailable_fan()
    }
    /// Legacy/mock backends never started a fan helper, so their shutdown is
    /// already clean. A real session backend must override this method.
    fn fan_shutdown(&mut self) -> FanClientUpdate {
        FanClientUpdate::default()
    }
    /// Cached lifecycle/reaper updates only. No EC reads or authorization.
    fn fan_poll(&mut self) -> Option<FanClientUpdate> {
        None
    }
}

fn unavailable_cpu_power() -> CpuPowerUpdate {
    CpuPowerUpdate {
        status: None,
        outcome: CpuOutcome::RejectedBeforeWrite,
        error: Some(ControlError::new(
            "cpu_power_unsupported",
            "此后端没有提供已核验的 CPU 长期功耗策略接口。",
        )),
        receipt: None,
        helper_reaped: true,
    }
}

fn unavailable_fan() -> FanClientUpdate {
    FanClientUpdate {
        error: Some(ControlError::new(
            "fan_backend_unsupported",
            "此后端未提供风扇曲线会话。",
        )),
        ..FanClientUpdate::default()
    }
}

fn unavailable_windows_fn() -> WindowsFnUpdate {
    WindowsFnUpdate {
        status: None,
        outcome: WindowsFnOutcome::RejectedBeforeWrite,
        result: None,
        receipt: WindowsFnReceipt::default(),
        error: Some(ControlError::new(
            "windows_fn_unavailable",
            "此后端未提供 Windows Fn 锁授权接口。",
        )),
        helper_reaped: true,
        helper_lifetime: WindowsFnLifetime::NotStarted,
    }
}

/// One service and one persistent broker client belong to the worker. Merely
/// constructing this backend never launches authentication or claims a lease.
pub struct NativeBackend {
    service: Service,
    fan: FanBrokerClient,
    cpu_power: CpuPowerClient,
    windows_oem: WindowsOemClient,
}
impl Default for NativeBackend {
    fn default() -> Self {
        Self::new()
    }
}
impl NativeBackend {
    pub fn new() -> Self {
        Self {
            service: Service::new(),
            fan: FanBrokerClient::new(),
            cpu_power: CpuPowerClient::new(),
            windows_oem: WindowsOemClient::new(),
        }
    }
}
impl Backend for NativeBackend {
    fn snapshot(&mut self) -> Snapshot {
        Backend::snapshot(&mut self.service)
    }
    fn sample_thermal(&mut self) -> Result<ThermalSample, ControlError> {
        self.service.sample_thermal()
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        Backend::catalog(&mut self.service)
    }
    fn plan(&mut self, profile: PowerProfile) -> Result<ProfilePlan, ControlError> {
        Backend::plan(&mut self.service, profile)
    }
    fn apply(&mut self, plan: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        Backend::apply(&mut self.service, plan)
    }
    fn plan_brightness(
        &mut self,
        percent: u8,
        device: &str,
    ) -> Result<BrightnessPlan, ControlError> {
        Backend::plan_brightness(&mut self.service, percent, device)
    }
    fn apply_brightness(
        &mut self,
        plan: &BrightnessPlan,
    ) -> Result<BrightnessApplyResult, ControlError> {
        Backend::apply_brightness(&mut self.service, plan)
    }
    fn plan_oem(&mut self, control: &str, target: OemValue) -> Result<OemPlan, ControlError> {
        Backend::plan_oem(&mut self.service, control, target)
    }
    fn apply_oem(&mut self, plan: &OemPlan) -> Result<OemApplyResult, ControlError> {
        Backend::apply_oem(&mut self.service, plan)
    }
    fn cpu_power_probe(&mut self) -> CpuPowerUpdate {
        self.cpu_power.probe_authorized()
    }
    fn cpu_power_execute(&mut self, plan: &CpuPlan) -> CpuPowerUpdate {
        self.cpu_power.execute_authorized(plan)
    }
    fn windows_fn_probe(&mut self) -> WindowsFnUpdate {
        self.windows_oem.probe_authorized()
    }
    fn windows_fn_apply(&mut self, plan: &OemPlan) -> WindowsFnUpdate {
        self.windows_oem.apply_authorized(plan)
    }
    fn windows_fn_finish_pending(&mut self) -> WindowsFnUpdate {
        self.windows_oem.finish_pending()
    }
    fn fan_probe(&mut self) -> FanClientUpdate {
        self.fan.probe()
    }
    fn fan_refresh(&mut self) -> FanClientUpdate {
        self.fan.refresh()
    }
    fn fan_apply(&mut self, curve: &ValidatedFanCurve) -> FanClientUpdate {
        self.fan.apply(curve)
    }
    fn fan_restore(&mut self) -> FanClientUpdate {
        self.fan.restore()
    }
    fn fan_shutdown(&mut self) -> FanClientUpdate {
        self.fan.shutdown()
    }
    fn fan_poll(&mut self) -> Option<FanClientUpdate> {
        self.fan.poll()
    }
}

impl Backend for Service {
    fn snapshot(&mut self) -> Snapshot {
        Service::snapshot(self)
    }
    fn sample_thermal(&mut self) -> Result<ThermalSample, ControlError> {
        Service::sample_thermal(self)
    }
    fn catalog(&mut self) -> Vec<CatalogModel> {
        Service::catalog(self, "")
    }
    fn plan(&mut self, profile: PowerProfile) -> Result<ProfilePlan, ControlError> {
        self.plan_profile(profile)
    }
    fn apply(&mut self, plan: &ProfilePlan) -> Result<ApplyResult, ControlError> {
        self.apply_profile(plan)
    }
    fn plan_brightness(
        &mut self,
        percent: u8,
        device: &str,
    ) -> Result<BrightnessPlan, ControlError> {
        Service::plan_brightness(self, percent, Some(device))
    }
    fn apply_brightness(
        &mut self,
        plan: &BrightnessPlan,
    ) -> Result<BrightnessApplyResult, ControlError> {
        Service::apply_brightness(self, plan)
    }
    fn plan_oem(&mut self, control: &str, target: OemValue) -> Result<OemPlan, ControlError> {
        Service::plan_oem(self, control, target)
    }
    fn apply_oem(&mut self, plan: &OemPlan) -> Result<OemApplyResult, ControlError> {
        mechrevo_console_core::privilege::apply_oem(self, plan)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DisplayMode {
    #[default]
    Standard,
    Compact,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Only display preferences persist. Plans and hardware state never do.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub display_mode: DisplayMode,
    pub theme: Theme,
    pub text_scale: f32,
    pub auto_refresh: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            display_mode: DisplayMode::Standard,
            theme: Theme::Dark,
            text_scale: 1.0,
            auto_refresh: true,
        }
    }
}

impl Preferences {
    pub fn sanitize(&mut self) {
        self.text_scale = if self.text_scale.is_finite() {
            self.text_scale.clamp(0.8, 1.6)
        } else {
            1.0
        };
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Refresh,
    Preview,
    Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FanOperation {
    Probe,
    Refresh,
    Apply,
    Restore,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsFnOperation {
    Probe,
    Apply,
    FinishPending,
}

#[derive(Debug)]
enum Job {
    Refresh,
    RefreshBrightness,
    ThermalRefresh,
    Preview(PowerProfile),
    Apply(ProfilePlan),
    PreviewBrightness { percent: u8, device: String },
    ApplyBrightness(BrightnessPlan),
    PreviewOem { control: String, target: OemValue },
    ApplyOem(OemPlan),
    ProbeCpuPower,
    ExecuteCpuPower(CpuPlan),
    ProbeWindowsFn,
    ApplyWindowsFn(OemPlan),
    FinishWindowsFn,
    ProbeFan,
    RefreshFan,
    ApplyFan(ValidatedFanCurve),
    RestoreFan,
    ShutdownFan,
}

impl Job {
    fn kind(&self) -> JobKind {
        match self {
            Self::Refresh
            | Self::RefreshBrightness
            | Self::ThermalRefresh
            | Self::ProbeFan
            | Self::RefreshFan
            | Self::ProbeCpuPower
            | Self::ProbeWindowsFn
            | Self::FinishWindowsFn => JobKind::Refresh,
            Self::Preview(_) | Self::PreviewBrightness { .. } | Self::PreviewOem { .. } => {
                JobKind::Preview
            }
            Self::Apply(_)
            | Self::ApplyBrightness(_)
            | Self::ApplyOem(_)
            | Self::ExecuteCpuPower(_)
            | Self::ApplyWindowsFn(_)
            | Self::ApplyFan(_)
            | Self::RestoreFan
            | Self::ShutdownFan => JobKind::Apply,
        }
    }
    fn fan_operation(&self) -> Option<FanOperation> {
        match self {
            Self::ProbeFan => Some(FanOperation::Probe),
            Self::RefreshFan => Some(FanOperation::Refresh),
            Self::ApplyFan(_) => Some(FanOperation::Apply),
            Self::RestoreFan => Some(FanOperation::Restore),
            Self::ShutdownFan => Some(FanOperation::Shutdown),
            _ => None,
        }
    }
    fn windows_fn_operation(&self) -> Option<WindowsFnOperation> {
        match self {
            Self::ProbeWindowsFn => Some(WindowsFnOperation::Probe),
            Self::ApplyWindowsFn(_) => Some(WindowsFnOperation::Apply),
            Self::FinishWindowsFn => Some(WindowsFnOperation::FinishPending),
            _ => None,
        }
    }
}

struct Request {
    id: u64,
    job: Job,
}

enum Reply {
    Snapshot(Snapshot, Vec<CatalogModel>, ThermalSample),
    ThermalSample(ThermalSample),
    ThermalError(ControlError, Instant, Instant),
    Preview(ProfilePlan),
    Applied(ApplyResult, Snapshot, ThermalSample),
    BrightnessPreview(BrightnessPlan),
    BrightnessPreviewError(ControlError),
    BrightnessApplied(
        Box<(
            BrightnessPlan,
            BrightnessApplyResult,
            Snapshot,
            ThermalSample,
        )>,
    ),
    BrightnessFailed {
        plan: BrightnessPlan,
        error: ControlError,
        writer_unknown: bool,
    },
    OemPreview(OemPlan),
    OemApplied(OemApplyResult, Snapshot, ThermalSample),
    CpuPower {
        probe: bool,
        update: CpuPowerUpdate,
        finished_at: Instant,
    },
    WindowsFn {
        operation: WindowsFnOperation,
        update: Box<WindowsFnUpdate>,
        finished_at: Instant,
    },
    Fan {
        operation: FanOperation,
        curve: Option<ValidatedFanCurve>,
        update: FanClientUpdate,
    },
    Error(ControlError),
}

struct Response {
    id: u64,
    sequence: u64,
    reply: Reply,
}
struct FanEvent {
    sequence: u64,
    update: FanClientUpdate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    Busy,
    WorkerStopped,
    Unsupported,
    PreviewRequired,
}

struct InFlight {
    id: u64,
    kind: JobKind,
    started: Instant,
    selection_revision: u64,
    fan_operation: Option<FanOperation>,
    thermal_refresh: bool,
    cpu_power_operation: bool,
    windows_fn_operation: Option<WindowsFnOperation>,
    brightness_reconcile: bool,
    brightness_plan: Option<BrightnessPlan>,
}

pub struct Controller {
    requests: Option<SyncSender<Request>>,
    responses: Receiver<Response>,
    fan_events: Receiver<FanEvent>,
    fan_sequence: u64,
    alive: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    in_flight: Option<InFlight>,
    next_id: u64,
    pub snapshot: Option<Snapshot>,
    pub catalog: Vec<CatalogModel>,
    pub selected_profile: PowerProfile,
    pub preview: Option<ProfilePlan>,
    pub applied: Option<ApplyResult>,
    pub selected_brightness_device: Option<String>,
    pub requested_brightness: u8,
    pub brightness_preview: Option<BrightnessPlan>,
    pub brightness_applied: Option<BrightnessApplyResult>,
    pub brightness_error: Option<ControlError>,
    pub brightness_preview_error: Option<ControlError>,
    pub brightness_transaction: Option<BrightnessTransaction>,
    pub brightness_last_plan: Option<BrightnessPlan>,
    pub brightness_needs_refresh: bool,
    pub brightness_writer_may_continue: bool,
    pub selected_oem_control: Option<String>,
    pub requested_oem_value: Option<OemValue>,
    pub oem_preview: Option<OemPlan>,
    pub oem_applied: Option<OemApplyResult>,
    /// A validated local edit is not an application or a hardware readback.
    pub fan_draft: Option<ValidatedFanCurve>,
    pub fan_preview: Option<ValidatedFanCurve>,
    pub fan_applied: Option<ValidatedFanCurve>,
    pub fan_draft_error: Option<ControlError>,
    pub fan_error: Option<ControlError>,
    pub fan_state: Option<BrokerState>,
    pub fan_session_held: bool,
    pub fan_helper_reaped: bool,
    pub fan_cleanup_complete: bool,
    pub fan_restoration_confirmed: bool,
    close_requested: bool,
    shutdown_attempted: bool,
    selection_revision: u64,
    oem_write_uncertain: bool,
    pub cpu_power_status: Option<CpuStatus>,
    pub cpu_power_draft: Option<CpuPolicy>,
    pub cpu_power_preview: Option<CpuPlan>,
    pub cpu_power_error: Option<ControlError>,
    pub cpu_power_draft_error: Option<ControlError>,
    pub cpu_power_receipt: Option<serde_json::Value>,
    pub cpu_power_outcome: Option<CpuOutcome>,
    pub cpu_power_helper_reaped: bool,
    pub cpu_power_state_unknown: bool,
    pub cpu_power_needs_probe: bool,
    /// Time that the worker received this status, not a sensor sampling timestamp.
    pub cpu_power_received_at: Option<std::time::SystemTime>,
    pub cpu_power_response_finished: Option<Instant>,
    cpu_power_revision: u64,
    cpu_power_preview_revision: Option<u64>,
    pub windows_fn_status: Option<WindowsFnStatus>,
    pub windows_fn_requested: Option<bool>,
    pub windows_fn_preview: Option<OemPlan>,
    pub windows_fn_error: Option<ControlError>,
    pub windows_fn_draft_error: Option<ControlError>,
    pub windows_fn_receipt: Option<WindowsFnReceipt>,
    pub windows_fn_followup_receipt: Option<WindowsFnReceipt>,
    pub windows_fn_result: Option<OemApplyResult>,
    pub windows_fn_outcome: Option<WindowsFnOutcome>,
    pub windows_fn_helper_reaped: bool,
    pub windows_fn_helper_lifetime: WindowsFnLifetime,
    pub windows_fn_state_unknown: bool,
    pub windows_fn_needs_probe: bool,
    /// Reception time only; authorization and hardware freshness are separate.
    pub windows_fn_received_at: Option<std::time::SystemTime>,
    pub windows_fn_response_finished: Option<Instant>,
    windows_fn_revision: u64,
    windows_fn_preview_revision: Option<u64>,
    pub error: Option<ControlError>,
    pub last_refresh: Option<Instant>,
    /// Full-device discovery and thermal samples retain separate clocks.
    pub thermal_sample: Option<ThermalSample>,
    pub thermal_error: Option<ControlError>,
    pub thermal_last_attempt: Option<Instant>,
    pub thermal_last_finished: Option<Instant>,
    pub worker_available: bool,
}

pub const THERMAL_SAMPLE_INTERVAL: Duration = Duration::from_secs(3);
pub const THERMAL_STALE_AFTER: Duration = Duration::from_secs(6);

fn is_thermal_reading(reading: &SensorReading) -> bool {
    matches!(
        reading.kind.as_str(),
        "temperature" | "temp" | "fan_speed" | "fan"
    )
}

fn observed_snapshot(backend: &mut impl Backend) -> (Snapshot, ThermalSample) {
    let started_at = Instant::now();
    let snapshot = backend.snapshot();
    let finished_at = Instant::now();
    // This is the envelope of the actual full read, not an additional read or
    // a fallback for the dedicated thermal provider.
    let thermal = ThermalSample {
        readings: snapshot
            .telemetry
            .iter()
            .filter(|reading| is_thermal_reading(reading))
            .cloned()
            .collect(),
        diagnostics: vec![],
        started_at,
        finished_at,
    };
    (snapshot, thermal)
}

fn fan_clean(update: &FanClientUpdate) -> bool {
    !update.session_held && update.helper_reaped && update.cleanup_complete
}

fn reconcile_fan_update(update: &mut FanClientUpdate) {
    // A fresh local-owner receipt cannot simultaneously prove cleanup. A
    // read-only observation of a foreign lease does not make this UI its owner.
    if update.cleanup_complete
        && !update.session_held
        && update
            .state
            .as_ref()
            .is_some_and(|state| state.state_is_current && state.owner)
    {
        update.session_held = true;
        update.cleanup_complete = false;
        update.restoration_confirmed = false;
        update.error.get_or_insert_with(|| {
            ControlError::new(
                "fan_cleanup_unconfirmed",
                "现场回读仍显示本会话持有控制权，不能确认清理完成。",
            )
        });
    }
}

fn execute_job(backend: &mut impl Backend, job: Job) -> Reply {
    match job {
        Job::Refresh | Job::RefreshBrightness => {
            let (snapshot, thermal) = observed_snapshot(backend);
            Reply::Snapshot(snapshot, backend.catalog(), thermal)
        }
        Job::ThermalRefresh => {
            let started_at = Instant::now();
            match backend.sample_thermal() {
                Ok(sample) => Reply::ThermalSample(sample),
                Err(error) => Reply::ThermalError(error, started_at, Instant::now()),
            }
        }
        Job::Preview(profile) => match backend.plan(profile) {
            Ok(plan) => Reply::Preview(plan),
            Err(error) => Reply::Error(error),
        },
        Job::Apply(plan) => match backend.apply(&plan) {
            Ok(result) => {
                let (snapshot, thermal) = observed_snapshot(backend);
                Reply::Applied(result, snapshot, thermal)
            }
            Err(error) => Reply::Error(error),
        },
        Job::PreviewBrightness { percent, device } => {
            match backend.plan_brightness(percent, &device) {
                Ok(plan) => Reply::BrightnessPreview(plan),
                Err(error) => Reply::BrightnessPreviewError(error),
            }
        }
        Job::ApplyBrightness(plan) => match backend.apply_brightness(&plan) {
            Ok(result) => {
                let (snapshot, thermal) = observed_snapshot(backend);
                Reply::BrightnessApplied(Box::new((plan, result, snapshot, thermal)))
            }
            Err(error) => Reply::BrightnessFailed {
                plan,
                error,
                writer_unknown: false,
            },
        },
        Job::PreviewOem { control, target } => match backend.plan_oem(&control, target) {
            Ok(plan) => Reply::OemPreview(plan),
            Err(error) => Reply::Error(error),
        },
        Job::ApplyOem(plan) => match backend.apply_oem(&plan) {
            Ok(result) => {
                let (snapshot, thermal) = observed_snapshot(backend);
                Reply::OemApplied(result, snapshot, thermal)
            }
            Err(error) => Reply::Error(error),
        },
        Job::ProbeCpuPower => Reply::CpuPower {
            probe: true,
            update: backend.cpu_power_probe(),
            finished_at: Instant::now(),
        },
        Job::ExecuteCpuPower(plan) => Reply::CpuPower {
            probe: false,
            update: backend.cpu_power_execute(&plan),
            finished_at: Instant::now(),
        },
        Job::ProbeWindowsFn => Reply::WindowsFn {
            operation: WindowsFnOperation::Probe,
            update: Box::new(backend.windows_fn_probe()),
            finished_at: Instant::now(),
        },
        Job::ApplyWindowsFn(plan) => Reply::WindowsFn {
            operation: WindowsFnOperation::Apply,
            update: Box::new(backend.windows_fn_apply(&plan)),
            finished_at: Instant::now(),
        },
        Job::FinishWindowsFn => Reply::WindowsFn {
            operation: WindowsFnOperation::FinishPending,
            update: Box::new(backend.windows_fn_finish_pending()),
            finished_at: Instant::now(),
        },

        Job::ProbeFan => Reply::Fan {
            operation: FanOperation::Probe,
            curve: None,
            update: backend.fan_probe(),
        },
        Job::RefreshFan => Reply::Fan {
            operation: FanOperation::Refresh,
            curve: None,
            update: backend.fan_refresh(),
        },
        Job::ApplyFan(curve) => {
            let update = backend.fan_apply(&curve);
            Reply::Fan {
                operation: FanOperation::Apply,
                curve: Some(curve),
                update,
            }
        }
        Job::RestoreFan => Reply::Fan {
            operation: FanOperation::Restore,
            curve: None,
            update: backend.fan_restore(),
        },
        Job::ShutdownFan => Reply::Fan {
            operation: FanOperation::Shutdown,
            curve: None,
            update: backend.fan_shutdown(),
        },
    }
}

impl Controller {
    /// Starting a controller does not access hardware. Refresh is explicit.
    pub fn new(mut backend: impl Backend) -> Result<Self, std::io::Error> {
        let (requests, incoming) = mpsc::sync_channel::<Request>(1);
        let (outgoing, responses) = mpsc::sync_channel::<Response>(1);
        let (fan_outgoing, fan_events) = mpsc::sync_channel::<FanEvent>(1);
        let alive = Arc::new(AtomicBool::new(true));
        let worker_alive = alive.clone();
        let worker = thread::Builder::new()
            .name("mechrevo-platform".into())
            .spawn(move || {
                let mut fan_may_live = false;
                let mut sequence = 0_u64;
                let mut pending_fan_event = None;
                loop {
                    let request = match incoming.recv_timeout(Duration::from_millis(200)) {
                        Ok(request) => request,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            if fan_may_live {
                                let update = catch_unwind(AssertUnwindSafe(|| backend.fan_poll()))
                                    .unwrap_or_else(|_| Some(FanClientUpdate {
                                        state:None, error:Some(ControlError::new("fan_poll_panicked", "辅助进程状态检测异常；恢复与退出尚未确认。")),
                                        session_held:true, helper_reaped:false,
                                        restoration_confirmed:false, cleanup_complete:false,
                                    }));
                                if let Some(mut update) = update {
                                    reconcile_fan_update(&mut update);
                                    fan_may_live = !fan_clean(&update);
                                    sequence = sequence.wrapping_add(1);
                                    pending_fan_event = Some(FanEvent { sequence, update });
                                }
                            }
                            if let Some(event) = pending_fan_event.take() {
                                match fan_outgoing.try_send(event) {
                                    Ok(()) => {},
                                    Err(mpsc::TrySendError::Full(event)) => pending_fan_event = Some(event),
                                    Err(mpsc::TrySendError::Disconnected(_)) => break,
                                }
                            }
                            continue;
                        }
                    };
                    if !worker_alive.load(Ordering::Acquire) {
                        break;
                    }
                    let fan_operation = request.job.fan_operation();
                    let thermal_refresh = matches!(&request.job, Job::ThermalRefresh);
                    let cpu_power_operation = matches!(&request.job, Job::ProbeCpuPower | Job::ExecuteCpuPower(_));
                    let cpu_probe = matches!(&request.job, Job::ProbeCpuPower);
                    let windows_fn_operation = request.job.windows_fn_operation();
                    let brightness_plan = match &request.job { Job::ApplyBrightness(plan) => Some(plan.clone()), _ => None };
                    let thermal_started_at = Instant::now();
                    let preserve_backend = fan_may_live || fan_operation.is_some() || cpu_power_operation || brightness_plan.is_some() || windows_fn_operation.is_some();
                    let result = catch_unwind(AssertUnwindSafe(|| execute_job(&mut backend, request.job)));
                    let panicked = result.is_err();
                    let mut reply = result.unwrap_or_else(|_| {
                        let error = ControlError::new(
                            "worker_panicked", if thermal_refresh {
                                "温度与转速采样异常，自动采样已暂停；请点击刷新重试。"
                            } else { "后台控制发生异常；恢复完成前不能关闭会话。" },
                        );
                        if let Some(operation) = windows_fn_operation {
                            Reply::WindowsFn { operation, finished_at: Instant::now(), update: Box::new(windows_fn_controller::uncertain_update(
                                ControlError::new("windows_fn_worker_panicked", "Windows Fn 锁后台异常，设备状态和辅助进程退出尚未确认。"))) }
                        } else if let Some(plan) = brightness_plan {
                            Reply::BrightnessFailed { plan,
                                error: ControlError::new("brightness_worker_panicked", "亮度操作异常，未确认上次调节结束，不能再次提交。"),
                                writer_unknown: true }
                        } else if cpu_power_operation {
                            Reply::CpuPower { probe: cpu_probe, finished_at: Instant::now(), update: CpuPowerUpdate {
                                status: None, outcome: CpuOutcome::FailedStateUnknown,
                                error: Some(ControlError::new("cpu_power_worker_panicked", "CPU 功耗后台异常；未确认授权辅助进程退出，后续写入和关闭已阻止。")),
                                receipt: None, helper_reaped: false,
                            } }
                        } else if thermal_refresh {
                            // A failed read-only provider does not revoke or
                            // fabricate a fan ownership/recovery receipt.
                            Reply::ThermalError(error, thermal_started_at, Instant::now())
                        } else if preserve_backend {
                            // Keep the client and worker for an explicit recovery attempt.
                            // A panic is never evidence that a privileged child stopped.
                            Reply::Fan {
                                operation:fan_operation.unwrap_or(FanOperation::Refresh), curve:None,
                                update:FanClientUpdate { state:None, error:Some(error),
                                    session_held:true, helper_reaped:false,
                                    restoration_confirmed:false, cleanup_complete:false },
                            }
                        } else { Reply::Error(error) }
                    });
                    if let Reply::Fan { update, .. } = &mut reply { reconcile_fan_update(update); }
                    let shutdown_complete = matches!(&reply,
                        Reply::Fan { operation:FanOperation::Shutdown, update, .. } if fan_clean(update));
                    if let Reply::Fan { update, .. } = &reply {
                        fan_may_live = !fan_clean(update);
                    }
                    sequence = sequence.wrapping_add(1);
                    if !worker_alive.load(Ordering::Acquire)
                        || outgoing.send(Response { id:request.id, sequence, reply }).is_err()
                        || (panicked && !preserve_backend && !thermal_refresh) || shutdown_complete
                    { break; }
                }
                if fan_may_live {
                    // Programmatic Drop is a fallback, never a UI-confirmed close.
                    // Cleanup runs here so dropping the UI does not block rendering.
                    let _ = catch_unwind(AssertUnwindSafe(|| backend.fan_shutdown()));
                }
            })?;
        Ok(Self {
            requests: Some(requests),
            responses,
            fan_events,
            fan_sequence: 0,
            alive,
            worker: Some(worker),
            in_flight: None,
            next_id: 1,
            snapshot: None,
            catalog: Vec::new(),
            selected_profile: PowerProfile::Balanced,
            preview: None,
            applied: None,
            selected_brightness_device: None,
            requested_brightness: 70,
            brightness_preview: None,
            brightness_applied: None,
            brightness_error: None,
            brightness_preview_error: None,
            brightness_transaction: None,
            brightness_last_plan: None,
            brightness_needs_refresh: false,
            brightness_writer_may_continue: false,
            selected_oem_control: None,
            requested_oem_value: None,
            oem_preview: None,
            oem_applied: None,
            fan_draft: None,
            fan_preview: None,
            fan_applied: None,
            fan_draft_error: None,
            fan_error: None,
            fan_state: None,
            fan_session_held: false,
            fan_helper_reaped: true,
            fan_cleanup_complete: true,
            fan_restoration_confirmed: false,
            close_requested: false,
            shutdown_attempted: false,
            selection_revision: 0,
            oem_write_uncertain: false,
            cpu_power_status: None,
            cpu_power_draft: None,
            cpu_power_preview: None,
            cpu_power_error: None,
            cpu_power_draft_error: None,
            cpu_power_receipt: None,
            cpu_power_outcome: None,
            cpu_power_helper_reaped: true,
            cpu_power_state_unknown: false,
            cpu_power_needs_probe: true,
            cpu_power_received_at: None,
            cpu_power_response_finished: None,
            cpu_power_revision: 0,
            cpu_power_preview_revision: None,
            windows_fn_status: None,
            windows_fn_requested: None,
            windows_fn_preview: None,
            windows_fn_error: None,
            windows_fn_draft_error: None,
            windows_fn_receipt: None,
            windows_fn_followup_receipt: None,
            windows_fn_result: None,
            windows_fn_outcome: None,
            windows_fn_helper_reaped: true,
            windows_fn_helper_lifetime: WindowsFnLifetime::NotStarted,
            windows_fn_state_unknown: false,
            windows_fn_needs_probe: true,
            windows_fn_received_at: None,
            windows_fn_response_finished: None,
            windows_fn_revision: 0,
            windows_fn_preview_revision: None,
            error: None,
            last_refresh: None,
            thermal_sample: None,
            thermal_error: None,
            thermal_last_attempt: None,
            thermal_last_finished: None,
            worker_available: true,
        })
    }

    pub fn busy(&self) -> bool {
        self.in_flight.is_some()
            || self.oem_write_uncertain
            || !self.cpu_power_helper_reaped
            || !self.windows_fn_lifetime_clean()
            || (self.close_requested
                && self.shutdown_attempted
                && self.fan_cleanup_complete
                && self.fan_helper_reaped
                && !self.fan_session_held
                && self.worker.is_some())
    }

    pub fn write_in_progress(&self) -> bool {
        self.oem_write_uncertain
            || !self.cpu_power_helper_reaped
            || !self.windows_fn_lifetime_clean()
            || self
                .in_flight
                .as_ref()
                .is_some_and(|job| job.kind == JobKind::Apply)
    }

    pub fn oem_write_state_unknown(&self) -> bool {
        self.oem_write_uncertain
    }

    pub fn activity(&self) -> Option<(JobKind, Duration)> {
        self.in_flight
            .as_ref()
            .map(|job| (job.kind, job.started.elapsed()))
    }

    pub fn fan_activity(&self) -> Option<FanOperation> {
        self.in_flight.as_ref().and_then(|job| job.fan_operation)
    }

    pub fn fan_apply_unavailable_reason(&self) -> Option<&str> {
        if self.windows_fn_state_unknown
            || !self.windows_fn_lifetime_clean()
            || self.windows_fn_preview.is_some()
        {
            return Some("Windows Fn 锁事务或预览尚未确认，不能开始其他写入。");
        }
        if self.cpu_power_state_unknown || !self.cpu_power_helper_reaped {
            return Some("CPU 功耗事务尚未确认，不能开始其他写入。请先检查其状态。");
        }
        if self.cpu_power_preview.is_some() {
            return Some("CPU 功耗预览待确认，请先完成或取消该预览。");
        }
        let Some(state) = self.fan_state.as_ref() else {
            return Some("先只读检测本机风扇接口；旧驱动或未知接口不能应用曲线。");
        };
        if !state.state_is_current
            || state.live_error != 0
            || state
                .live_table
                .as_ref()
                .is_none_or(|table| table.len() != 96)
            || state.live_controls.is_none()
        {
            return Some("风扇现场回读未确认，请显式刷新状态。");
        }
        if state.capabilities & 0x07 != 0x07 {
            return Some("此型号或已加载驱动未提供经过限定的曲线控制接口。");
        }
        if state.revoked || state.phase == BrokerPhase::RecoveryRequired {
            return Some("会话已撤销或需要恢复，请先恢复原始风扇状态。");
        }
        if self.fan_session_held {
            if !state.owner || !matches!(state.phase, BrokerPhase::Acquired | BrokerPhase::Active) {
                return Some("当前会话没有可验证的曲线控制权，请先恢复。");
            }
        } else if state.phase != BrokerPhase::Idle
            || state.lease_id != 0
            || state.owner
            || state.modified_or_uncertain
        {
            return Some("风扇接口由其他会话占用或状态不明，不能申请新控制权。");
        }
        None
    }

    pub fn probe_fan(&mut self) -> Result<(), SubmitError> {
        // A new read-only fd would not own the existing lease. Query the
        // retained session instead of mislabelling it as a foreign owner.
        self.enqueue(if self.fan_session_held || !self.fan_helper_reaped {
            Job::RefreshFan
        } else {
            Job::ProbeFan
        })
    }

    pub fn refresh_fan(&mut self) -> Result<(), SubmitError> {
        self.enqueue(Job::RefreshFan)
    }

    pub fn apply_fan(&mut self) -> Result<(), SubmitError> {
        if self.busy() || self.close_requested {
            return Err(SubmitError::Busy);
        }
        if self.fan_apply_unavailable_reason().is_some() {
            return Err(SubmitError::Unsupported);
        }
        let curve = self
            .fan_preview
            .as_ref()
            .filter(|preview| self.fan_draft.as_ref() == Some(*preview))
            .cloned()
            .ok_or(SubmitError::PreviewRequired)?;
        self.enqueue(Job::ApplyFan(curve))?;
        self.invalidate_previews();
        Ok(())
    }

    pub fn restore_fan(&mut self) -> Result<(), SubmitError> {
        if self.busy() {
            return Err(SubmitError::Busy);
        }
        if self.fan_helper_reaped
            && self.fan_cleanup_complete
            && !self.fan_session_held
            && !self
                .fan_state
                .as_ref()
                .is_some_and(|state| state.phase == BrokerPhase::RecoveryRequired)
        {
            return Err(SubmitError::Unsupported);
        }
        self.enqueue(Job::RestoreFan)?;
        self.invalidate_previews();
        Ok(())
    }

    /// Start asynchronous cleanup. Repeated close events do not retry a failed
    /// transaction; the visible Restore action is the explicit recovery path.
    pub fn request_close(&mut self) {
        self.close_requested = true;
        self.invalidate_previews();
        self.invalidate_cpu_power_preview();
        self.invalidate_windows_fn_preview();
        self.drive_close();
    }

    pub fn close_ready(&self) -> bool {
        self.close_requested
            && self.in_flight.is_none()
            && !self.oem_write_uncertain
            && self.cpu_power_helper_reaped
            && self.windows_fn_lifetime_clean()
            && self.worker.is_none()
            && !self.fan_session_held
            && self.fan_helper_reaped
            && self.fan_cleanup_complete
    }

    fn drive_close(&mut self) {
        if self.close_requested
            && !self.shutdown_attempted
            && self.in_flight.is_none()
            && !self.oem_write_uncertain
            && self.cpu_power_helper_reaped
            && self.windows_fn_lifetime_clean()
            && self.worker_available
            && self.enqueue(Job::ShutdownFan).is_ok()
        {
            self.shutdown_attempted = true;
        }
    }

    pub fn refresh(&mut self) -> Result<(), SubmitError> {
        if self.cpu_power_preview.is_some() {
            return Err(SubmitError::Busy);
        }
        self.submit(Job::Refresh)?;
        self.invalidate_previews();
        Ok(())
    }

    /// This is deliberately allowed while a fan lease or recovery is pending.
    /// It never runs snapshot, OEM discovery, catalog or an authentication path.
    pub fn refresh_thermal(&mut self) -> Result<(), SubmitError> {
        if self.close_requested {
            return Err(SubmitError::Busy);
        }
        self.enqueue(Job::ThermalRefresh)?;
        self.thermal_last_attempt = self.in_flight.as_ref().map(|job| job.started);
        Ok(())
    }

    pub fn thermal_sampling(&self) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|job| job.thermal_refresh)
    }

    pub fn thermal_refresh_due(&self) -> bool {
        self.thermal_last_finished
            .or(self.thermal_last_attempt)
            .or(self.last_refresh)
            .is_some_and(|last| last.elapsed() >= THERMAL_SAMPLE_INTERVAL)
    }

    pub fn requires_thermal_refresh(&self) -> bool {
        self.fan_session_held || !self.fan_cleanup_complete || !self.fan_helper_reaped
    }

    pub fn uses_periodic_thermal_refresh(&self) -> bool {
        self.requires_thermal_refresh()
            || (self.has_pending_preview()
                && self
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot.platform == "linux"))
    }

    fn accept_thermal_sample(&mut self, sample: ThermalSample) {
        self.thermal_last_attempt = Some(sample.started_at);
        self.thermal_last_finished = Some(sample.finished_at);
        self.thermal_error = None;
        if let Some(snapshot) = &mut self.snapshot {
            snapshot
                .telemetry
                .retain(|reading| !is_thermal_reading(reading));
            snapshot.telemetry.extend(sample.readings.iter().cloned());
        }
        self.thermal_sample = Some(sample);
    }

    pub fn has_pending_preview(&self) -> bool {
        self.preview.is_some()
            || self.brightness_preview.is_some()
            || self.oem_preview.is_some()
            || self.fan_preview.is_some()
            || self.cpu_power_preview.is_some()
            || self.windows_fn_preview.is_some()
    }

    fn invalidate_previews(&mut self) {
        self.preview = None;
        self.brightness_preview = None;
        self.oem_preview = None;
        self.fan_preview = None;
        self.selection_revision = self.selection_revision.wrapping_add(1);
    }

    pub fn invalidate_fan_preview(&mut self) {
        self.fan_preview = None;
    }

    /// Bounded, local parsing only. An invalid replacement cannot leave an old
    /// valid preview armed for a later Apply click.
    pub fn import_fan_curve(&mut self, input: &[u8]) -> Result<(), ControlError> {
        self.fan_preview = None;
        self.fan_draft = None;
        self.fan_draft_error = None;
        match ValidatedFanCurve::from_json(input) {
            Ok(curve) => {
                self.fan_draft = Some(curve);
                Ok(())
            }
            Err(error) => {
                self.fan_draft_error = Some(error.clone());
                Err(error)
            }
        }
    }

    pub fn set_fan_draft(&mut self, curve: FanCurve) -> Result<(), ControlError> {
        self.fan_preview = None;
        self.fan_draft = None;
        self.fan_draft_error = None;
        match curve.validate() {
            Ok(curve) => {
                self.fan_draft = Some(curve);
                Ok(())
            }
            Err(error) => {
                self.fan_draft_error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// Preview checks the typed input without probing, authorizing or leasing.
    pub fn preview_fan(&mut self) -> Result<(), SubmitError> {
        if self.cpu_power_preview.is_some() || self.windows_fn_preview.is_some() {
            return Err(SubmitError::Busy);
        }
        if self.busy() {
            return Err(SubmitError::Busy);
        }
        self.fan_preview = Some(self.fan_draft.clone().ok_or(SubmitError::PreviewRequired)?);
        self.fan_draft_error = None;
        Ok(())
    }

    pub fn select_profile(&mut self, profile: PowerProfile) {
        if self.selected_profile != profile {
            self.selected_profile = profile;
            self.invalidate_previews();
        }
    }

    pub fn policy_unavailable_reason(&self) -> Option<&str> {
        if self.cpu_power_state_unknown {
            return Some("CPU 功耗状态未确认，请先显式检查；其他写入已阻止。");
        }
        if self.cpu_power_preview.is_some() {
            return Some("CPU 功耗预览待确认，请先完成或取消该预览。");
        }
        let Some(snapshot) = self.snapshot.as_ref() else {
            return Some("设备检测完成后才能预览电源策略。");
        };
        match snapshot
            .capabilities
            .iter()
            .find(|cap| cap.id == "power_profile")
        {
            Some(cap)
                if cap.status == CapabilityStatus::Available
                    && mechrevo_console_core::profile_available(
                        snapshot,
                        self.selected_profile,
                    ) =>
            {
                None
            }
            Some(cap) if cap.status == CapabilityStatus::Available => {
                Some("当前系统未公布所选电源策略，请选择受支持的档位。")
            }
            Some(cap) => Some(cap.reason.as_str()),
            None => Some("当前系统没有提供可用的电源策略接口。"),
        }
    }

    pub fn preview_profile(&mut self) -> Result<(), SubmitError> {
        if self.cpu_power_preview.is_some() {
            return Err(SubmitError::Busy);
        }
        if self.policy_unavailable_reason().is_some() {
            return Err(SubmitError::Unsupported);
        }
        self.invalidate_previews();
        self.submit(Job::Preview(self.selected_profile))?;
        Ok(())
    }

    pub fn apply_preview(&mut self) -> Result<(), SubmitError> {
        if self.policy_unavailable_reason().is_some() {
            return Err(SubmitError::Unsupported);
        }
        let plan = self
            .preview
            .as_ref()
            .filter(|plan| plan.requested == self.selected_profile)
            .cloned()
            .ok_or(SubmitError::PreviewRequired)?;
        self.submit(Job::Apply(plan))?;
        // A preview is single-use, including when applying fails or readback fails.
        self.invalidate_previews();
        self.applied = None;
        Ok(())
    }

    pub fn select_brightness_device(&mut self, device: &str) {
        if self.selected_brightness_device.as_deref() != Some(device) {
            self.selected_brightness_device = Some(device.to_owned());
            self.invalidate_previews();
            if let Some(current) = self
                .snapshot
                .as_ref()
                .and_then(|snapshot| {
                    snapshot
                        .brightness
                        .iter()
                        .find(|candidate| candidate.id == device)
                })
                .and_then(|candidate| {
                    candidate
                        .control_value
                        .zip(candidate.maximum)
                        .filter(|(_, maximum)| *maximum > 0)
                        .map(|(value, maximum)| f64::from(value) * 100.0 / f64::from(maximum))
                        .or(candidate.current_percent)
                })
                .filter(|value| value.is_finite())
            {
                self.requested_brightness = current.round().clamp(0.0, 100.0) as u8;
            }
        }
    }

    pub fn set_brightness_target(&mut self, percent: u8) {
        if self.requested_brightness != percent {
            self.requested_brightness = percent;
            self.invalidate_previews();
        }
    }

    pub fn brightness_unavailable_reason(&self) -> Option<&str> {
        if self.windows_fn_state_unknown
            || !self.windows_fn_lifetime_clean()
            || self.windows_fn_preview.is_some()
        {
            return Some("Windows Fn 锁事务或预览尚未确认，请先完成检查或取消预览。");
        }
        if self.brightness_writer_may_continue {
            return Some("尚未确认上次调节结束，不能再次提交。刷新只能核对当前读数。");
        }
        if self.brightness_needs_refresh {
            return Some("亮度状态未确认，请先刷新亮度状态，再重新预览。");
        }
        if self.cpu_power_state_unknown {
            return Some("CPU 功耗状态未确认，请先显式检查；其他写入已阻止。");
        }
        if self.cpu_power_preview.is_some() {
            return Some("CPU 功耗预览待确认，请先完成或取消该预览。");
        }
        if self.close_requested || !self.worker_available {
            return Some("后台正在退出或已停止，暂不能调节亮度。");
        }
        if self.fan_session_held || !self.fan_helper_reaped || !self.fan_cleanup_complete {
            return Some("风扇会话尚未恢复并退出，暂不能调节亮度。");
        }
        if self.preview.is_some() || self.oem_preview.is_some() || self.fan_preview.is_some() {
            return Some("其他控制的预览待确认，请先完成或取消该预览。");
        }
        let Some(snapshot) = &self.snapshot else {
            return Some("设备检测完成后才能预览亮度。");
        };
        let Some(id) = &self.selected_brightness_device else {
            return Some(if snapshot.brightness.is_empty() {
                "未发现操作系统亮度设备。"
            } else {
                "请选择要调节的显示设备。"
            });
        };
        let Some(device) = snapshot.brightness.iter().find(|device| &device.id == id) else {
            return Some("所选显示设备已离线，请刷新并重新选择。");
        };
        if device.status != CapabilityStatus::Available || !device.can_control() {
            return Some(
                device
                    .reason
                    .as_deref()
                    .unwrap_or("当前用户不能读写此设备的系统亮度接口。"),
            );
        }
        if !device
            .control_value
            .zip(device.maximum)
            .is_some_and(|(value, maximum)| maximum > 0 && value <= maximum)
        {
            return Some("当前亮度或设备范围未确认，请刷新后重新预览。");
        }
        if self.requested_brightness > 100 {
            return Some("亮度必须在 0% 到 100% 之间。");
        }
        if !device.supported_percentages.is_empty()
            && !device
                .supported_percentages
                .contains(&self.requested_brightness)
        {
            return Some("该显示设备不支持此亮度档位，请选择其公布的档位。");
        }
        None
    }

    pub fn preview_brightness(&mut self) -> Result<(), SubmitError> {
        if self.cpu_power_preview.is_some() {
            return Err(SubmitError::Busy);
        }
        if self.brightness_unavailable_reason().is_some() {
            return Err(SubmitError::Unsupported);
        }
        self.submit(Job::PreviewBrightness {
            percent: self.requested_brightness,
            device: self
                .selected_brightness_device
                .clone()
                .ok_or(SubmitError::Unsupported)?,
        })?;
        self.brightness_preview = None;
        self.brightness_preview_error = None;
        Ok(())
    }

    pub fn apply_brightness_preview(&mut self) -> Result<(), SubmitError> {
        if self.brightness_preview.is_none() {
            return Err(SubmitError::PreviewRequired);
        }
        if self.brightness_unavailable_reason().is_some() {
            return Err(SubmitError::Unsupported);
        }
        let plan = self
            .brightness_preview
            .as_ref()
            .filter(|plan| {
                Some(plan.device_id.as_str()) == self.selected_brightness_device.as_deref()
                    && plan.requested_percent == self.requested_brightness
            })
            .cloned()
            .ok_or(SubmitError::PreviewRequired)?;
        self.submit(Job::ApplyBrightness(plan))?;
        self.brightness_preview = None;
        self.brightness_applied = None;
        Ok(())
    }

    /// Explicit reconciliation, never an automatic retry of the old plan.
    pub fn refresh_brightness_state(&mut self) -> Result<(), SubmitError> {
        if self.has_pending_preview() {
            return Err(SubmitError::Busy);
        }
        self.submit(Job::RefreshBrightness)?;
        self.brightness_preview_error = None;
        Ok(())
    }

    fn reconcile_brightness_after_explicit_refresh(&mut self) {
        if self.brightness_writer_may_continue || !self.brightness_needs_refresh {
            return;
        }
        let valid = self.brightness_last_plan.as_ref().is_some_and(|plan| {
            self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.brightness.iter().any(|device| {
                    self.selected_brightness_device.as_deref() == Some(plan.device_id.as_str())
                        && device.id == plan.device_id
                        && device.source == plan.source
                        && device.maximum == Some(plan.maximum)
                        && device.status == CapabilityStatus::Available
                        && device.readable
                        && device
                            .control_value
                            .is_some_and(|value| plan.maximum > 0 && value <= plan.maximum)
                })
            })
        });
        if valid {
            self.brightness_needs_refresh = false;
        } else {
            self.brightness_preview_error = Some(ControlError::new(
                "brightness_refresh_unconfirmed",
                "未能确认上次调节的显示设备，请选择该设备并重新刷新亮度状态。",
            ));
        }
    }

    fn accept_brightness_failure(
        &mut self,
        plan: BrightnessPlan,
        error: ControlError,
        writer_unknown: bool,
    ) {
        let mut transaction =
            brightness::transaction_from_error(&error).unwrap_or_else(|| BrightnessTransaction {
                outcome: BrightnessOutcome::Unknown,
                write_attempted: true,
                writer_may_continue: writer_unknown,
                observed_control: None,
                primary_error: Some(error.clone()),
                rollback_error: None,
                readback_error: None,
                recovery: BrightnessRecovery::Unknown,
                recovery_reason: Some("未收到完整的亮度事务回执，不能推定未写入或已恢复。".into()),
            });
        if writer_unknown {
            transaction.writer_may_continue = true;
        }
        self.brightness_writer_may_continue |= transaction.writer_may_continue;
        self.brightness_needs_refresh =
            self.brightness_writer_may_continue || transaction.blocks_further_writes();
        self.brightness_transaction = Some(transaction);
        self.brightness_last_plan = Some(plan);
        self.brightness_applied = None;
        self.error = Some(error.clone());
        self.brightness_error = Some(error);
    }

    pub fn select_oem_control(&mut self, id: &str) {
        if self.selected_oem_control.as_deref() != Some(id) {
            self.selected_oem_control = Some(id.to_owned());
            self.requested_oem_value = self
                .selected_oem()
                .and_then(|control| control.current.clone());
            self.invalidate_previews();
        }
    }

    pub fn set_oem_target(&mut self, target: OemValue) {
        if self.requested_oem_value.as_ref() != Some(&target) {
            self.requested_oem_value = Some(target);
            self.invalidate_previews();
        }
    }

    pub fn selected_oem(&self) -> Option<&OemControl> {
        let id = self.selected_oem_control.as_deref()?;
        self.snapshot
            .as_ref()?
            .oem_controls
            .iter()
            .find(|control| control.id == id)
    }

    pub fn oem_unavailable_reason(&self) -> Option<&str> {
        if self.cpu_power_state_unknown {
            return Some("CPU 功耗状态未确认，请先显式检查；其他写入已阻止。");
        }
        if self.cpu_power_preview.is_some() {
            return Some("CPU 功耗预览待确认，请先完成或取消该预览。");
        }
        let Some(control) = self.selected_oem() else {
            return Some("请选择本机实际提供的设备控制接口。");
        };
        if control.status != CapabilityStatus::Available || control.current.is_none() {
            return Some(
                control
                    .reason
                    .as_deref()
                    .unwrap_or("该接口目前无法完成读取与写入，请刷新核对。"),
            );
        }
        if control
            .current
            .as_ref()
            .is_some_and(|value| !oem_value_in_domain(&control.domain, value))
        {
            return Some("接口当前值不在驱动公布的类型或范围内，请刷新核对。");
        }
        let Some(target) = &self.requested_oem_value else {
            return Some("选择目标值后才能预览更改。");
        };
        if !oem_value_in_domain(&control.domain, target) {
            return Some("目标值不在驱动公布的类型或范围内，请重新选择。");
        }
        None
    }

    fn oem_plan_matches(&self, plan: &OemPlan) -> bool {
        self.selected_oem().is_some_and(|control| {
            control.id == plan.control_id
                && control.kind == plan.kind
                && control.current.as_ref() == Some(&plan.previous)
                && control.domain == plan.domain
                && control.source == plan.source
                && control.binding == plan.binding
                && control.readback_scope == plan.readback_scope
                && self.requested_oem_value.as_ref() == Some(&plan.target)
        })
    }

    pub fn preview_oem(&mut self) -> Result<(), SubmitError> {
        if self.cpu_power_preview.is_some() {
            return Err(SubmitError::Busy);
        }
        if self.oem_unavailable_reason().is_some() {
            return Err(SubmitError::Unsupported);
        }
        let control = self
            .selected_oem_control
            .clone()
            .ok_or(SubmitError::Unsupported)?;
        let target = self
            .requested_oem_value
            .clone()
            .ok_or(SubmitError::Unsupported)?;
        self.invalidate_previews();
        self.submit(Job::PreviewOem { control, target })
    }

    pub fn apply_oem_preview(&mut self) -> Result<(), SubmitError> {
        if self.busy() {
            return Err(SubmitError::Busy);
        }
        if self.oem_unavailable_reason().is_some() {
            self.invalidate_previews();
            return Err(SubmitError::Unsupported);
        }
        let Some(plan) = self
            .oem_preview
            .as_ref()
            .filter(|plan| self.oem_plan_matches(plan))
            .cloned()
        else {
            self.invalidate_previews();
            return Err(SubmitError::PreviewRequired);
        };
        self.submit(Job::ApplyOem(plan))?;
        self.invalidate_previews();
        self.oem_applied = None;
        Ok(())
    }

    fn submit(&mut self, job: Job) -> Result<(), SubmitError> {
        if self.close_requested
            || self.fan_session_held
            || !self.fan_cleanup_complete
            || !self.fan_helper_reaped
            || (self.cpu_power_preview.is_some()
                && !matches!(&job, Job::ProbeCpuPower | Job::ExecuteCpuPower(_)))
            || (self.cpu_power_state_unknown && job.kind() == JobKind::Apply)
            || (self.windows_fn_preview.is_some() && job.windows_fn_operation().is_none())
            || (self.windows_fn_state_unknown && job.kind() == JobKind::Apply)
        {
            return Err(SubmitError::Busy);
        }
        self.enqueue(job)
    }

    fn enqueue(&mut self, job: Job) -> Result<(), SubmitError> {
        if self.busy() && !matches!(&job, Job::FinishWindowsFn) {
            return Err(SubmitError::Busy);
        }
        if !self.worker_available {
            return Err(SubmitError::WorkerStopped);
        }
        let id = self.next_id;
        let kind = job.kind();
        let fan_operation = job.fan_operation();
        let thermal_refresh = matches!(&job, Job::ThermalRefresh);
        let cpu_power_operation = matches!(&job, Job::ProbeCpuPower | Job::ExecuteCpuPower(_));
        let windows_fn_operation = job.windows_fn_operation();
        let brightness_operation = matches!(
            &job,
            Job::PreviewBrightness { .. } | Job::ApplyBrightness(_) | Job::RefreshBrightness
        );
        let brightness_reconcile = matches!(&job, Job::RefreshBrightness);
        let brightness_plan = match &job {
            Job::ApplyBrightness(plan) => Some(plan.clone()),
            _ => None,
        };
        let result = self
            .requests
            .as_ref()
            .ok_or(SubmitError::WorkerStopped)?
            .try_send(Request { id, job });
        match result {
            Ok(()) => {
                self.next_id = self.next_id.wrapping_add(1);
                self.in_flight = Some(InFlight {
                    id,
                    kind,
                    started: Instant::now(),
                    selection_revision: self.selection_revision,
                    fan_operation,
                    thermal_refresh,
                    cpu_power_operation,
                    windows_fn_operation,
                    brightness_reconcile,
                    brightness_plan,
                });
                // Fan lifecycle housekeeping has its own error channel. In
                // particular, closing must retain an earlier write/rollback
                // failure instead of erasing it while queuing ShutdownFan.
                if fan_operation.is_none()
                    && !thermal_refresh
                    && !cpu_power_operation
                    && windows_fn_operation.is_none()
                    && !brightness_operation
                {
                    self.error = None;
                }
                Ok(())
            }
            Err(mpsc::TrySendError::Full(_)) => Err(SubmitError::Busy),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.stopped();
                Err(SubmitError::WorkerStopped)
            }
        }
    }

    /// Nonblocking; safe to call from the event/render thread.
    pub fn poll(&mut self) -> bool {
        let mut lifecycle_changed = false;
        if let Ok(event) = self.fan_events.try_recv() {
            if event.sequence > self.fan_sequence {
                self.fan_sequence = event.sequence;
                self.accept_fan_update(FanOperation::Refresh, None, event.update);
                lifecycle_changed = true;
            }
        }
        let mut reaped_worker = false;
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            self.worker_available = false;
            reaped_worker = true;
        }
        match self.responses.try_recv() {
            Ok(response) => {
                if self.in_flight.as_ref().map(|job| job.id) != Some(response.id) {
                    return false;
                }
                let current_selection = self
                    .in_flight
                    .as_ref()
                    .is_some_and(|job| job.selection_revision == self.selection_revision);
                let brightness_reconcile = self
                    .in_flight
                    .as_ref()
                    .is_some_and(|job| job.brightness_reconcile);
                self.in_flight = None;
                match response.reply {
                    Reply::Snapshot(snapshot, catalog, thermal) => {
                        self.snapshot = Some(snapshot);
                        self.catalog = catalog;
                        self.last_refresh = Some(thermal.finished_at);
                        self.accept_thermal_sample(thermal);
                        if brightness_reconcile {
                            self.reconcile_brightness_after_explicit_refresh();
                        }
                    }
                    Reply::ThermalSample(sample) => {
                        self.accept_thermal_sample(sample);
                    }
                    Reply::ThermalError(error, started_at, finished_at) => {
                        self.thermal_error = Some(error);
                        self.thermal_last_attempt = Some(started_at);
                        self.thermal_last_finished = Some(finished_at);
                    }
                    Reply::CpuPower {
                        probe,
                        update,
                        finished_at,
                    } => {
                        self.accept_cpu_power_update(probe, update, finished_at);
                    }
                    Reply::WindowsFn {
                        operation,
                        update,
                        finished_at,
                    } => {
                        self.accept_windows_fn_update(operation, *update, finished_at);
                    }
                    Reply::Preview(plan) => {
                        // A selection made while a request ran must invalidate its late result.
                        if current_selection && plan.requested == self.selected_profile {
                            self.preview = Some(plan);
                        }
                    }
                    Reply::BrightnessPreview(plan) => {
                        if current_selection
                            && Some(plan.device_id.as_str())
                                == self.selected_brightness_device.as_deref()
                            && plan.requested_percent == self.requested_brightness
                        {
                            self.brightness_preview = Some(plan);
                            self.brightness_preview_error = None;
                        }
                    }
                    Reply::BrightnessPreviewError(error) => {
                        if current_selection {
                            self.brightness_preview_error = Some(error);
                        }
                    }
                    Reply::Applied(result, snapshot, thermal) => {
                        self.applied = Some(result);
                        self.snapshot = Some(snapshot);
                        self.last_refresh = Some(thermal.finished_at);
                        self.accept_thermal_sample(thermal);
                    }
                    Reply::BrightnessApplied(payload) => {
                        let (plan, result, snapshot, thermal) = *payload;
                        if self.error.as_ref().is_some_and(|error| {
                            [
                                self.brightness_error.as_ref(),
                                self.brightness_preview_error.as_ref(),
                            ]
                            .into_iter()
                            .flatten()
                            .any(|brightness| {
                                brightness.code == error.code
                                    && brightness.message == error.message
                                    && brightness.details == error.details
                            })
                        }) {
                            self.error = None;
                        }
                        self.brightness_transaction = Some(result.transaction.clone());
                        self.brightness_last_plan = Some(plan);
                        self.brightness_error = None;
                        self.brightness_preview_error = None;
                        self.brightness_needs_refresh = false;
                        self.brightness_applied = Some(result);
                        self.snapshot = Some(snapshot);
                        self.last_refresh = Some(thermal.finished_at);
                        self.accept_thermal_sample(thermal);
                    }
                    Reply::BrightnessFailed {
                        plan,
                        error,
                        writer_unknown,
                    } => {
                        self.accept_brightness_failure(plan, error, writer_unknown);
                    }
                    Reply::OemPreview(plan) => {
                        if current_selection {
                            if self.oem_plan_matches(&plan) {
                                self.oem_preview = Some(plan);
                            } else {
                                self.error = Some(ControlError::new(
                                    "oem_preview_stale",
                                    "设备状态或接口已变化，请刷新后重新预览。",
                                ));
                            }
                        }
                    }
                    Reply::OemApplied(result, snapshot, thermal) => {
                        self.oem_applied = Some(result);
                        self.snapshot = Some(snapshot);
                        self.last_refresh = Some(thermal.finished_at);
                        self.accept_thermal_sample(thermal);
                    }
                    Reply::Fan {
                        operation,
                        curve,
                        update,
                    } => {
                        // Lifecycle observations can arrive after an operation
                        // completed but before its reply was rendered. Never let
                        // an older Apply ACK overwrite a newer child-exit event.
                        if response.sequence > self.fan_sequence {
                            self.fan_sequence = response.sequence;
                            self.accept_fan_update(operation, curve, update);
                        }
                    }
                    Reply::Error(error) => {
                        if error.code == "oem_helper_liveness_unknown" {
                            // A missing wait result is not evidence that a
                            // privileged writer stopped. Keep the write/close
                            // interlock until this process is explicitly shut
                            // down outside the normal native close path.
                            self.oem_write_uncertain = true;
                            self.invalidate_previews();
                        }
                        self.error = Some(error);
                    }
                }
                if let Some(snapshot) = &self.snapshot {
                    if self.selected_brightness_device.is_none() && snapshot.brightness.len() == 1 {
                        let id = snapshot.brightness[0].id.clone();
                        self.select_brightness_device(&id);
                    }
                }
                // A previously unavailable control can recover with the same
                // ID. Initialize only an absent target; keep user edits.
                if self.requested_oem_value.is_none() {
                    if let Some(value) = self
                        .selected_oem()
                        .filter(|control| control.status == CapabilityStatus::Available)
                        .and_then(|control| control.current.clone())
                    {
                        self.requested_oem_value = Some(value);
                        self.invalidate_previews();
                    }
                }
                self.drive_close();
                true
            }
            Err(TryRecvError::Empty) => {
                self.drive_close();
                reaped_worker || lifecycle_changed
            }
            Err(TryRecvError::Disconnected) => {
                let changed = self.worker_available;
                if self.close_requested
                    && self.shutdown_attempted
                    && self.fan_cleanup_complete
                    && self.fan_helper_reaped
                    && !self.fan_session_held
                    && self.in_flight.is_none()
                {
                    self.worker_available = false;
                } else {
                    self.stopped();
                }
                changed || reaped_worker || lifecycle_changed
            }
        }
    }

    fn accept_fan_update(
        &mut self,
        operation: FanOperation,
        curve: Option<ValidatedFanCurve>,
        mut update: FanClientUpdate,
    ) {
        reconcile_fan_update(&mut update);
        let confirmed_apply = operation == FanOperation::Apply
            && update.error.is_none()
            && update.session_held
            && !update.helper_reaped
            && update.state.as_ref().is_some_and(|state| {
                state.state_is_current
                    && state.live_error == 0
                    && state.last_error == 0
                    && state.restore_error == 0
                    && state.owner
                    && !state.revoked
                    && state.phase == BrokerPhase::Active
                    && curve
                        .as_ref()
                        .is_some_and(|curve| state.applied_curve.as_ref() == Some(curve.curve()))
            });
        if operation == FanOperation::Apply {
            self.fan_applied = if confirmed_apply { curve } else { None };
            if update.error.is_none() && !confirmed_apply {
                update.error = Some(ControlError::new(
                    "fan_apply_unconfirmed",
                    "没有收到与目标曲线对应的现场应用回执；请检查并恢复原始状态。",
                ));
                update.session_held = true;
                update.cleanup_complete = false;
                update.restoration_confirmed = false;
            }
        }
        if operation == FanOperation::Restore
            || operation == FanOperation::Shutdown
            || update.state.as_ref().is_none_or(|state| {
                !state.state_is_current
                    || state.revoked
                    || !state.owner
                    || state.phase != BrokerPhase::Active
            })
        {
            self.fan_applied = None;
        }
        if let Some(state) = update.state {
            self.fan_state = Some(state);
        } else if let Some(state) = self.fan_state.as_mut() {
            state.state_is_current = false;
            if update.session_held {
                state.modified_or_uncertain = true;
            }
        }
        self.fan_session_held = update.session_held;
        self.fan_helper_reaped = update.helper_reaped;
        self.fan_cleanup_complete = update.cleanup_complete;
        self.fan_restoration_confirmed = update.restoration_confirmed;
        self.fan_error = update.error;
        if operation == FanOperation::Shutdown {
            self.shutdown_attempted = true;
        } else if operation == FanOperation::Refresh
            && self.close_requested
            && self.in_flight.is_none()
            && self.fan_cleanup_complete
            && self.fan_helper_reaped
            && !self.fan_session_held
        {
            // Cleanup may become confirmed later by the cheap reaper path.
            // End the now-clean worker too; no new hardware transaction is
            // needed, and the previous failed close must not leave it stuck.
            self.shutdown_attempted = false;
        }
        if operation == FanOperation::Restore && self.close_requested {
            // A successful explicit recovery allows the final worker shutdown;
            // a failed one waits for another explicit action, without a loop.
            self.shutdown_attempted = !self.fan_cleanup_complete || self.fan_session_held;
        }
    }

    fn stopped(&mut self) {
        if let Some(operation) = self
            .in_flight
            .as_ref()
            .and_then(|job| job.windows_fn_operation)
        {
            self.accept_windows_fn_update(
                operation,
                windows_fn_controller::uncertain_update(ControlError::new(
                    "windows_fn_worker_disconnected",
                    "Windows Fn 锁后台中断，辅助进程退出尚未确认。",
                )),
                Instant::now(),
            );
        }
        if let Some(plan) = self
            .in_flight
            .as_ref()
            .and_then(|job| job.brightness_plan.clone())
        {
            self.accept_brightness_failure(
                plan,
                ControlError::new(
                    "brightness_worker_disconnected",
                    "亮度后台中断，未确认上次调节结束，不能再次提交。",
                ),
                true,
            );
        }
        if self
            .in_flight
            .as_ref()
            .is_some_and(|job| job.cpu_power_operation)
        {
            self.cpu_power_state_unknown = true;
            self.cpu_power_helper_reaped = false;
            self.cpu_power_error = Some(ControlError::new(
                "cpu_power_worker_disconnected",
                "CPU 功耗后台断开，未确认授权辅助进程退出；后续写入和关闭已阻止。",
            ));
        }
        if self
            .in_flight
            .as_ref()
            .is_some_and(|job| job.fan_operation.is_some())
        {
            self.fan_session_held = true;
            self.fan_helper_reaped = false;
            self.fan_cleanup_complete = false;
            self.fan_restoration_confirmed = false;
            self.fan_applied = None;
            self.fan_error = Some(ControlError::new(
                "fan_worker_disconnected",
                "风扇控制线程已断开；未确认恢复或辅助进程退出。",
            ));
        }
        self.worker_available = false;
        self.in_flight = None;
        self.preview = None;
        self.brightness_preview = None;
        self.oem_preview = None;
        self.fan_preview = None;
        self.invalidate_cpu_power_preview();
        self.invalidate_windows_fn_preview();
        if self.error.is_none() {
            self.error = Some(ControlError::new(
                "worker_stopped",
                "后台检测已退出；请关闭后重新打开控制台。",
            ));
        }
    }

    pub fn matching_models<'a>(&'a self, query: &str) -> Vec<&'a CatalogModel> {
        let query = query.trim().to_lowercase();
        self.catalog
            .iter()
            .filter(|model| {
                [
                    model.id.as_str(),
                    model.name.as_str(),
                    model.slug.as_str(),
                    model.category.as_deref().unwrap_or(""),
                    model.series.as_deref().unwrap_or(""),
                ]
                .iter()
                .any(|part| part.to_lowercase().contains(&query))
            })
            .collect()
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        self.requests.take();
        // Native UI defers close while write_in_progress(). Read-only operations
        // may detach: a process exit can safely terminate an unfinished read.
        // Core separately bounds subprocess execution.
        if let Some(worker) = self.worker.take() {
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

pub fn reading_text(reading: &SensorReading) -> String {
    match reading.value.filter(|value| value.is_finite()) {
        Some(value) if reading.status == CapabilityStatus::Available => {
            format!("{value:.1} {}", reading.unit)
        }
        _ => "不可用".into(),
    }
}

fn oem_value_in_domain(domain: &OemDomain, value: &OemValue) -> bool {
    match (domain, value) {
        (OemDomain::Boolean, OemValue::Boolean(_)) | (OemDomain::Rgb, OemValue::Rgb(_)) => true,
        (OemDomain::Choices(choices), OemValue::Choice(value)) => choices.contains(value),
        (OemDomain::Level { max }, OemValue::Level(value)) => value <= max,
        _ => false,
    }
}

// @mark mechrevo.console_ui role=verification
#[cfg(test)]
mod tests {
    use super::*;
    use mechrevo_console_core::{
        brightness::BrightnessBackend, BrightnessDevice, Capability, EvidenceSource, Identity,
        PowerPolicy,
    };
    use std::sync::atomic::AtomicUsize;

    pub(crate) fn fixture_snapshot() -> Snapshot {
        Snapshot {
            platform: "linux".into(),
            identity: Identity {
                product_name: Some("TEST FIXTURE / 非真实硬件".into()),
                ..Default::default()
            },
            telemetry: vec![],
            capabilities: vec![Capability {
                id: "power_profile".into(),
                label: "System power policy".into(),
                status: CapabilityStatus::Available,
                reason: "fixture".into(),
                source: None,
                options: vec!["balanced".into(), "performance".into()],
            }],
            power_policy: PowerPolicy {
                scope: "OS test fixture".into(),
                current: Some("balanced".into()),
                options: vec!["balanced".into(), "performance".into(), "low-power".into()],
                source: None,
            },
            brightness: vec![BrightnessDevice {
                id: "fixture-display".into(),
                label: "TEST DISPLAY / 非真实硬件".into(),
                current_percent: Some(50.0),
                control_value: Some(50),
                actual_value: Some(50),
                maximum: Some(100),
                source: EvidenceSource {
                    backend: "fixture".into(),
                    path: "display".into(),
                    device_id: Some("fixture-display".into()),
                },
                status: CapabilityStatus::Available,
                readable: true,
                writable: true,
                write_backend: Some(BrightnessBackend::LinuxSysfs),
                logind_session: None,
                supported_percentages: vec![],
                reason: None,
            }],
            diagnostics: vec![],
            oem_controls: vec![],
        }
    }

    struct FixtureBackend {
        reads: Arc<AtomicUsize>,
        apply_error: bool,
        panic: bool,
    }

    impl Backend for FixtureBackend {
        fn snapshot(&mut self) -> Snapshot {
            self.reads.fetch_add(1, Ordering::Relaxed);
            assert!(!self.panic, "fixture worker panic");
            fixture_snapshot()
        }
        fn catalog(&mut self) -> Vec<CatalogModel> {
            vec![]
        }
        fn plan(&mut self, profile: PowerProfile) -> Result<ProfilePlan, ControlError> {
            Ok(ProfilePlan {
                requested: profile,
                target: profile.to_string(),
                previous: "balanced".into(),
                scope: "OS fixture".into(),
                source: EvidenceSource {
                    backend: "test".into(),
                    path: "fixture".into(),
                    device_id: None,
                },
            })
        }
        fn apply(&mut self, plan: &ProfilePlan) -> Result<ApplyResult, ControlError> {
            if self.apply_error {
                return Err(ControlError::new(
                    "access_denied",
                    "fixture permission denied",
                ));
            }
            Ok(ApplyResult {
                requested: plan.requested,
                confirmed: plan.target.clone(),
                previous: plan.previous.clone(),
                scope: plan.scope.clone(),
            })
        }
        fn plan_brightness(
            &mut self,
            percent: u8,
            device: &str,
        ) -> Result<BrightnessPlan, ControlError> {
            Ok(BrightnessPlan {
                device_id: device.into(),
                requested_percent: percent,
                target: u32::from(percent),
                previous: 50,
                maximum: 100,
                source: EvidenceSource {
                    backend: "fixture".into(),
                    path: "display".into(),
                    device_id: Some(device.into()),
                },
                backend: BrightnessBackend::LinuxSysfs,
                logind_session: None,
            })
        }
        fn apply_brightness(
            &mut self,
            plan: &BrightnessPlan,
        ) -> Result<BrightnessApplyResult, ControlError> {
            if self.apply_error {
                return Err(ControlError::new(
                    "brightness_readback_mismatch",
                    "fixture readback mismatch; rollback confirmed",
                ));
            }
            Ok(BrightnessApplyResult {
                device_id: plan.device_id.clone(),
                requested_percent: plan.requested_percent,
                confirmed: plan.target,
                previous: plan.previous,
                actual_value: Some(plan.target),
                source: plan.source.clone(),
                transaction: BrightnessTransaction {
                    outcome: BrightnessOutcome::Confirmed,
                    write_attempted: true,
                    writer_may_continue: false,
                    observed_control: Some(plan.target),
                    primary_error: None,
                    rollback_error: None,
                    readback_error: None,
                    recovery: BrightnessRecovery::NotNeeded,
                    recovery_reason: None,
                },
            })
        }
    }

    fn controller() -> Controller {
        Controller::new(FixtureBackend {
            reads: Arc::new(AtomicUsize::new(0)),
            apply_error: false,
            panic: false,
        })
        .unwrap()
    }

    pub(crate) fn finish(controller: &mut Controller) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while controller.busy() && Instant::now() < deadline {
            controller.poll();
            thread::yield_now();
        }
        assert!(
            !controller.busy(),
            "worker failed to finish within test deadline"
        );
    }

    #[test]
    fn startup_does_not_read_or_apply_hardware() {
        let reads = Arc::new(AtomicUsize::new(0));
        let mut controller = Controller::new(FixtureBackend {
            reads: reads.clone(),
            apply_error: false,
            panic: false,
        })
        .unwrap();
        assert_eq!(reads.load(Ordering::Relaxed), 0);
        assert_eq!(controller.apply_preview(), Err(SubmitError::Unsupported));
        controller.refresh().unwrap();
        finish(&mut controller);
        assert_eq!(reads.load(Ordering::Relaxed), 1);
        assert!(controller.snapshot.is_some());
    }

    #[test]
    fn repeated_refresh_is_single_in_flight_and_nonblocking() {
        let mut controller = controller();
        controller.refresh().unwrap();
        for _ in 0..1000 {
            assert_eq!(controller.refresh(), Err(SubmitError::Busy));
        }
        finish(&mut controller);
        assert!(controller.snapshot.is_some());
    }

    #[test]
    fn missing_driver_is_explained_and_cannot_be_applied() {
        let mut controller = controller();
        let mut snapshot = fixture_snapshot();
        snapshot.capabilities[0].status = CapabilityStatus::Unavailable;
        snapshot.capabilities[0].reason = "power-profiles-daemon is absent".into();
        controller.snapshot = Some(snapshot);
        assert!(controller
            .policy_unavailable_reason()
            .unwrap()
            .contains("absent"));
        assert_eq!(controller.preview_profile(), Err(SubmitError::Unsupported));
        assert_eq!(controller.apply_preview(), Err(SubmitError::Unsupported));
        assert!(!controller.busy());
    }

    #[test]
    fn apply_requires_preview_and_success_has_readback_and_updated_snapshot() {
        let mut controller = controller();
        controller.refresh().unwrap();
        finish(&mut controller);
        assert_eq!(
            controller.apply_preview(),
            Err(SubmitError::PreviewRequired)
        );
        controller.select_profile(PowerProfile::Performance);
        controller.preview_profile().unwrap();
        finish(&mut controller);
        assert_eq!(
            controller.preview.as_ref().unwrap().requested,
            PowerProfile::Performance
        );
        controller.apply_preview().unwrap();
        assert!(controller.preview.is_none());
        finish(&mut controller);
        assert_eq!(
            controller.applied.as_ref().unwrap().confirmed,
            "performance"
        );
        assert!(controller.snapshot.is_some());
        assert_eq!(
            controller.apply_preview(),
            Err(SubmitError::PreviewRequired)
        );
    }

    #[test]
    fn permission_error_is_visible_and_preview_cannot_be_replayed() {
        let mut controller = Controller::new(FixtureBackend {
            reads: Arc::new(AtomicUsize::new(0)),
            apply_error: true,
            panic: false,
        })
        .unwrap();
        controller.refresh().unwrap();
        finish(&mut controller);
        controller.preview_profile().unwrap();
        finish(&mut controller);
        controller.apply_preview().unwrap();
        finish(&mut controller);
        assert_eq!(controller.error.as_ref().unwrap().code, "access_denied");
        assert!(controller.applied.is_none());
        assert!(controller.preview.is_none());
    }

    #[test]
    fn changed_selection_and_refresh_invalidate_plans() {
        let mut controller = controller();
        controller.refresh().unwrap();
        finish(&mut controller);
        controller.preview_profile().unwrap();
        controller.select_profile(PowerProfile::PowerSaver);
        finish(&mut controller);
        assert!(
            controller.preview.is_none(),
            "late response must not revive an old selection"
        );
        controller.preview_profile().unwrap();
        finish(&mut controller);
        assert!(controller.preview.is_some());
        controller.refresh().unwrap();
        assert!(controller.preview.is_none());
        finish(&mut controller);
    }

    #[test]
    fn worker_panic_clears_loading_and_reports_restart() {
        let mut controller = Controller::new(FixtureBackend {
            reads: Arc::new(AtomicUsize::new(0)),
            apply_error: false,
            panic: true,
        })
        .unwrap();
        controller.refresh().unwrap();
        finish(&mut controller);
        assert_eq!(controller.error.as_ref().unwrap().code, "worker_panicked");
        let deadline = Instant::now() + Duration::from_secs(2);
        while controller.worker_available && Instant::now() < deadline {
            controller.poll();
            thread::yield_now();
        }
        assert!(!controller.worker_available);
        assert_eq!(controller.refresh(), Err(SubmitError::WorkerStopped));
    }

    #[test]
    fn closing_a_delayed_backend_does_not_wait_for_it() {
        struct Delayed {
            entered: SyncSender<()>,
            release: Receiver<()>,
        }
        impl Backend for Delayed {
            fn snapshot(&mut self) -> Snapshot {
                self.entered.send(()).unwrap();
                let _ = self.release.recv();
                fixture_snapshot()
            }
            fn catalog(&mut self) -> Vec<CatalogModel> {
                vec![]
            }
            fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
                unreachable!()
            }
            fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
                unreachable!()
            }
        }
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, delayed) = mpsc::sync_channel(1);
        let mut controller = Controller::new(Delayed {
            entered,
            release: delayed,
        })
        .unwrap();
        controller.refresh().unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        let before = Instant::now();
        drop(controller);
        assert!(before.elapsed() < Duration::from_millis(100));
        release.send(()).unwrap();
    }

    #[test]
    fn absent_invalid_and_unsupported_values_never_become_zero() {
        let mut reading = SensorReading {
            id: "fan0".into(),
            label: "Fan".into(),
            device_id: "gpu0".into(),
            kind: "fan".into(),
            unit: "RPM".into(),
            value: None,
            source: EvidenceSource {
                backend: "fixture".into(),
                path: "none".into(),
                device_id: Some("gpu0".into()),
            },
            status: CapabilityStatus::Unavailable,
            reason: Some("missing".into()),
        };
        assert_eq!(reading_text(&reading), "不可用");
        reading.value = Some(0.0);
        assert_eq!(reading_text(&reading), "不可用");
        reading.status = CapabilityStatus::Available;
        assert_eq!(reading_text(&reading), "0.0 RPM");
        reading.value = Some(f64::NAN);
        assert_eq!(reading_text(&reading), "不可用");
    }

    #[test]
    fn display_mode_persists_but_no_hardware_plan_is_serialized() {
        let preferences = Preferences {
            display_mode: DisplayMode::Compact,
            theme: Theme::Dark,
            text_scale: 1.5,
            auto_refresh: true,
        };
        let encoded = serde_json::to_string(&preferences).unwrap();
        let decoded: Preferences = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.display_mode, DisplayMode::Compact);
        assert_eq!(decoded.theme, Theme::Dark);
        assert_eq!(decoded.text_scale, 1.5);
        assert!(!encoded.contains("plan"));
        let mut old: Preferences = serde_json::from_str("{}").unwrap();
        old.text_scale = f32::INFINITY;
        old.sanitize();
        assert_eq!(old.text_scale, 1.0);
    }

    #[test]
    fn brightness_success_is_single_use_and_has_separate_request_and_actual_readback() {
        let mut controller = controller();
        controller.refresh().unwrap();
        finish(&mut controller);
        assert_eq!(
            controller.selected_brightness_device.as_deref(),
            Some("fixture-display")
        );
        assert_eq!(
            controller.apply_brightness_preview(),
            Err(SubmitError::PreviewRequired)
        );
        controller.set_brightness_target(80);
        controller.preview_brightness().unwrap();
        finish(&mut controller);
        let plan = controller.brightness_preview.as_ref().unwrap();
        assert_eq!((plan.previous, plan.target), (50, 80));
        controller.apply_brightness_preview().unwrap();
        finish(&mut controller);
        let applied = controller.brightness_applied.as_ref().unwrap();
        assert_eq!(applied.confirmed, 80);
        assert_eq!(applied.actual_value, Some(80));
        assert!(controller.brightness_preview.is_none());
        assert_eq!(
            controller.apply_brightness_preview(),
            Err(SubmitError::PreviewRequired)
        );
    }

    #[test]
    fn brightness_target_changes_and_supported_levels_reject_wrong_or_late_plan() {
        let mut controller = controller();
        controller.refresh().unwrap();
        finish(&mut controller);
        controller.set_brightness_target(80);
        controller.preview_brightness().unwrap();
        controller.set_brightness_target(60);
        finish(&mut controller);
        assert!(controller.brightness_preview.is_none());
        controller.snapshot.as_mut().unwrap().brightness[0].supported_percentages =
            vec![0, 50, 100];
        assert_eq!(
            controller.preview_brightness(),
            Err(SubmitError::Unsupported)
        );
        controller.set_brightness_target(100);
        assert!(controller.brightness_unavailable_reason().is_none());
        controller.snapshot.as_mut().unwrap().brightness[0].writable = false;
        controller.snapshot.as_mut().unwrap().brightness[0].write_backend = None;
        assert_eq!(
            controller.preview_brightness(),
            Err(SubmitError::Unsupported)
        );
        assert!(!controller.busy());
    }

    fn oem_control() -> OemControl {
        OemControl {
            id: "fixture-fn-lock".into(),
            kind: mechrevo_console_core::OemControlKind::FnLock,
            label: "TEST Fn Lock / 非真实硬件".into(),
            current: Some(OemValue::Boolean(false)),
            domain: OemDomain::Boolean,
            status: CapabilityStatus::Available,
            // Root-owned OEM attributes still support the reviewed helper.
            writable: false,
            source: EvidenceSource {
                backend: "fixture-only".into(),
                path: "fixture://fn-lock".into(),
                device_id: None,
            },
            binding: "fixture-identity".into(),
            readback_scope: "fixture driver request; not physical measurement".into(),
            reason: None,
        }
    }

    struct OemFixtureBackend {
        control: OemControl,
        writes: Arc<AtomicUsize>,
        reads: Arc<AtomicUsize>,
        failure: Option<&'static str>,
        apply_entered: Option<SyncSender<()>>,
        apply_release: Option<Receiver<()>>,
    }
    impl Backend for OemFixtureBackend {
        fn snapshot(&mut self) -> Snapshot {
            self.reads.fetch_add(1, Ordering::Relaxed);
            let mut snapshot = fixture_snapshot();
            snapshot.oem_controls = vec![self.control.clone()];
            snapshot
        }
        fn catalog(&mut self) -> Vec<CatalogModel> {
            vec![]
        }
        fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
            unreachable!("OEM fixture never changes system power policy")
        }
        fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
            unreachable!("OEM fixture never writes system power policy")
        }
        fn plan_oem(&mut self, id: &str, target: OemValue) -> Result<OemPlan, ControlError> {
            assert_eq!(id, self.control.id);
            assert!(oem_value_in_domain(&self.control.domain, &target));
            Ok(OemPlan {
                control_id: id.into(),
                kind: self.control.kind,
                label: self.control.label.clone(),
                previous: self.control.current.clone().unwrap(),
                target,
                domain: self.control.domain.clone(),
                source: self.control.source.clone(),
                binding: self.control.binding.clone(),
                readback_scope: self.control.readback_scope.clone(),
                requires_privilege: true,
                companion_brightness: None,
            })
        }
        fn apply_oem(&mut self, plan: &OemPlan) -> Result<OemApplyResult, ControlError> {
            if let Some(entered) = self.apply_entered.take() {
                entered.send(()).unwrap();
            }
            if let Some(release) = self.apply_release.take() {
                release.recv().unwrap();
            }
            self.writes.fetch_add(1, Ordering::Relaxed);
            if let Some(code) = self.failure {
                let mut error = ControlError::new(code, "fixture OEM operation failed");
                error.details =
                    vec!["fixture rollback/readback evidence must remain visible".into()];
                return Err(error);
            }
            self.control.current = Some(plan.target.clone());
            Ok(OemApplyResult {
                control_id: plan.control_id.clone(),
                kind: plan.kind,
                previous: plan.previous.clone(),
                confirmed: plan.target.clone(),
                source: plan.source.clone(),
                readback_scope: plan.readback_scope.clone(),
            })
        }
    }
    fn oem_controller(
        failure: Option<&'static str>,
    ) -> (Controller, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let writes = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let controller = Controller::new(OemFixtureBackend {
            control: oem_control(),
            writes: writes.clone(),
            reads: reads.clone(),
            failure,
            apply_entered: None,
            apply_release: None,
        })
        .unwrap();
        (controller, writes, reads)
    }
    pub(crate) fn prepare_oem(controller: &mut Controller) {
        controller.refresh().unwrap();
        finish(controller);
        controller.select_oem_control("fixture-fn-lock");
        controller.set_oem_target(OemValue::Boolean(true));
    }
    pub(crate) fn gated_oem_controller(
        failure: Option<&'static str>,
    ) -> (Controller, Receiver<()>, SyncSender<()>) {
        let (entered, ready) = mpsc::sync_channel(1);
        let (release, delayed) = mpsc::sync_channel(1);
        let controller = Controller::new(OemFixtureBackend {
            control: oem_control(),
            writes: Arc::new(AtomicUsize::new(0)),
            reads: Arc::new(AtomicUsize::new(0)),
            failure,
            apply_entered: Some(entered),
            apply_release: Some(delayed),
        })
        .unwrap();
        (controller, ready, release)
    }

    #[test]
    fn oem_success_requires_single_use_preview_and_refreshes_actual_snapshot() {
        let (mut controller, writes, reads) = oem_controller(None);
        prepare_oem(&mut controller);
        assert_eq!(
            controller.apply_oem_preview(),
            Err(SubmitError::PreviewRequired)
        );
        controller.preview_oem().unwrap();
        finish(&mut controller);
        assert_eq!(
            writes.load(Ordering::Relaxed),
            0,
            "preview must not authorize or write"
        );
        assert!(controller.oem_preview.as_ref().unwrap().requires_privilege);
        assert!(controller.has_pending_preview());
        controller.apply_oem_preview().unwrap();
        assert!(controller.write_in_progress());
        assert!(!controller.has_pending_preview());
        finish(&mut controller);
        assert_eq!(writes.load(Ordering::Relaxed), 1);
        assert_eq!(reads.load(Ordering::Relaxed), 2);
        assert_eq!(
            controller.oem_applied.as_ref().unwrap().confirmed,
            OemValue::Boolean(true)
        );
        assert_eq!(
            controller.selected_oem().unwrap().current,
            Some(OemValue::Boolean(true))
        );
        assert!(!controller.write_in_progress());
        assert_eq!(
            controller.apply_oem_preview(),
            Err(SubmitError::PreviewRequired)
        );
    }

    #[test]
    fn oem_missing_unavailable_unknown_and_out_of_domain_never_reach_worker() {
        let (mut controller, writes, _) = oem_controller(None);
        assert_eq!(controller.preview_oem(), Err(SubmitError::Unsupported));
        prepare_oem(&mut controller);
        for status in [CapabilityStatus::Unavailable, CapabilityStatus::Unsupported] {
            controller.snapshot.as_mut().unwrap().oem_controls[0].status = status;
            assert_eq!(controller.preview_oem(), Err(SubmitError::Unsupported));
        }
        let control = &mut controller.snapshot.as_mut().unwrap().oem_controls[0];
        control.status = CapabilityStatus::Available;
        control.current = None;
        assert_eq!(controller.preview_oem(), Err(SubmitError::Unsupported));
        controller.snapshot.as_mut().unwrap().oem_controls[0].current =
            Some(OemValue::Boolean(false));
        for (domain, target) in [
            (OemDomain::Boolean, OemValue::Level(1)),
            (
                OemDomain::Choices(vec!["balanced".into()]),
                OemValue::Choice("made-up".into()),
            ),
            (OemDomain::Level { max: 3 }, OemValue::Level(4)),
            (OemDomain::Rgb, OemValue::Boolean(true)),
        ] {
            let current = match &domain {
                OemDomain::Boolean => OemValue::Boolean(false),
                OemDomain::Choices(choices) => OemValue::Choice(choices[0].clone()),
                OemDomain::Level { .. } => OemValue::Level(0),
                OemDomain::Rgb => OemValue::Rgb([0, 0, 0]),
            };
            controller.snapshot.as_mut().unwrap().oem_controls[0].current = Some(current);
            controller.snapshot.as_mut().unwrap().oem_controls[0].domain = domain;
            controller.set_oem_target(target);
            assert_eq!(controller.preview_oem(), Err(SubmitError::Unsupported));
        }
        assert_eq!(writes.load(Ordering::Relaxed), 0);
        assert!(!controller.busy());
    }

    #[test]
    fn oem_default_backend_reports_unsupported_without_manufactured_success() {
        let mut controller = controller();
        let mut snapshot = fixture_snapshot();
        snapshot.oem_controls = vec![oem_control()];
        controller.snapshot = Some(snapshot);
        controller.select_oem_control("fixture-fn-lock");
        controller.set_oem_target(OemValue::Boolean(true));
        controller.preview_oem().unwrap();
        finish(&mut controller);
        assert_eq!(controller.error.as_ref().unwrap().code, "oem_unavailable");
        assert!(controller.oem_preview.is_none());
        assert!(controller.oem_applied.is_none());
    }

    #[test]
    fn oem_failed_apply_preserves_recovery_details_and_cannot_replay_preview() {
        let (mut controller, writes, reads) = oem_controller(Some("oem_readback_mismatch"));
        prepare_oem(&mut controller);
        controller.preview_oem().unwrap();
        finish(&mut controller);
        controller.apply_oem_preview().unwrap();
        finish(&mut controller);
        let error = controller.error.as_ref().unwrap();
        assert_eq!(error.code, "oem_readback_mismatch");
        assert!(error.details[0].contains("rollback/readback"));
        assert_eq!(writes.load(Ordering::Relaxed), 1);
        assert_eq!(
            reads.load(Ordering::Relaxed),
            1,
            "failed apply is not a confirmed snapshot"
        );
        assert!(controller.oem_applied.is_none());
        assert_eq!(
            controller.apply_oem_preview(),
            Err(SubmitError::PreviewRequired)
        );
    }

    #[test]
    fn oem_late_a_to_b_to_a_and_cross_control_results_cannot_revive_old_plan() {
        let (mut controller, writes, _) = oem_controller(None);
        prepare_oem(&mut controller);
        controller.preview_oem().unwrap();
        controller.set_oem_target(OemValue::Boolean(false));
        controller.set_oem_target(OemValue::Boolean(true));
        finish(&mut controller);
        assert!(
            controller.oem_preview.is_none(),
            "matching value does not cancel request revisions"
        );
        controller.preview_oem().unwrap();
        controller.select_oem_control("different-control");
        controller.select_oem_control("fixture-fn-lock");
        controller.set_oem_target(OemValue::Boolean(true));
        finish(&mut controller);
        assert!(controller.oem_preview.is_none());
        assert_eq!(writes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn oem_other_requests_refresh_and_changed_binding_invalidate_preview() {
        let (mut controller, writes, _) = oem_controller(None);
        prepare_oem(&mut controller);
        controller.preview_oem().unwrap();
        finish(&mut controller);
        controller.select_profile(PowerProfile::Performance);
        assert!(controller.oem_preview.is_none());
        controller.preview_oem().unwrap();
        finish(&mut controller);
        controller.set_brightness_target(42);
        assert!(controller.oem_preview.is_none());
        controller.preview_oem().unwrap();
        finish(&mut controller);
        controller.snapshot.as_mut().unwrap().oem_controls[0].binding = "replaced-device".into();
        assert_eq!(
            controller.apply_oem_preview(),
            Err(SubmitError::PreviewRequired)
        );
        assert!(controller.oem_preview.is_none());
        controller.refresh().unwrap();
        finish(&mut controller);
        assert_eq!(writes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn oem_same_id_recovery_initializes_absent_target_without_overwriting_user_choice() {
        let (mut controller, _, _) = oem_controller(None);
        let mut snapshot = fixture_snapshot();
        let mut control = oem_control();
        control.status = CapabilityStatus::Unavailable;
        control.current = None;
        snapshot.oem_controls = vec![control];
        controller.snapshot = Some(snapshot);
        controller.select_oem_control("fixture-fn-lock");
        assert!(controller.requested_oem_value.is_none());
        controller.refresh().unwrap();
        finish(&mut controller);
        assert_eq!(
            controller.requested_oem_value,
            Some(OemValue::Boolean(false))
        );
        controller.set_oem_target(OemValue::Boolean(true));
        controller.refresh().unwrap();
        finish(&mut controller);
        assert_eq!(
            controller.requested_oem_value,
            Some(OemValue::Boolean(true))
        );
        controller.preview_oem().unwrap();
        finish(&mut controller);
        assert!(controller.oem_preview.is_some());
    }

    #[test]
    fn oem_fresh_preview_of_changed_device_state_explains_required_refresh() {
        let (mut controller, writes, _) = oem_controller(None);
        prepare_oem(&mut controller);
        controller.snapshot.as_mut().unwrap().oem_controls[0].current =
            Some(OemValue::Boolean(true));
        controller.preview_oem().unwrap();
        finish(&mut controller);
        assert!(controller.oem_preview.is_none());
        assert_eq!(controller.error.as_ref().unwrap().code, "oem_preview_stale");
        assert_eq!(writes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn oem_busy_write_owns_single_worker_until_result_finishes() {
        let (mut controller, ready, release) = gated_oem_controller(None);
        prepare_oem(&mut controller);
        controller.preview_oem().unwrap();
        finish(&mut controller);
        controller.apply_oem_preview().unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(controller.busy() && controller.write_in_progress());
        assert_eq!(controller.refresh(), Err(SubmitError::Busy));
        assert_eq!(controller.preview_oem(), Err(SubmitError::Busy));
        assert!(
            controller.write_in_progress(),
            "native close must remain deferred for OEM writes"
        );
        release.send(()).unwrap();
        finish(&mut controller);
        assert!(!controller.write_in_progress());
    }

    #[test]
    fn oem_unknown_helper_liveness_keeps_write_and_close_interlock() {
        let (mut controller, _, _) = oem_controller(Some("oem_helper_liveness_unknown"));
        prepare_oem(&mut controller);
        controller.preview_oem().unwrap();
        finish(&mut controller);
        controller.apply_oem_preview().unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while controller.error.is_none() && Instant::now() < deadline {
            controller.poll();
            thread::yield_now();
        }
        assert_eq!(
            controller.error.as_ref().unwrap().code,
            "oem_helper_liveness_unknown"
        );
        assert!(controller.busy() && controller.write_in_progress());
        assert_eq!(controller.refresh(), Err(SubmitError::Busy));
        assert!(controller.oem_preview.is_none());
        assert!(controller.oem_applied.is_none());
    }

    #[test]
    fn brightness_readback_failure_keeps_the_error_and_invalidates_the_plan() {
        let mut controller = Controller::new(FixtureBackend {
            reads: Arc::new(AtomicUsize::new(0)),
            apply_error: true,
            panic: false,
        })
        .unwrap();
        controller.refresh().unwrap();
        finish(&mut controller);
        controller.preview_brightness().unwrap();
        finish(&mut controller);
        controller.apply_brightness_preview().unwrap();
        finish(&mut controller);
        assert_eq!(
            controller.error.as_ref().unwrap().code,
            "brightness_readback_mismatch"
        );
        assert!(controller.brightness_applied.is_none());
        assert!(controller.brightness_preview.is_none());
        assert_eq!(
            controller.apply_brightness_preview(),
            Err(SubmitError::PreviewRequired)
        );
    }

    #[test]
    fn brightness_target_starts_from_requested_value_when_actual_lags() {
        let mut controller = controller();
        let mut snapshot = fixture_snapshot();
        let device = &mut snapshot.brightness[0];
        device.control_value = Some(250);
        device.actual_value = Some(200);
        device.current_percent = Some(20.0);
        device.maximum = Some(1000);
        controller.snapshot = Some(snapshot);
        controller.select_brightness_device("fixture-display");
        assert_eq!(controller.requested_brightness, 25);
    }

    #[test]
    fn idle_worker_exits_after_controller_closes() {
        struct Idle {
            stopped: SyncSender<()>,
        }
        impl Drop for Idle {
            fn drop(&mut self) {
                let _ = self.stopped.send(());
            }
        }
        impl Backend for Idle {
            fn snapshot(&mut self) -> Snapshot {
                fixture_snapshot()
            }
            fn catalog(&mut self) -> Vec<CatalogModel> {
                vec![]
            }
            fn plan(&mut self, _: PowerProfile) -> Result<ProfilePlan, ControlError> {
                unreachable!()
            }
            fn apply(&mut self, _: &ProfilePlan) -> Result<ApplyResult, ControlError> {
                unreachable!()
            }
        }
        let (stopped, done) = mpsc::sync_channel(1);
        let controller = Controller::new(Idle { stopped }).unwrap();
        drop(controller);
        done.recv_timeout(Duration::from_secs(2))
            .expect("idle worker should release the backend after its sender closes");
    }
}
