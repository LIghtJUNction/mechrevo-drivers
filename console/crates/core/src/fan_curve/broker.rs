//! Bounded JSONL protocol and persistent, explicitly acquired fan helper.
//! Raw tables are response-only evidence, never a restoration input.

use super::FanCurve;
#[cfg(any(target_os = "linux", test))]
use super::ValidatedFanCurve;
use crate::ControlError;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 16_384;
pub const MAX_RESPONSE_BYTES: usize = 32_768;
pub const HEARTBEAT_TIMEOUT_MS: u64 = 20_000;
pub const OUTPUT_TIMEOUT_MS: u64 = 2_000;
pub const HELPER_PATH: &str = "/usr/lib/mechrevo-console/fan-helper";
pub const BACKEND: &str = "uniwill_fan_curve_ioctl_v1";
pub const DEVICE_PATH: &str = "/dev/tuxedo_io";

// @mark mechrevo.fan_curve role=protocol contract=bounded_session_requests
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerRequest {
    pub schema_version: u32,
    pub request_id: u64,
    pub session_id: String,
    pub command: BrokerCommand,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrokerCommand {
    Apply { curve: FanCurve },
    Inspect,
    Ping,
    Restore,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerOutcome {
    Applied,
    Inspected,
    Pong,
    Restored,
    Released,
    Shutdown,
    Rejected,
    Failed,
    Terminated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerPhase {
    Idle,
    Acquired,
    Active,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerState {
    pub backend: String,
    pub device_path: String,
    pub epoch: u64,
    pub generation: u64,
    pub lease_id: u64,
    pub last_request_id: u64,
    pub phase: BrokerPhase,
    pub capabilities: u32,
    pub owner: bool,
    pub revoked: bool,
    pub modified_or_uncertain: bool,
    pub state_is_current: bool,
    pub restored_verified: bool,
    pub live_error: i32,
    pub last_error: i32,
    pub restore_error: i32,
    /// Exactly 96 bytes when available; these are read-only diagnostics.
    pub live_table: Option<Vec<u8>>,
    pub original_table: Option<Vec<u8>>,
    pub live_controls: Option<[u8; 6]>,
    pub original_controls: Option<[u8; 6]>,
    /// Cached driver commands, not RPM.
    pub cached_original_targets: Option<[u8; 4]>,
    pub applied_curve: Option<FanCurve>,
}

impl BrokerState {
    #[cfg(target_os = "linux")]
    pub fn from_linux(
        state: &super::linux::FanCurveState,
        state_is_current: bool,
        modified_or_uncertain: bool,
        applied_curve: Option<FanCurve>,
    ) -> Self {
        use super::linux::FanCurvePhase;
        let mut result = Self {
            backend: BACKEND.into(),
            device_path: DEVICE_PATH.into(),
            epoch: state.token.epoch,
            generation: state.token.generation,
            lease_id: state.token.lease_id,
            last_request_id: state.last_request_id,
            phase: match state.phase {
                FanCurvePhase::Idle => BrokerPhase::Idle,
                FanCurvePhase::Acquired => BrokerPhase::Acquired,
                FanCurvePhase::Active => BrokerPhase::Active,
                FanCurvePhase::RecoveryRequired => BrokerPhase::RecoveryRequired,
            },
            capabilities: state.capabilities,
            owner: state.owner,
            revoked: state.revoked,
            modified_or_uncertain: modified_or_uncertain || state.applied,
            state_is_current,
            restored_verified: state.restored_verified,
            live_error: state.live_error,
            last_error: state.last_error,
            restore_error: state.restore_error,
            live_table: state
                .live_table
                .as_ref()
                .map(|table| table.as_bytes().to_vec()),
            original_table: state
                .original_table
                .as_ref()
                .map(|table| table.as_bytes().to_vec()),
            live_controls: state.live_controls,
            original_controls: state.original_controls,
            cached_original_targets: state.cached_original_targets,
            applied_curve: None,
        };
        if let Some(curve) = applied_curve {
            if result.state_is_current
                && result.owner
                && result.phase == BrokerPhase::Active
                && !result.revoked
                && curve_matches_state(&curve, &result)
            {
                result.applied_curve = Some(curve);
            }
        }
        result
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerError {
    pub code: String,
    pub message: String,
    pub operation: Option<String>,
    pub errno: Option<i32>,
    pub state: Option<Box<BrokerState>>,
    pub state_is_current: bool,
    pub mutation_may_have_occurred: bool,
    pub state_query_error: Option<String>,
    pub state_query_errno: Option<i32>,
    pub invalid_response: Option<Vec<u8>>,
    pub state_query_invalid_response: Option<Vec<u8>>,
}

impl BrokerError {
    #[cfg(any(target_os = "linux", test))]
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: bounded_text(message.into(), 2048),
            operation: None,
            errno: None,
            state: None,
            state_is_current: false,
            mutation_may_have_occurred: false,
            state_query_error: None,
            state_query_errno: None,
            invalid_response: None,
            state_query_invalid_response: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrokerResponse {
    pub schema_version: u32,
    /// Zero denotes an unsolicited terminal protocol/transport failure.
    pub request_id: u64,
    pub session_id: String,
    pub outcome: BrokerOutcome,
    pub state: Option<BrokerState>,
    pub error: Option<Box<BrokerError>>,
    pub recovery_error: Option<Box<BrokerError>>,
    pub terminal: bool,
    /// This request's explicit restore was verified; not a historical flag.
    pub restoration_confirmed: bool,
    /// This helper never called ACQUIRE. False also covers an ambiguous ACK;
    /// global state may belong to another controller even when this is true.
    pub no_acquisition_attempted: bool,
    /// Closing a file description is never treated as verified restoration.
    pub close_fallback_unconfirmed: bool,
}

fn bounded_text(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max.saturating_sub(16);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str(" [truncated]");
    }
    text
}
fn protocol_error(message: impl Into<String>) -> ControlError {
    ControlError::new("invalid_fan_broker_protocol", message)
}
fn valid_nonce(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn frame_body(bytes: &[u8], max: usize) -> Result<&[u8], ControlError> {
    let body = if let Some(body) = bytes.strip_suffix(b"\n") {
        body.strip_suffix(b"\r").unwrap_or(body)
    } else {
        bytes
    };
    if body.is_empty() || body.len() > max || body.iter().any(|byte| matches!(byte, b'\r' | b'\n'))
    {
        return Err(protocol_error("Expected one bounded JSONL frame"));
    }
    Ok(body)
}
pub fn validate_request(request: &BrokerRequest) -> Result<(), ControlError> {
    if request.schema_version != PROTOCOL_VERSION
        || request.request_id == 0
        || !valid_nonce(&request.session_id)
    {
        return Err(protocol_error(
            "Invalid schema, request ID or 128-bit session nonce",
        ));
    }
    if let BrokerCommand::Apply { curve } = &request.command {
        curve.clone().validate()?;
    }
    Ok(())
}
pub fn parse_request(bytes: &[u8]) -> Result<BrokerRequest, ControlError> {
    let request = serde_json::from_slice(frame_body(bytes, MAX_FRAME_BYTES)?)
        .map_err(|error| protocol_error(bounded_text(error.to_string(), 2048)))?;
    validate_request(&request)?;
    Ok(request)
}
pub fn encode_request(request: &BrokerRequest) -> Result<Vec<u8>, ControlError> {
    validate_request(request)?;
    let mut bytes =
        serde_json::to_vec(request).map_err(|error| protocol_error(error.to_string()))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(protocol_error("Request exceeds the frame limit"));
    }
    bytes.push(b'\n');
    Ok(bytes)
}
fn validate_state(state: &BrokerState) -> Result<(), ControlError> {
    if state.backend != BACKEND
        || state.device_path != DEVICE_PATH
        || state.capabilities & !0x07 != 0
        || state
            .live_table
            .as_ref()
            .is_some_and(|bytes| bytes.len() != super::TABLE_BYTES)
        || state
            .original_table
            .as_ref()
            .is_some_and(|bytes| bytes.len() != super::TABLE_BYTES)
        || state.live_table.is_some() != state.live_controls.is_some()
        || state.original_table.is_some() != state.original_controls.is_some()
        || state.original_table.is_some() != state.cached_original_targets.is_some()
        || (state.live_table.is_some() && state.live_error != 0)
        || (state.owner && (state.lease_id == 0 || state.epoch == 0 || state.revoked))
        || (state.phase == BrokerPhase::Idle && (state.owner || state.lease_id != 0))
        || (state.restored_verified && (state.last_error != 0 || state.restore_error != 0))
    {
        return Err(protocol_error("Inconsistent fan state or raw table length"));
    }
    if let Some(curve) = &state.applied_curve {
        curve.clone().validate()?;
        if !state.state_is_current
            || !state.owner
            || state.phase != BrokerPhase::Active
            || !curve_matches_state(curve, state)
        {
            return Err(protocol_error(
                "An acknowledged curve lacks matching current owned readback",
            ));
        }
    }
    Ok(())
}
fn validate_error(error: &BrokerError) -> Result<(), ControlError> {
    if error.code.is_empty()
        || error.code.len() > 96
        || error.message.len() > 2048
        || error
            .operation
            .as_ref()
            .is_some_and(|value| value.len() > 64)
        || error
            .state_query_error
            .as_ref()
            .is_some_and(|value| value.len() > 2048)
        || [&error.invalid_response, &error.state_query_invalid_response]
            .iter()
            .any(|value| {
                value
                    .as_ref()
                    .is_some_and(|bytes| !matches!(bytes.len(), 64 | 320))
            })
    {
        return Err(protocol_error(
            "Unbounded or malformed broker error evidence",
        ));
    }
    if let Some(state) = &error.state {
        validate_state(state)?;
    }
    Ok(())
}
pub fn validate_response(response: &BrokerResponse) -> Result<(), ControlError> {
    if response.schema_version != PROTOCOL_VERSION
        || (response.request_id != 0 && !valid_nonce(&response.session_id))
        || (response.request_id == 0
            && (!response.terminal
                || (!response.session_id.is_empty() && !valid_nonce(&response.session_id))))
    {
        return Err(protocol_error(
            "Invalid response version, request ID or nonce",
        ));
    }
    if let Some(state) = &response.state {
        validate_state(state)?;
    }
    if let Some(error) = &response.error {
        validate_error(error)?;
    }
    if let Some(error) = &response.recovery_error {
        validate_error(error)?;
    }
    if response.outcome == BrokerOutcome::Applied
        && (response.terminal
            || response.error.is_some()
            || response.recovery_error.is_some()
            || response
                .state
                .as_ref()
                .is_none_or(|state| state.applied_curve.is_none()))
    {
        return Err(protocol_error(
            "Applied outcome requires a validated curve acknowledgement",
        ));
    }
    if response.close_fallback_unconfirmed && !response.terminal {
        return Err(protocol_error(
            "Close fallback only belongs to a terminal reply",
        ));
    }
    if response.restoration_confirmed
        && (!response.terminal
            || response.close_fallback_unconfirmed
            || response.recovery_error.is_some()
            || response.state.as_ref().is_none_or(|state| {
                !state.state_is_current
                    || !state.restored_verified
                    || state.owner
                    || state.phase != BrokerPhase::Idle
                    || state.last_error != 0
                    || state.restore_error != 0
            }))
    {
        return Err(protocol_error(
            "Current restoration receipt lacks a verified terminal release",
        ));
    }
    if response.outcome == BrokerOutcome::Restored && !response.restoration_confirmed {
        return Err(protocol_error(
            "Restored outcome requires this request's restoration receipt",
        ));
    }
    if response.no_acquisition_attempted
        && (response.restoration_confirmed
            || response.recovery_error.is_some()
            || response.state.as_ref().is_some_and(|state| state.owner)
            || response.error.as_ref().is_some_and(|error| {
                error.mutation_may_have_occurred
                    || error.state.as_ref().is_some_and(|state| state.owner)
            })
            || response.outcome == BrokerOutcome::Applied)
    {
        return Err(protocol_error(
            "No-acquisition proof contradicts ownership, mutation or restoration evidence",
        ));
    }
    match response.outcome {
        BrokerOutcome::Restored | BrokerOutcome::Released | BrokerOutcome::Shutdown => {
            if !response.terminal
                || response.error.is_some()
                || response.recovery_error.is_some()
                || response.close_fallback_unconfirmed
                || response.state.as_ref().is_none_or(|state| {
                    !state.state_is_current
                        || state.owner
                        || state.phase != BrokerPhase::Idle
                        || state.lease_id != 0
                        || state.modified_or_uncertain
                })
                || (response.outcome == BrokerOutcome::Released && response.restoration_confirmed)
            {
                return Err(protocol_error(
                    "Successful terminal outcome requires a current, unowned idle release",
                ));
            }
        }
        BrokerOutcome::Applied | BrokerOutcome::Inspected | BrokerOutcome::Pong => {
            if response.terminal
                || response.error.is_some()
                || response.recovery_error.is_some()
                || response.restoration_confirmed
            {
                return Err(protocol_error(
                    "Successful continuing outcome has terminal or error evidence",
                ));
            }
        }
        BrokerOutcome::Terminated | BrokerOutcome::Failed => {
            if !response.terminal || response.error.is_none() {
                return Err(protocol_error(
                    "Terminal failure requires an independent primary error",
                ));
            }
        }
        BrokerOutcome::Rejected => {
            if response.error.is_none() {
                return Err(protocol_error("Rejected outcome requires a reason"));
            }
        }
    }
    if response.request_id == 0 && response.error.is_none() {
        return Err(protocol_error(
            "Unsolicited terminal frame requires a primary error",
        ));
    }
    Ok(())
}
pub fn parse_response(bytes: &[u8]) -> Result<BrokerResponse, ControlError> {
    let response = serde_json::from_slice(frame_body(bytes, MAX_RESPONSE_BYTES)?)
        .map_err(|error| protocol_error(bounded_text(error.to_string(), 2048)))?;
    validate_response(&response)?;
    Ok(response)
}
pub fn encode_response(response: &BrokerResponse) -> Result<Vec<u8>, ControlError> {
    validate_response(response)?;
    let mut bytes =
        serde_json::to_vec(response).map_err(|error| protocol_error(error.to_string()))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(protocol_error("Response exceeds the frame limit"));
    }
    bytes.push(b'\n');
    Ok(bytes)
}
fn curve_matches_state(curve: &FanCurve, state: &BrokerState) -> bool {
    let (Some(bytes), Some(controls)) = (&state.live_table, state.live_controls) else {
        return false;
    };
    let Ok(bytes) = <[u8; super::TABLE_BYTES]>::try_from(bytes.as_slice()) else {
        return false;
    };
    let Ok(validated) = curve.clone().validate() else {
        return false;
    };
    let table = super::RawFanTable::from_bytes(bytes);
    table.with_curve(&validated) == table
        && (controls[0] & 0x80 != 0) == curve.respective
        && controls[1] & 0x04 != 0
        && controls[2] & 0x40 == 0
}

/// Root check belongs inside the installed helper as well as the launcher.
/// No backend, device path or environment override is accepted.
pub fn serve_stdio() -> Result<(), ControlError> {
    #[cfg(target_os = "linux")]
    {
        server::serve_linux()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Err(ControlError::new(
            "fan_broker_unsupported",
            "The fan helper backend is implemented only for Linux",
        ))
    }
}

#[cfg(any(target_os = "linux", test))]
mod server {
    use super::*;
    use std::{io, time::Duration};

    type BackendResult<T> = Result<T, Box<BrokerError>>;
    trait LeaseBackend {
        fn cached(&self) -> BrokerState;
        fn acquire(&mut self) -> BackendResult<BrokerState>;
        fn apply(&mut self, curve: &ValidatedFanCurve) -> BackendResult<BrokerState>;
        fn inspect(&mut self) -> BackendResult<BrokerState>;
        fn restore(&mut self) -> BackendResult<BrokerState>;
    }
    trait BackendFactory {
        type Session: LeaseBackend;
        fn open(&mut self) -> BackendResult<Self::Session>;
        fn inspect(&mut self) -> BackendResult<BrokerState>;
    }
    enum Input {
        Bytes(Vec<u8>),
        Eof,
        Wait,
    }
    trait Transport {
        fn now(&self) -> Duration;
        fn receive(&mut self, timeout: Duration) -> io::Result<Input>;
        fn send(&mut self, bytes: &[u8]) -> io::Result<()>;
    }
    struct Machine<F: BackendFactory> {
        factory: F,
        session: Option<F::Session>,
        nonce: Option<String>,
        last_id: u64,
        opened_once: bool,
        may_hold_lease: bool,
        last_state: Option<BrokerState>,
        restoration_confirmed: bool,
        acquisition_attempted: bool,
    }
    impl<F: BackendFactory> Machine<F> {
        fn new(factory: F) -> Self {
            Self {
                factory,
                session: None,
                nonce: None,
                last_id: 0,
                opened_once: false,
                may_hold_lease: false,
                last_state: None,
                restoration_confirmed: false,
                acquisition_attempted: false,
            }
        }
        fn response(&self, id: u64, outcome: BrokerOutcome, terminal: bool) -> BrokerResponse {
            BrokerResponse {
                schema_version: PROTOCOL_VERSION,
                request_id: id,
                session_id: self.nonce.clone().unwrap_or_default(),
                outcome,
                state: self.last_state.clone(),
                error: None,
                recovery_error: None,
                terminal,
                restoration_confirmed: self.restoration_confirmed,
                no_acquisition_attempted: !self.acquisition_attempted,
                close_fallback_unconfirmed: false,
            }
        }
        fn record_error(&mut self, error: &BrokerError) {
            if let Some(state) = &error.state {
                self.last_state = Some((**state).clone());
            } else if let Some(state) = &mut self.last_state {
                state.state_is_current = false;
                state.applied_curve = None;
            }
        }
        fn recover(&mut self) -> Option<Box<BrokerError>> {
            self.restoration_confirmed = false;
            let mut session = self.session.take()?;
            if !self.may_hold_lease {
                return None;
            }
            match session.restore() {
                Ok(state) => {
                    self.restoration_confirmed = state.restored_verified;
                    self.last_state = Some(state);
                    self.may_hold_lease = false;
                    None
                }
                Err(error) => {
                    self.record_error(&error);
                    Some(error)
                }
            }
            // session closes here; that fallback is not a verified receipt.
        }
        fn terminate(&mut self, id: u64, error: Box<BrokerError>) -> BrokerResponse {
            self.record_error(&error);
            let recovery_error = self.recover();
            let mut response = self.response(id, BrokerOutcome::Terminated, true);
            response.close_fallback_unconfirmed = recovery_error.is_some() || self.may_hold_lease;
            response.error = Some(error);
            response.recovery_error = recovery_error;
            response
        }
        fn process(&mut self, request: BrokerRequest) -> BrokerResponse {
            if let Err(error) = validate_request(&request) {
                return self.terminate(0, Box::new(BrokerError::new(&error.code, error.message)));
            }
            if request.request_id <= self.last_id
                || self
                    .nonce
                    .as_ref()
                    .is_some_and(|nonce| nonce != &request.session_id)
            {
                return self.terminate(
                    0,
                    Box::new(BrokerError::new(
                        "fan_broker_session_mismatch",
                        "Request IDs must increase and the session nonce must remain unchanged",
                    )),
                );
            }
            self.nonce.get_or_insert(request.session_id);
            self.last_id = request.request_id;
            let id = request.request_id;
            match request.command {
                BrokerCommand::Ping => {
                    let mut response = self.response(id, BrokerOutcome::Pong, false);
                    if let Some(state) = &mut response.state {
                        state.state_is_current = false;
                        state.applied_curve = None;
                    }
                    response
                }
                BrokerCommand::Inspect => {
                    let result = match &mut self.session {
                        Some(session) => session.inspect(),
                        None => self.factory.inspect(),
                    };
                    match result {
                        Ok(state) => {
                            let lost = self.may_hold_lease
                                && (!state.owner
                                    || state.revoked
                                    || state.phase == BrokerPhase::Idle);
                            self.last_state = Some(state);
                            if lost {
                                self.terminate(id, Box::new(BrokerError::new("fan_broker_lease_lost", "The fd lease is no longer owned; no reacquisition or replay was attempted")))
                            } else {
                                self.response(id, BrokerOutcome::Inspected, false)
                            }
                        }
                        Err(error) => self.terminate(id, error),
                    }
                }
                BrokerCommand::Apply { curve } => {
                    let curve = match curve.validate() {
                        Ok(curve) => curve,
                        Err(error) => {
                            return self.terminate(
                                id,
                                Box::new(BrokerError::new(&error.code, error.message)),
                            )
                        }
                    };
                    if self.session.is_none() {
                        if self.opened_once {
                            return self.terminate(id, Box::new(BrokerError::new("fan_broker_lease_closed", "A closed or lost session cannot automatically acquire another lease")));
                        }
                        self.opened_once = true;
                        let session = match self.factory.open() {
                            Ok(session) => session,
                            Err(error) => return self.terminate(id, error),
                        };
                        self.last_state = Some(session.cached());
                        self.session = Some(session);
                        if self
                            .last_state
                            .as_ref()
                            .is_none_or(|state| state.phase != BrokerPhase::Idle || state.owner)
                        {
                            return self.terminate(id, Box::new(BrokerError::new("fan_broker_recovery_or_busy", "The driver is not idle; use explicit recovery for an orphaned recovery state")));
                        }
                        self.may_hold_lease = true;
                        self.acquisition_attempted = true;
                        match self.session.as_mut().expect("session inserted").acquire() {
                            Ok(state) => self.last_state = Some(state),
                            Err(error) => return self.terminate(id, error),
                        }
                    }
                    match self
                        .session
                        .as_mut()
                        .expect("session inserted")
                        .apply(&curve)
                    {
                        Ok(mut state) => {
                            state.applied_curve = Some(curve.curve().clone());
                            if validate_state(&state).is_err() {
                                return self.terminate(
                                    id,
                                    Box::new(BrokerError::new(
                                        "fan_broker_invalid_ack",
                                        "The backend did not provide matching owned curve readback",
                                    )),
                                );
                            }
                            self.last_state = Some(state);
                            self.response(id, BrokerOutcome::Applied, false)
                        }
                        Err(error) => self.terminate(id, error),
                    }
                }
                command @ (BrokerCommand::Restore | BrokerCommand::Shutdown) => {
                    let shutdown = matches!(command, BrokerCommand::Shutdown);
                    self.finish(id, shutdown)
                }
            }
        }
        fn finish(&mut self, id: u64, shutdown: bool) -> BrokerResponse {
            if self.session.is_none() {
                let state = match self.factory.inspect() {
                    Ok(state) => state,
                    Err(error) => return self.terminate(id, error),
                };
                let orphan = state.phase == BrokerPhase::RecoveryRequired
                    && !state.owner
                    && state.lease_id == 0;
                let idle = state.phase == BrokerPhase::Idle && !state.owner && state.lease_id == 0;
                self.last_state = Some(state);
                if !idle && !orphan {
                    return self.terminate(
                        id,
                        Box::new(BrokerError::new(
                            "fan_broker_busy",
                            "Another fd owns the fan controller; recovery did not acquire it",
                        )),
                    );
                }
                if orphan {
                    let session = match self.factory.open() {
                        Ok(session) => session,
                        Err(error) => return self.terminate(id, error),
                    };
                    let state = session.cached();
                    let still_orphan = state.phase == BrokerPhase::RecoveryRequired
                        && !state.owner
                        && state.lease_id == 0;
                    self.last_state = Some(state);
                    self.session = Some(session);
                    self.opened_once = true;
                    if !still_orphan {
                        return self.terminate(id, Box::new(BrokerError::new("fan_broker_recovery_changed", "Recovery state changed before acquisition; no new idle lease was acquired")));
                    }
                    self.may_hold_lease = true;
                    self.acquisition_attempted = true;
                    match self.session.as_mut().expect("session inserted").acquire() {
                        Ok(state) => self.last_state = Some(state),
                        Err(error) => return self.terminate(id, error),
                    }
                }
            }
            let recovery_error = self.recover();
            let outcome = if shutdown {
                BrokerOutcome::Shutdown
            } else if self.restoration_confirmed {
                BrokerOutcome::Restored
            } else {
                BrokerOutcome::Released
            };
            let mut response = self.response(
                id,
                if recovery_error.is_some() {
                    BrokerOutcome::Failed
                } else {
                    outcome
                },
                true,
            );
            response.close_fallback_unconfirmed = recovery_error.is_some() || self.may_hold_lease;
            response.error = recovery_error;
            response
        }
    }

    fn write_response(
        transport: &mut impl Transport,
        response: &BrokerResponse,
    ) -> Result<(), ControlError> {
        let result = encode_response(response).and_then(|bytes| {
            transport
                .send(&bytes)
                .map_err(|error| ControlError::new("fan_broker_output_failed", error.to_string()))
        });
        result.map_err(|mut error| {
            if response.terminal {
                attach_terminal_evidence(&mut error, response);
            }
            error
        })
    }
    fn attach_terminal_evidence(error: &mut ControlError, response: &BrokerResponse) {
        if let Some(primary) = &response.error {
            error.details.push(format!(
                "primary_code={} errno={:?}",
                primary.code, primary.errno
            ));
        }
        if let Some(recovery) = &response.recovery_error {
            error.details.push(format!(
                "recovery_code={} errno={:?}",
                recovery.code, recovery.errno
            ));
        }
        error.details.push(format!(
            "restoration_confirmed={} close_fallback_unconfirmed={} no_acquisition_attempted={}",
            response.restoration_confirmed,
            response.close_fallback_unconfirmed,
            response.no_acquisition_attempted
        ));
        if let Some(state) = &response.state {
            error.details.push(format!(
                "kernel_phase={:?} owner={} current={} epoch={} generation={}",
                state.phase, state.owner, state.state_is_current, state.epoch, state.generation
            ));
        }
        error.details.push(
            serde_json::to_string(response)
                .unwrap_or_else(|_| "Recovery evidence could not be serialized".into()),
        );
    }
    fn run<F: BackendFactory>(
        factory: F,
        transport: &mut impl Transport,
    ) -> Result<(), ControlError> {
        let mut machine = Machine::new(factory);
        let mut buffer = Vec::new();
        let mut last_activity = transport.now();
        let timeout = Duration::from_millis(HEARTBEAT_TIMEOUT_MS);
        loop {
            let age = transport.now().saturating_sub(last_activity);
            if age >= timeout {
                let response = machine.terminate(
                    0,
                    Box::new(BrokerError::new(
                        "fan_broker_heartbeat_expired",
                        "No complete valid request arrived before the heartbeat deadline",
                    )),
                );
                write_response(transport, &response)?;
                return Err(ControlError::new(
                    "fan_broker_heartbeat_expired",
                    "The helper restored or reported recovery failure and exited",
                ));
            }
            let input = match transport.receive(timeout - age) {
                Ok(input) => input,
                Err(error) => {
                    let response = machine.terminate(
                        0,
                        Box::new(BrokerError::new(
                            "fan_broker_input_failed",
                            error.to_string(),
                        )),
                    );
                    write_response(transport, &response)?;
                    return Err(ControlError::new(
                        "fan_broker_input_failed",
                        error.to_string(),
                    ));
                }
            };
            match input {
                Input::Wait => continue,
                Input::Eof => {
                    let code = if buffer.is_empty() {
                        "fan_broker_eof"
                    } else {
                        "fan_broker_truncated_frame"
                    };
                    let response = machine.terminate(
                        0,
                        Box::new(BrokerError::new(
                            code,
                            "The request pipe closed before an explicit shutdown",
                        )),
                    );
                    write_response(transport, &response)?;
                    return Err(ControlError::new(
                        code,
                        "The helper stopped after explicit recovery was attempted",
                    ));
                }
                Input::Bytes(bytes) => {
                    for byte in bytes {
                        if byte != b'\n' {
                            if buffer.len() > MAX_FRAME_BYTES
                                || (buffer.len() == MAX_FRAME_BYTES && byte != b'\r')
                            {
                                let response = machine.terminate(
                                    0,
                                    Box::new(BrokerError::new(
                                        "fan_broker_frame_too_large",
                                        "Request exceeded the bounded frame limit",
                                    )),
                                );
                                write_response(transport, &response)?;
                                return Err(protocol_error("Oversized broker request"));
                            }
                            buffer.push(byte);
                            continue;
                        }
                        buffer.push(b'\n');
                        let request = match parse_request(&buffer) {
                            Ok(request) => request,
                            Err(error) => {
                                let response = machine.terminate(
                                    0,
                                    Box::new(BrokerError::new(&error.code, error.message)),
                                );
                                write_response(transport, &response)?;
                                return Err(protocol_error(
                                    "Invalid broker request; recovery was attempted",
                                ));
                            }
                        };
                        buffer.clear();
                        last_activity = transport.now();
                        let mut response = machine.process(request);
                        if !response.terminal
                            && transport.now().saturating_sub(last_activity) >= timeout
                        {
                            response = machine.terminate(0, Box::new(BrokerError::new("fan_broker_heartbeat_expired", "The synchronous kernel operation outlasted the heartbeat deadline; it could not be cancelled")));
                        }
                        if let Err(mut output_error) = write_response(transport, &response) {
                            if !response.terminal {
                                let cleanup = machine.terminate(
                                    0,
                                    Box::new(BrokerError::new(
                                        "fan_broker_output_failed",
                                        output_error.message.clone(),
                                    )),
                                );
                                attach_terminal_evidence(&mut output_error, &cleanup);
                            }
                            return Err(output_error);
                        }
                        if response.terminal {
                            if response.error.is_some() || response.recovery_error.is_some() {
                                return Err(ControlError::new("fan_broker_terminated", "The terminal response preserves the primary and recovery outcomes"));
                            }
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    mod real {
        use super::*;
        use crate::fan_curve::linux::{self, FanCurveSession, FanCurveSessionError};
        use std::time::Instant;

        fn kernel_error(error: FanCurveSessionError, modified: bool) -> Box<BrokerError> {
            let errno = error.raw_os_error();
            let message = bounded_text(error.to_string(), 2048);
            let state = error.state.as_ref().map(|state| {
                Box::new(BrokerState::from_linux(
                    state,
                    error.state_is_current,
                    modified,
                    None,
                ))
            });
            Box::new(BrokerError {
                code: "fan_kernel_operation_failed".into(),
                message,
                operation: Some(error.operation.into()),
                errno,
                state,
                state_is_current: error.state_is_current,
                mutation_may_have_occurred: error.mutation_may_have_occurred,
                state_query_error: error
                    .state_query_error
                    .map(|value| bounded_text(value.into(), 2048)),
                state_query_errno: error.state_query_errno,
                invalid_response: error.invalid_response.map(Vec::from),
                state_query_invalid_response: error.state_query_invalid_response.map(Vec::from),
            })
        }
        pub(super) struct Factory;
        pub(super) struct Session {
            hal: FanCurveSession,
            state: BrokerState,
            desired: Option<FanCurve>,
        }
        impl BackendFactory for Factory {
            type Session = Session;
            fn open(&mut self) -> BackendResult<Session> {
                let hal = FanCurveSession::open().map_err(|error| kernel_error(error, false))?;
                let state =
                    BrokerState::from_linux(hal.state(), true, hal.modified_or_uncertain(), None);
                Ok(Session {
                    hal,
                    state,
                    desired: None,
                })
            }
            fn inspect(&mut self) -> BackendResult<BrokerState> {
                linux::inspect()
                    .map(|state| BrokerState::from_linux(&state, true, false, None))
                    .map_err(|error| kernel_error(error, false))
            }
        }
        impl Session {
            fn result(
                &mut self,
                result: Result<linux::FanCurveState, FanCurveSessionError>,
            ) -> BackendResult<BrokerState> {
                match result {
                    Ok(state) => {
                        self.state = BrokerState::from_linux(
                            &state,
                            true,
                            self.hal.modified_or_uncertain(),
                            self.desired.clone(),
                        );
                        Ok(self.state.clone())
                    }
                    Err(error) => {
                        let error = kernel_error(error, self.hal.modified_or_uncertain());
                        if let Some(state) = &error.state {
                            self.state = (**state).clone();
                        } else {
                            self.state.state_is_current = false;
                            self.state.applied_curve = None;
                        }
                        Err(error)
                    }
                }
            }
        }
        impl LeaseBackend for Session {
            fn cached(&self) -> BrokerState {
                self.state.clone()
            }
            fn acquire(&mut self) -> BackendResult<BrokerState> {
                let result = self.hal.acquire();
                self.result(result)
            }
            fn apply(&mut self, curve: &ValidatedFanCurve) -> BackendResult<BrokerState> {
                let result = self.hal.apply(curve);
                if result.is_ok() {
                    self.desired = Some(curve.curve().clone());
                }
                self.result(result)
            }
            fn inspect(&mut self) -> BackendResult<BrokerState> {
                let result = self.hal.refresh();
                self.result(result)
            }
            fn restore(&mut self) -> BackendResult<BrokerState> {
                let result = self.hal.restore();
                self.desired = None;
                self.result(result)
            }
        }
        pub(super) struct Stdio {
            origin: Instant,
        }
        impl Stdio {
            pub(super) fn new() -> io::Result<Self> {
                for fd in [libc::STDIN_FILENO, libc::STDOUT_FILENO] {
                    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                    if flags == -1
                        || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
                    {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(Self {
                    origin: Instant::now(),
                })
            }
        }
        fn poll(fd: i32, events: i16, timeout: Duration) -> io::Result<bool> {
            let mut descriptor = libc::pollfd {
                fd,
                events,
                revents: 0,
            };
            let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
            let result = unsafe { libc::poll(&mut descriptor, 1, millis.max(1)) };
            if result == -1 {
                return Err(io::Error::last_os_error());
            }
            if descriptor.revents & libc::POLLNVAL != 0 {
                return Err(io::Error::from_raw_os_error(libc::EBADF));
            }
            Ok(result != 0)
        }
        impl Transport for Stdio {
            fn now(&self) -> Duration {
                self.origin.elapsed()
            }
            fn receive(&mut self, timeout: Duration) -> io::Result<Input> {
                match poll(libc::STDIN_FILENO, libc::POLLIN, timeout) {
                    Ok(false) => return Ok(Input::Wait),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                        return Ok(Input::Wait)
                    }
                    Err(error) => return Err(error),
                    Ok(true) => {}
                }
                let mut bytes = [0u8; 4096];
                let count = unsafe {
                    libc::read(libc::STDIN_FILENO, bytes.as_mut_ptr().cast(), bytes.len())
                };
                if count == 0 {
                    return Ok(Input::Eof);
                }
                if count < 0 {
                    let error = io::Error::last_os_error();
                    return if matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                    ) {
                        Ok(Input::Wait)
                    } else {
                        Err(error)
                    };
                }
                Ok(Input::Bytes(bytes[..count as usize].to_vec()))
            }
            fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
                let deadline = Instant::now() + Duration::from_millis(OUTPUT_TIMEOUT_MS);
                let mut written = 0;
                while written < bytes.len() {
                    let remaining =
                        deadline
                            .checked_duration_since(Instant::now())
                            .ok_or_else(|| {
                                io::Error::new(
                                    io::ErrorKind::TimedOut,
                                    "Broker response pipe stopped accepting data",
                                )
                            })?;
                    match poll(libc::STDOUT_FILENO, libc::POLLOUT, remaining) {
                        Ok(false) => continue,
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                        Err(error) => return Err(error),
                        Ok(true) => {}
                    }
                    let count = unsafe {
                        libc::write(
                            libc::STDOUT_FILENO,
                            bytes[written..].as_ptr().cast(),
                            bytes.len() - written,
                        )
                    };
                    if count == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::WriteZero,
                            "Broker output made no progress",
                        ));
                    }
                    if count < 0 {
                        let error = io::Error::last_os_error();
                        if matches!(
                            error.kind(),
                            io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                        ) {
                            continue;
                        }
                        return Err(error);
                    }
                    written += count as usize;
                }
                Ok(())
            }
        }
    }
    #[cfg(target_os = "linux")]
    pub(super) fn serve_linux() -> Result<(), ControlError> {
        if unsafe { libc::geteuid() } != 0 {
            return Err(ControlError::new(
                "fan_broker_privilege_required",
                "The installed fan helper requires explicit root authorization",
            ));
        }
        let mut transport = real::Stdio::new()
            .map_err(|error| ControlError::new("fan_broker_stdio_failed", error.to_string()))?;
        run(real::Factory, &mut transport)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::fan_curve::{FanCurveChannel, FanCurveLayout, RawFanTable};
        use std::{
            cell::{Cell, RefCell},
            collections::VecDeque,
            rc::Rc,
        };

        const NONCE: &str = "0123456789abcdef0123456789abcdef";
        fn curve(duty: u8) -> FanCurve {
            FanCurve {
                schema_version: 1,
                layout: FanCurveLayout::UniwillRamFan1p5,
                respective: true,
                cpu: FanCurveChannel {
                    rise_celsius: [35; 15],
                    fall_celsius: [30; 15],
                    duty_raw: [duty; 16],
                },
                gpu: FanCurveChannel {
                    rise_celsius: [255; 15],
                    fall_celsius: [255; 15],
                    duty_raw: [150; 16],
                },
            }
        }
        fn idle() -> BrokerState {
            BrokerState {
                backend: BACKEND.into(),
                device_path: DEVICE_PATH.into(),
                epoch: 11,
                generation: 20,
                lease_id: 0,
                last_request_id: 0,
                phase: BrokerPhase::Idle,
                capabilities: 7,
                owner: false,
                revoked: false,
                modified_or_uncertain: false,
                state_is_current: true,
                restored_verified: false,
                live_error: 0,
                last_error: 0,
                restore_error: 0,
                live_table: Some(vec![50; 96]),
                original_table: None,
                live_controls: Some([0; 6]),
                original_controls: None,
                cached_original_targets: None,
                applied_curve: None,
            }
        }
        struct World {
            state: BrokerState,
            calls: Vec<&'static str>,
            fail_open: bool,
            fail_acquire: bool,
            fail_apply: bool,
            fail_inspect: bool,
            fail_restore: bool,
            revoke_on_inspect: bool,
            full_mode_ack: bool,
            apply_delay: Duration,
            clock: Rc<Cell<Duration>>,
        }
        impl World {
            fn new(clock: Rc<Cell<Duration>>) -> Self {
                Self {
                    state: idle(),
                    calls: Vec::new(),
                    fail_open: false,
                    fail_acquire: false,
                    fail_apply: false,
                    fail_inspect: false,
                    fail_restore: false,
                    revoke_on_inspect: false,
                    full_mode_ack: false,
                    apply_delay: Duration::ZERO,
                    clock,
                }
            }
            fn error(&self, operation: &str) -> Box<BrokerError> {
                let mut error =
                    BrokerError::new("fixture_kernel_failure", format!("{operation} failed"));
                error.operation = Some(operation.into());
                error.errno = Some(5);
                error.state = Some(Box::new(self.state.clone()));
                error.state_is_current = true;
                error.mutation_may_have_occurred =
                    matches!(operation, "acquire" | "apply" | "restore");
                Box::new(error)
            }
        }
        struct Factory(Rc<RefCell<World>>);
        struct Session(Rc<RefCell<World>>);
        impl BackendFactory for Factory {
            type Session = Session;
            fn open(&mut self) -> BackendResult<Session> {
                let mut world = self.0.borrow_mut();
                world.calls.push("open_rw");
                if world.fail_open {
                    return Err(world.error("open"));
                }
                Ok(Session(self.0.clone()))
            }
            fn inspect(&mut self) -> BackendResult<BrokerState> {
                let mut world = self.0.borrow_mut();
                world.calls.push("inspect_ro");
                if world.fail_inspect {
                    return Err(world.error("inspect_ro"));
                }
                let mut state = world.state.clone();
                state.owner = false;
                state.applied_curve = None;
                Ok(state)
            }
        }
        impl Drop for Session {
            fn drop(&mut self) {
                self.0.borrow_mut().calls.push("close");
            }
        }
        impl LeaseBackend for Session {
            fn cached(&self) -> BrokerState {
                self.0.borrow().state.clone()
            }
            fn acquire(&mut self) -> BackendResult<BrokerState> {
                let mut world = self.0.borrow_mut();
                world.calls.push("acquire");
                if world.fail_acquire {
                    return Err(world.error("acquire"));
                }
                let state = &mut world.state;
                state.generation += 1;
                state.lease_id = 77;
                state.owner = true;
                state.restored_verified = false;
                if state.phase != BrokerPhase::RecoveryRequired {
                    state.phase = BrokerPhase::Acquired;
                }
                if state.original_table.is_none() {
                    state.original_table = state.live_table.clone();
                    state.original_controls = state.live_controls;
                    state.cached_original_targets = Some([51, 52, 53, 54]);
                }
                Ok(state.clone())
            }
            fn apply(&mut self, curve: &ValidatedFanCurve) -> BackendResult<BrokerState> {
                let mut world = self.0.borrow_mut();
                world.calls.push("apply");
                world.clock.set(world.clock.get() + world.apply_delay);
                if world.fail_apply {
                    world.state.phase = BrokerPhase::RecoveryRequired;
                    world.state.modified_or_uncertain = true;
                    return Err(world.error("apply"));
                }
                let full = world.full_mode_ack;
                let state = &mut world.state;
                state.generation += 1;
                state.last_request_id += 1;
                let raw = RawFanTable::from_bytes(
                    state
                        .live_table
                        .as_ref()
                        .unwrap()
                        .as_slice()
                        .try_into()
                        .unwrap(),
                )
                .with_curve(curve);
                state.live_table = Some(raw.as_bytes().to_vec());
                state.live_controls = Some([
                    if curve.curve().respective { 0x80 } else { 0 },
                    4,
                    if full { 0x40 } else { 0 },
                    0,
                    0,
                    0,
                ]);
                state.phase = BrokerPhase::Active;
                state.modified_or_uncertain = true;
                state.applied_curve = None;
                Ok(state.clone())
            }
            fn inspect(&mut self) -> BackendResult<BrokerState> {
                let mut world = self.0.borrow_mut();
                world.calls.push("inspect_owned");
                if world.revoke_on_inspect {
                    world.state.owner = false;
                    world.state.revoked = true;
                    world.state.lease_id = 0;
                    world.state.phase = BrokerPhase::RecoveryRequired;
                    world.state.applied_curve = None;
                }
                if world.fail_inspect {
                    return Err(world.error("inspect"));
                }
                Ok(world.state.clone())
            }
            fn restore(&mut self) -> BackendResult<BrokerState> {
                let mut world = self.0.borrow_mut();
                world.calls.push("restore");
                if world.fail_restore || !world.state.owner {
                    world.state.applied_curve = None;
                    return Err(world.error("restore"));
                }
                let state = &mut world.state;
                let modified = state.modified_or_uncertain;
                state.generation += 1;
                state.last_request_id += 1;
                state.phase = BrokerPhase::Idle;
                state.owner = false;
                state.lease_id = 0;
                state.live_table = state.original_table.take();
                state.live_controls = state.original_controls.take();
                state.cached_original_targets = None;
                state.modified_or_uncertain = false;
                state.restored_verified = modified;
                state.last_error = 0;
                state.restore_error = 0;
                state.applied_curve = None;
                state.revoked = false;
                Ok(state.clone())
            }
        }
        enum Event {
            Bytes(Vec<u8>, u64),
            Wait(u64),
            Eof,
            Error,
        }
        struct FakeTransport {
            events: VecDeque<Event>,
            output: Vec<BrokerResponse>,
            clock: Rc<Cell<Duration>>,
            sends: usize,
            fail_send: Option<usize>,
        }
        impl Transport for FakeTransport {
            fn now(&self) -> Duration {
                self.clock.get()
            }
            fn receive(&mut self, timeout: Duration) -> io::Result<Input> {
                match self.events.pop_front().unwrap_or(Event::Eof) {
                    Event::Bytes(bytes, ms) => {
                        self.clock.set(self.clock.get() + Duration::from_millis(ms));
                        Ok(Input::Bytes(bytes))
                    }
                    Event::Wait(ms) => {
                        self.clock
                            .set(self.clock.get() + Duration::from_millis(ms).min(timeout));
                        Ok(Input::Wait)
                    }
                    Event::Eof => Ok(Input::Eof),
                    Event::Error => Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "fixture input failed",
                    )),
                }
            }
            fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
                self.sends += 1;
                if self.fail_send == Some(self.sends) {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "fixture output failed",
                    ));
                }
                assert!(bytes.len() <= MAX_RESPONSE_BYTES + 1);
                self.output.push(parse_response(bytes).unwrap());
                Ok(())
            }
        }
        fn request(id: u64, command: BrokerCommand) -> BrokerRequest {
            BrokerRequest {
                schema_version: 1,
                request_id: id,
                session_id: NONCE.into(),
                command,
            }
        }
        fn frame(id: u64, command: BrokerCommand) -> Event {
            Event::Bytes(encode_request(&request(id, command)).unwrap(), 0)
        }
        fn setup(events: Vec<Event>) -> (Factory, FakeTransport, Rc<RefCell<World>>) {
            let clock = Rc::new(Cell::new(Duration::ZERO));
            let world = Rc::new(RefCell::new(World::new(clock.clone())));
            (
                Factory(world.clone()),
                FakeTransport {
                    events: events.into(),
                    output: Vec::new(),
                    clock,
                    sends: 0,
                    fail_send: None,
                },
                world,
            )
        }
        #[test]
        fn repeated_apply_uses_one_persistent_fd_ping_is_cached_and_shutdown_restores_once() {
            let (factory, mut transport, world) = setup(vec![
                frame(1, BrokerCommand::Apply { curve: curve(101) }),
                frame(2, BrokerCommand::Ping),
                frame(3, BrokerCommand::Apply { curve: curve(111) }),
                frame(4, BrokerCommand::Shutdown),
            ]);
            run(factory, &mut transport).unwrap();
            assert_eq!(
                world.borrow().calls,
                ["open_rw", "acquire", "apply", "apply", "restore", "close"]
            );
            assert_eq!(
                transport.output[0].state.as_ref().unwrap().applied_curve,
                Some(curve(101))
            );
            assert_eq!(
                transport.output[2].state.as_ref().unwrap().applied_curve,
                Some(curve(111))
            );
            assert!(!transport.output[1].state.as_ref().unwrap().state_is_current);
            assert!(transport.output[1]
                .state
                .as_ref()
                .unwrap()
                .applied_curve
                .is_none());
            assert!(transport.output[3].restoration_confirmed);
        }
        #[test]
        fn curve_validation_precedes_every_device_open_and_unknown_fields_are_rejected() {
            let mut invalid =
                serde_json::to_value(request(1, BrokerCommand::Apply { curve: curve(101) }))
                    .unwrap();
            invalid["command"]["curve"]["cpu"]["duty_raw"][0] = 201.into();
            let mut bytes = serde_json::to_vec(&invalid).unwrap();
            bytes.push(b'\n');
            let (factory, mut transport, world) = setup(vec![Event::Bytes(bytes, 0)]);
            assert!(run(factory, &mut transport).is_err());
            assert!(world.borrow().calls.is_empty());
            invalid["command"]["curve"]["cpu"]["duty_raw"][0] = 101.into();
            invalid["command"]["path"] = "/arbitrary/device".into();
            assert!(parse_request(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        #[test]
        fn stale_request_ids_or_changed_nonce_restore_before_terminal_error() {
            for nonce_mismatch in [false, true] {
                let mut bad = request(if nonce_mismatch { 2 } else { 1 }, BrokerCommand::Ping);
                if nonce_mismatch {
                    bad.session_id = "ffffffffffffffffffffffffffffffff".into();
                }
                let (factory, mut transport, world) = setup(vec![
                    frame(1, BrokerCommand::Apply { curve: curve(101) }),
                    Event::Bytes(encode_request(&bad).unwrap(), 0),
                ]);
                assert!(run(factory, &mut transport).is_err());
                assert_eq!(
                    world
                        .borrow()
                        .calls
                        .iter()
                        .filter(|call| **call == "restore")
                        .count(),
                    1
                );
                assert!(transport.output.last().unwrap().restoration_confirmed);
                assert_eq!(transport.output.last().unwrap().request_id, 0);
            }
        }
        #[test]
        fn eof_bad_partial_and_oversized_frames_restore_without_accepting_another_curve() {
            for last in [
                Event::Eof,
                Event::Bytes(b"bad json\n".to_vec(), 0),
                Event::Bytes(vec![b'x'; MAX_FRAME_BYTES + 1], 0),
                Event::Bytes(b"{partial".to_vec(), 0),
                Event::Error,
            ] {
                let (factory, mut transport, world) = setup(vec![
                    frame(1, BrokerCommand::Apply { curve: curve(101) }),
                    last,
                ]);
                assert!(run(factory, &mut transport).is_err());
                assert_eq!(
                    world.borrow().calls,
                    ["open_rw", "acquire", "apply", "restore", "close"]
                );
                let last = transport.output.last().unwrap();
                assert!(last.terminal);
                assert!(last.error.is_some());
                assert!(last.restoration_confirmed);
            }
        }
        #[test]
        fn slow_partial_frame_cannot_renew_heartbeat_and_valid_ping_can() {
            let (factory, mut transport, world) = setup(vec![
                frame(1, BrokerCommand::Apply { curve: curve(101) }),
                Event::Bytes(b"{partial".to_vec(), 10_000),
                Event::Wait(20_000),
            ]);
            assert!(run(factory, &mut transport).is_err());
            assert_eq!(
                transport
                    .output
                    .last()
                    .unwrap()
                    .error
                    .as_ref()
                    .unwrap()
                    .code,
                "fan_broker_heartbeat_expired"
            );
            assert!(world.borrow().calls.contains(&"restore"));
            let mut ping = encode_request(&request(2, BrokerCommand::Ping)).unwrap();
            ping.pop();
            ping.extend_from_slice(b"\r\n");
            let (factory, mut transport, world) = setup(vec![
                frame(1, BrokerCommand::Apply { curve: curve(101) }),
                Event::Wait(10_000),
                Event::Bytes(ping[..10].to_vec(), 0),
                Event::Bytes(ping[10..].to_vec(), 0),
                Event::Wait(10_000),
                frame(3, BrokerCommand::Shutdown),
            ]);
            run(factory, &mut transport).unwrap();
            assert!(!world
                .borrow()
                .calls
                .iter()
                .any(|call| call.starts_with("inspect")));
        }
        #[test]
        fn slow_kernel_apply_is_not_acknowledged_after_deadline_and_gets_recovery() {
            let (factory, mut transport, world) =
                setup(vec![frame(1, BrokerCommand::Apply { curve: curve(101) })]);
            world.borrow_mut().apply_delay = Duration::from_secs(21);
            assert!(run(factory, &mut transport).is_err());
            assert_eq!(transport.output.len(), 1);
            assert_ne!(transport.output[0].outcome, BrokerOutcome::Applied);
            assert!(transport.output[0].restoration_confirmed);
        }
        #[test]
        fn primary_failure_and_failed_restoration_are_independent_and_fd_closes() {
            let (factory, mut transport, world) =
                setup(vec![frame(1, BrokerCommand::Apply { curve: curve(101) })]);
            world.borrow_mut().fail_apply = true;
            world.borrow_mut().fail_restore = true;
            assert!(run(factory, &mut transport).is_err());
            let response = transport.output.last().unwrap();
            assert_eq!(
                response.error.as_ref().unwrap().operation.as_deref(),
                Some("apply")
            );
            assert_eq!(
                response
                    .recovery_error
                    .as_ref()
                    .unwrap()
                    .operation
                    .as_deref(),
                Some("restore")
            );
            assert!(!response.restoration_confirmed);
            assert!(response.close_fallback_unconfirmed);
            assert_eq!(
                world.borrow().calls,
                ["open_rw", "acquire", "apply", "restore", "close"]
            );
        }
        #[test]
        fn output_broken_pipe_restores_and_keeps_short_recovery_evidence_for_stderr() {
            let (factory, mut transport, world) =
                setup(vec![frame(1, BrokerCommand::Apply { curve: curve(101) })]);
            transport.fail_send = Some(1);
            world.borrow_mut().fail_restore = true;
            let error = run(factory, &mut transport).unwrap_err();
            assert!(error
                .details
                .iter()
                .any(|detail| detail.starts_with("recovery_code=fixture_kernel_failure")));
            assert!(error.details.iter().any(|detail| detail == "restoration_confirmed=false close_fallback_unconfirmed=true no_acquisition_attempted=false"));
            assert!(error
                .details
                .iter()
                .take(3)
                .all(|detail| detail.len() <= 128));
            assert_eq!(world.borrow().calls.last(), Some(&"close"));
        }
        #[test]
        fn eof_or_heartbeat_with_broken_output_keeps_failed_recovery_evidence() {
            for last in [
                Event::Eof,
                Event::Wait(20_000),
                Event::Bytes(b"bad json\n".to_vec(), 0),
                Event::Error,
            ] {
                let (factory, mut transport, world) = setup(vec![
                    frame(1, BrokerCommand::Apply { curve: curve(101) }),
                    last,
                ]);
                transport.fail_send = Some(2);
                world.borrow_mut().fail_restore = true;
                let error = run(factory, &mut transport).unwrap_err();
                assert_eq!(error.code, "fan_broker_output_failed");
                assert!(error
                    .details
                    .iter()
                    .any(|detail| detail.starts_with("primary_code=")));
                assert!(error
                    .details
                    .iter()
                    .any(|detail| detail.starts_with("recovery_code=fixture_kernel_failure")));
                assert!(error.details.iter().any(|detail| detail == "restoration_confirmed=false close_fallback_unconfirmed=true no_acquisition_attempted=false"));
                assert_eq!(
                    world
                        .borrow()
                        .calls
                        .iter()
                        .filter(|call| **call == "restore")
                        .count(),
                    1
                );
                assert_eq!(world.borrow().calls.last(), Some(&"close"));
            }
        }
        #[test]
        fn idle_historical_receipt_is_not_a_new_restore_and_foreign_recovery_is_not_orphaned() {
            let (factory, mut transport, world) = setup(vec![frame(1, BrokerCommand::Restore)]);
            world.borrow_mut().state.restored_verified = true;
            run(factory, &mut transport).unwrap();
            assert_eq!(world.borrow().calls, ["inspect_ro"]);
            assert_eq!(transport.output[0].outcome, BrokerOutcome::Released);
            assert!(!transport.output[0].restoration_confirmed);
            let (factory, mut transport, world) = setup(vec![frame(1, BrokerCommand::Restore)]);
            world.borrow_mut().state.phase = BrokerPhase::RecoveryRequired;
            world.borrow_mut().state.lease_id = 88;
            assert!(run(factory, &mut transport).is_err());
            assert_eq!(world.borrow().calls, ["inspect_ro"]);
        }
        #[test]
        fn explicit_orphan_recovery_adopts_only_for_restore_and_apply_never_adopts() {
            for apply in [false, true] {
                let command = if apply {
                    BrokerCommand::Apply { curve: curve(101) }
                } else {
                    BrokerCommand::Restore
                };
                let (factory, mut transport, world) = setup(vec![frame(1, command)]);
                world.borrow_mut().state.phase = BrokerPhase::RecoveryRequired;
                world.borrow_mut().state.modified_or_uncertain = true;
                if apply {
                    assert!(run(factory, &mut transport).is_err());
                    assert!(!world.borrow().calls.contains(&"acquire"));
                } else {
                    run(factory, &mut transport).unwrap();
                    assert_eq!(
                        world.borrow().calls,
                        ["inspect_ro", "open_rw", "acquire", "restore", "close"]
                    );
                    assert!(transport.output[0].restoration_confirmed);
                }
                assert!(!world.borrow().calls.contains(&"apply"));
            }
        }
        #[test]
        fn revoked_lease_exits_without_reacquiring_or_replaying_and_full_mode_cannot_ack_curve() {
            let (factory, mut transport, world) = setup(vec![
                frame(1, BrokerCommand::Apply { curve: curve(101) }),
                frame(2, BrokerCommand::Inspect),
            ]);
            world.borrow_mut().revoke_on_inspect = true;
            assert!(run(factory, &mut transport).is_err());
            assert_eq!(
                world
                    .borrow()
                    .calls
                    .iter()
                    .filter(|call| **call == "acquire")
                    .count(),
                1
            );
            assert_eq!(
                world
                    .borrow()
                    .calls
                    .iter()
                    .filter(|call| **call == "apply")
                    .count(),
                1
            );
            let (factory, mut transport, world) =
                setup(vec![frame(1, BrokerCommand::Apply { curve: curve(101) })]);
            world.borrow_mut().full_mode_ack = true;
            assert!(run(factory, &mut transport).is_err());
            assert_ne!(transport.output[0].outcome, BrokerOutcome::Applied);
            assert!(transport.output[0].restoration_confirmed);
        }
        #[test]
        fn open_acquire_and_inspect_failures_keep_primary_error_and_never_reopen() {
            for operation in ["open", "acquire", "inspect"] {
                let mut events = vec![frame(1, BrokerCommand::Apply { curve: curve(101) })];
                if operation == "inspect" {
                    events.push(frame(2, BrokerCommand::Inspect));
                }
                let (factory, mut transport, world) = setup(events);
                match operation {
                    "open" => world.borrow_mut().fail_open = true,
                    "acquire" => world.borrow_mut().fail_acquire = true,
                    _ => world.borrow_mut().fail_inspect = true,
                }
                assert!(run(factory, &mut transport).is_err());
                assert_eq!(
                    transport
                        .output
                        .last()
                        .unwrap()
                        .error
                        .as_ref()
                        .unwrap()
                        .operation
                        .as_deref(),
                    Some(operation)
                );
                assert_eq!(
                    world
                        .borrow()
                        .calls
                        .iter()
                        .filter(|call| **call == "open_rw")
                        .count(),
                    1
                );
                if operation == "open" {
                    assert!(!world.borrow().calls.contains(&"acquire"));
                } else {
                    assert_eq!(
                        world
                            .borrow()
                            .calls
                            .iter()
                            .filter(|call| **call == "restore")
                            .count(),
                        1
                    );
                }
            }
        }
        #[test]
        fn maximum_sized_crlf_frame_is_bounded_and_does_not_open_a_write_session() {
            let mut bytes = encode_request(&request(1, BrokerCommand::Ping)).unwrap();
            bytes.pop();
            bytes.resize(MAX_FRAME_BYTES, b' ');
            bytes.extend_from_slice(b"\r\n");
            assert!(parse_request(&bytes).is_ok());
            let (factory, mut transport, world) = setup(vec![
                Event::Bytes(bytes, 0),
                frame(2, BrokerCommand::Shutdown),
            ]);
            run(factory, &mut transport).unwrap();
            assert_eq!(world.borrow().calls, ["inspect_ro"]);
            assert_eq!(transport.output[0].outcome, BrokerOutcome::Pong);
            assert!(transport.output[0].state.is_none());
        }
        #[test]
        fn competing_foreign_owner_refuses_apply_without_local_lease_or_restore() {
            for phase in [
                BrokerPhase::Acquired,
                BrokerPhase::Active,
                BrokerPhase::RecoveryRequired,
            ] {
                let (factory, mut transport, world) =
                    setup(vec![frame(1, BrokerCommand::Apply { curve: curve(101) })]);
                world.borrow_mut().state.phase = phase;
                world.borrow_mut().state.lease_id = 88;
                world.borrow_mut().state.owner = false;
                world.borrow_mut().state.modified_or_uncertain = true;
                assert!(run(factory, &mut transport).is_err());
                assert_eq!(world.borrow().calls, ["open_rw", "close"]);
                let response = transport.output.last().unwrap();
                assert!(response.terminal);
                assert!(response.no_acquisition_attempted);
                assert!(!response.restoration_confirmed);
                assert!(!response.close_fallback_unconfirmed);
                assert_eq!(response.state.as_ref().unwrap().lease_id, 88);
            }
            let (factory, mut transport, world) =
                setup(vec![frame(1, BrokerCommand::Apply { curve: curve(101) })]);
            world.borrow_mut().fail_acquire = true;
            assert!(run(factory, &mut transport).is_err());
            assert!(!transport.output.last().unwrap().no_acquisition_attempted, "an attempted acquisition cannot become a zero-write proof from a failed acknowledgement");
        }
        #[test]
        fn response_validation_rejects_wrong_raw_lengths_stale_receipts_and_impossible_terminal_states(
        ) {
            let (factory, mut transport, _) = setup(vec![
                frame(1, BrokerCommand::Apply { curve: curve(101) }),
                frame(2, BrokerCommand::Restore),
            ]);
            run(factory, &mut transport).unwrap();
            let valid = transport.output.last().unwrap().clone();
            for change in 0..7 {
                let mut invalid = valid.clone();
                match change {
                    0 => invalid.state.as_mut().unwrap().state_is_current = false,
                    1 => invalid.close_fallback_unconfirmed = true,
                    2 => invalid.state.as_mut().unwrap().live_table = Some(vec![0; 95]),
                    3 => invalid.state.as_mut().unwrap().owner = true,
                    4 => invalid.terminal = false,
                    5 => invalid.restoration_confirmed = false,
                    _ => invalid.state.as_mut().unwrap().phase = BrokerPhase::Active,
                }
                assert!(validate_response(&invalid).is_err());
                assert!(parse_response(&serde_json::to_vec(&invalid).unwrap()).is_err());
            }
            let mut invalid = request(1, BrokerCommand::Ping);
            invalid.session_id = "0".repeat(31);
            assert!(encode_request(&invalid).is_err());
            invalid.session_id = NONCE.into();
            invalid.request_id = 0;
            assert!(encode_request(&invalid).is_err());
            let raw = b"{\"schema_version\":1,\"schema_version\":1,\"request_id\":1,\"session_id\":\"0123456789abcdef0123456789abcdef\",\"command\":{\"action\":\"ping\"}}";
            assert!(parse_request(raw).is_err());
        }
    }
}
