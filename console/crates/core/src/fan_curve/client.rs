//! Persistent, explicitly started fan-helper client. Construction never opens a
//! device or starts authentication. Drop closes input and leaves a real reaper;
//! it is not a restoration receipt. Synchronous operations belong on a worker.

use super::{broker::BrokerState, ValidatedFanCurve};
use crate::ControlError;

#[derive(Debug, Clone)]
pub struct FanClientUpdate {
    pub state: Option<BrokerState>,
    pub error: Option<ControlError>,
    /// Includes an uncertain/orphaned lease, even after its process has exited.
    pub session_held: bool,
    pub helper_reaped: bool,
    /// A matched restoration transaction, never just a historical GET bit.
    pub restoration_confirmed: bool,
    pub cleanup_complete: bool,
}
impl Default for FanClientUpdate {
    fn default() -> Self {
        Self {
            state: None,
            error: None,
            session_held: false,
            helper_reaped: true,
            restoration_confirmed: false,
            cleanup_complete: true,
        }
    }
}

// @mark mechrevo.fan_curve role=client contract=persistent_unprivileged_session
pub struct FanBrokerClient {
    #[cfg(target_os = "linux")]
    inner: platform::Client,
}
impl Default for FanBrokerClient {
    fn default() -> Self {
        Self::new()
    }
}
impl FanBrokerClient {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            inner: platform::Client::new(),
        }
    }
    pub fn probe(&mut self) -> FanClientUpdate {
        #[cfg(target_os = "linux")]
        {
            self.inner.probe()
        }
        #[cfg(not(target_os = "linux"))]
        {
            unsupported()
        }
    }
    pub fn apply(&mut self, curve: &ValidatedFanCurve) -> FanClientUpdate {
        #[cfg(target_os = "linux")]
        {
            self.inner.apply(curve)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = curve;
            unsupported()
        }
    }
    pub fn refresh(&mut self) -> FanClientUpdate {
        #[cfg(target_os = "linux")]
        {
            self.inner.refresh()
        }
        #[cfg(not(target_os = "linux"))]
        {
            unsupported()
        }
    }
    pub fn restore(&mut self) -> FanClientUpdate {
        #[cfg(target_os = "linux")]
        {
            self.inner.restore()
        }
        #[cfg(not(target_os = "linux"))]
        {
            unsupported()
        }
    }
    pub fn shutdown(&mut self) -> FanClientUpdate {
        #[cfg(target_os = "linux")]
        {
            self.inner.shutdown()
        }
        #[cfg(not(target_os = "linux"))]
        {
            unsupported()
        }
    }
    /// No device reads, authentication or blocking child waits. Poll this on
    /// idle UI ticks as well: helpers can expire or exit without a user action.
    pub fn poll(&mut self) -> Option<FanClientUpdate> {
        #[cfg(target_os = "linux")]
        {
            self.inner.poll()
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    }
}
#[cfg(not(target_os = "linux"))]
fn unsupported() -> FanClientUpdate {
    FanClientUpdate {
        error: Some(ControlError::new(
            "fan_broker_unsupported",
            "The persistent fan helper is supported only on Linux",
        )),
        ..Default::default()
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use crate::fan_curve::{
        broker::{
            self, BrokerCommand, BrokerOutcome, BrokerPhase, BrokerRequest, BrokerResponse,
            PROTOCOL_VERSION,
        },
        linux,
    };
    use std::{
        collections::BTreeMap,
        io::{self, Read},
        os::fd::AsRawFd,
        process::{Child, ChildStdin, Command, ExitStatus, Stdio},
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Condvar, Mutex, MutexGuard, Weak,
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };

    const MAX_PENDING: usize = 32;
    const STDERR_LIMIT: usize = 16_384;
    const HEARTBEAT: Duration = Duration::from_secs(2);
    const WRITE_LIMIT: Duration = Duration::from_secs(2);
    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        value.lock().unwrap_or_else(|error| error.into_inner())
    }
    fn error(code: &str, message: impl Into<String>) -> ControlError {
        ControlError::new(code, message)
    }
    fn append_error(primary: &mut Option<ControlError>, secondary: ControlError) {
        if let Some(primary) = primary {
            if primary.code == secondary.code
                && primary.message == secondary.message
                && primary.details == secondary.details
            {
                return;
            }
            if primary.details.len() < 8 {
                primary
                    .details
                    .push(serde_json::json!({"secondary_error":secondary}).to_string());
            }
        } else {
            *primary = Some(secondary);
        }
    }
    fn response_error(reply: &BrokerResponse) -> Option<ControlError> {
        let mut result = reply
            .error
            .as_ref()
            .map(|failure| error(&failure.code, &failure.message));
        if result.is_none()
            && (reply.recovery_error.is_some()
                || matches!(
                    reply.outcome,
                    BrokerOutcome::Rejected | BrokerOutcome::Failed | BrokerOutcome::Terminated
                ))
        {
            result = Some(error(
                "fan_helper_operation_failed",
                "The helper did not complete this operation",
            ));
        }
        if let Some(result) = &mut result {
            // Includes primary and recovery errors, raw kernel response bytes,
            // the original state and the exact matched protocol envelope.
            result
                .details
                .push(serde_json::json!({"broker_response":reply}).to_string());
        }
        result
    }
    fn clean_state(state: &BrokerState) -> bool {
        state.state_is_current
            && state.phase == BrokerPhase::Idle
            && !state.owner
            && state.lease_id == 0
            && !state.modified_or_uncertain
            && state.last_error == 0
            && state.restore_error == 0
    }
    fn nonce() -> io::Result<String> {
        let mut bytes = [0u8; 16];
        let mut offset = 0;
        while offset < bytes.len() {
            let count = unsafe {
                libc::getrandom(bytes[offset..].as_mut_ptr().cast(), bytes.len() - offset, 0)
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "OS randomness returned no bytes",
                ));
            }
            offset += count as usize;
        }
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }
    fn installed_command() -> Command {
        let mut command = Command::new("/usr/bin/pkexec");
        command
            .args(["--disable-internal-agent", broker::HELPER_PATH, "--serve"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LANG", "C")
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn spawn_installed() -> io::Result<Child> {
        installed_command().spawn()
    }

    struct Pending {
        command: BrokerCommand,
        reply: Option<BrokerResponse>,
    }
    #[derive(Default)]
    struct Status {
        pending: BTreeMap<u64, Pending>,
        response: Option<BrokerResponse>,
        error: Option<ControlError>,
        exit: Option<ExitStatus>,
        wait_error: bool,
        stdout_done: bool,
        stderr_done: bool,
        stderr: Vec<u8>,
        stderr_truncated: bool,
        discard_protocol: bool,
        observed_reply: bool,
        revision: u64,
        response_revision: u64,
        terminal_received: bool,
    }
    #[derive(Default)]
    struct Shared {
        status: Mutex<Status>,
        changed: Condvar,
        stop_heartbeat: AtomicBool,
    }
    impl Shared {
        fn change(&self, status: &mut Status) {
            status.revision = status.revision.wrapping_add(1);
            self.changed.notify_all();
        }
        fn fail(&self, mut failure: ControlError, discard: bool) {
            let mut status = lock(&self.status);
            if status.error.is_none() {
                if let Some(reply) = &status.response {
                    failure
                        .details
                        .push(serde_json::json!({"last_valid_response":reply}).to_string());
                }
            }
            append_error(&mut status.error, failure);
            status.discard_protocol |= discard;
            self.stop_heartbeat.store(true, Ordering::Release);
            self.change(&mut status);
        }
    }
    struct WriteState {
        input: Option<ChildStdin>,
        next_id: u64,
    }
    struct Writer {
        state: Mutex<WriteState>,
        closed: AtomicBool,
        nonce: String,
        shared: Arc<Shared>,
    }
    impl Writer {
        fn close(&self) {
            self.closed.store(true, Ordering::Release);
            lock(&self.state).input.take();
        }
        fn send(&self, command: BrokerCommand) -> Result<u64, ControlError> {
            // This same lock allocates IDs AND writes complete frames. A Ping
            // can never overtake an allocated-but-not-yet-written Apply ID.
            let mut writer = lock(&self.state);
            if self.closed.load(Ordering::Acquire) || writer.input.is_none() {
                return Err(error(
                    "fan_helper_input_closed",
                    "Helper input is closed; no request was retried",
                ));
            }
            let id = writer
                .next_id
                .checked_add(1)
                .ok_or_else(|| error("fan_helper_request_exhausted", "Request IDs exhausted"))?;
            let request = BrokerRequest {
                schema_version: PROTOCOL_VERSION,
                request_id: id,
                session_id: self.nonce.clone(),
                command: command.clone(),
            };
            let bytes = broker::encode_request(&request)?;
            {
                let mut status = lock(&self.shared.status);
                if status.pending.len() >= MAX_PENDING {
                    return Err(error("fan_helper_backpressure", "Pending replies reached the bounded limit; operation outcome remains uncertain"));
                }
                status.pending.insert(
                    id,
                    Pending {
                        command,
                        reply: None,
                    },
                );
            }
            writer.next_id = id;
            let fd = writer
                .input
                .as_ref()
                .expect("input checked while locked")
                .as_raw_fd();
            let deadline = Instant::now() + WRITE_LIMIT;
            let mut sent = 0;
            while sent < bytes.len() {
                if self.closed.load(Ordering::Acquire) || Instant::now() >= deadline {
                    let mut failure = error(
                        "fan_helper_input_stalled",
                        "Bounded pipe write stopped; this does not cancel a kernel operation",
                    );
                    failure.details.push(format!(
                        "request_id={id}; bytes_written={sent}; frame_bytes={}",
                        bytes.len()
                    ));
                    return Err(failure);
                }
                let count =
                    unsafe { libc::write(fd, bytes[sent..].as_ptr().cast(), bytes.len() - sent) };
                if count > 0 {
                    sent += count as usize;
                    continue;
                }
                if count < 0 {
                    let failure = io::Error::last_os_error();
                    if failure.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    if failure.kind() != io::ErrorKind::WouldBlock {
                        let mut result = error("fan_helper_input_failed", failure.to_string());
                        result
                            .details
                            .push(format!("request_id={id}; bytes_written={sent}"));
                        return Err(result);
                    }
                } else {
                    return Err(error("fan_helper_input_failed", "Pipe write returned zero"));
                }
                let mut poll = libc::pollfd {
                    fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                let result = unsafe { libc::poll(&mut poll, 1, 25) };
                if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                    return Err(error(
                        "fan_helper_input_failed",
                        io::Error::last_os_error().to_string(),
                    ));
                }
            }
            Ok(id)
        }
    }
    fn stop_transport(
        shared: &Shared,
        writer: &Weak<Writer>,
        failure: ControlError,
        discard: bool,
    ) {
        shared.fail(failure, discard);
        if let Some(writer) = writer.upgrade() {
            writer.close();
        }
    }
    fn valid_action(command: &BrokerCommand, reply: &BrokerResponse) -> bool {
        if reply.outcome == BrokerOutcome::Terminated {
            return reply.terminal && reply.error.is_some();
        }
        if matches!(
            reply.outcome,
            BrokerOutcome::Rejected | BrokerOutcome::Failed
        ) {
            return reply.error.is_some();
        }
        match command {
            BrokerCommand::Apply { curve } => {
                reply.outcome == BrokerOutcome::Applied
                    && reply
                        .state
                        .as_ref()
                        .and_then(|state| state.applied_curve.as_ref())
                        == Some(curve)
            }
            BrokerCommand::Inspect => reply.outcome == BrokerOutcome::Inspected,
            BrokerCommand::Ping => reply.outcome == BrokerOutcome::Pong && !reply.terminal,
            BrokerCommand::Restore => matches!(
                reply.outcome,
                BrokerOutcome::Restored | BrokerOutcome::Released
            ),
            BrokerCommand::Shutdown => {
                reply.terminal
                    && matches!(
                        reply.outcome,
                        BrokerOutcome::Restored | BrokerOutcome::Released | BrokerOutcome::Shutdown
                    )
            }
        }
    }
    fn receive_frame(shared: &Shared, writer: &Weak<Writer>, nonce: &str, frame: &[u8]) {
        let parsed = broker::parse_response(frame);
        let reply = match parsed {
            Ok(reply) => reply,
            Err(mut failure) => {
                failure
                    .details
                    .push(serde_json::json!({"invalid_response_bytes":frame}).to_string());
                stop_transport(shared, writer, failure, true);
                return;
            }
        };
        let mut status = lock(&shared.status);
        let invalid = if status.terminal_received {
            Some("Response followed an already terminal response")
        } else if reply.session_id != nonce {
            Some("Response session nonce does not match this helper session")
        } else if reply.request_id == 0 {
            if reply.terminal && reply.outcome == BrokerOutcome::Terminated {
                None
            } else {
                Some("Unsolicited response is not a terminal failure")
            }
        } else if let Some(pending) = status.pending.get(&reply.request_id) {
            if pending.reply.is_some() {
                Some("Duplicate response ID")
            } else if !valid_action(&pending.command, &reply) {
                Some("Response action or applied target does not match its request")
            } else {
                None
            }
        } else {
            Some("Response ID was not pending (unknown, duplicate or already consumed)")
        };
        if let Some(message) = invalid {
            drop(status);
            let mut failure = error("invalid_fan_broker_protocol", message);
            failure
                .details
                .push(serde_json::json!({"invalid_response":reply}).to_string());
            stop_transport(shared, writer, failure, true);
            return;
        }
        status.observed_reply = true;
        if reply.request_id == 0 {
            status.response = Some(reply.clone());
            status.response_revision = status.response_revision.wrapping_add(1);
            append_error(
                &mut status.error,
                response_error(&reply).unwrap_or_else(|| {
                    error("fan_helper_terminated", "Helper terminated asynchronously")
                }),
            );
        } else {
            let ping = matches!(
                status.pending[&reply.request_id].command,
                BrokerCommand::Ping
            );
            if ping {
                status.pending.remove(&reply.request_id);
                if reply.outcome != BrokerOutcome::Pong {
                    append_error(
                        &mut status.error,
                        response_error(&reply).unwrap_or_else(|| {
                            error(
                                "fan_helper_heartbeat_failed",
                                "Heartbeat was not acknowledged",
                            )
                        }),
                    );
                }
            } else {
                status
                    .pending
                    .get_mut(&reply.request_id)
                    .expect("pending checked")
                    .reply = Some(reply.clone());
                status.response = Some(reply.clone());
                status.response_revision = status.response_revision.wrapping_add(1);
            }
        }
        let terminal = reply.terminal;
        status.terminal_received |= terminal;
        if reply.outcome == BrokerOutcome::Pong {
            shared.changed.notify_all();
        } else {
            shared.change(&mut status);
        }
        drop(status);
        if terminal {
            shared.stop_heartbeat.store(true, Ordering::Release);
            if let Some(writer) = writer.upgrade() {
                writer.close();
            }
        }
    }
    fn read_stdout(
        mut output: impl Read,
        shared: Arc<Shared>,
        writer: Weak<Writer>,
        nonce: String,
    ) {
        let mut block = [0u8; 4096];
        let mut frame = Vec::new();
        let mut discard_frame = false;
        loop {
            let count = match output.read(&mut block) {
                Ok(0) => break,
                Ok(count) => count,
                Err(failure) if failure.kind() == io::ErrorKind::Interrupted => continue,
                Err(failure) => {
                    stop_transport(
                        &shared,
                        &writer,
                        error("fan_helper_output_failed", failure.to_string()),
                        true,
                    );
                    break;
                }
            };
            for byte in &block[..count] {
                if lock(&shared.status).discard_protocol {
                    continue;
                }
                if *byte == b'\n' {
                    if !discard_frame {
                        receive_frame(&shared, &writer, &nonce, &frame);
                    }
                    frame.clear();
                    discard_frame = false;
                } else if !discard_frame {
                    if frame.len() == broker::MAX_RESPONSE_BYTES {
                        let mut failure = error(
                            "invalid_fan_broker_protocol",
                            "Response frame exceeds 32 KiB; remaining output is still drained",
                        );
                        failure
                            .details
                            .push(serde_json::json!({"invalid_response_prefix":frame}).to_string());
                        frame.clear();
                        discard_frame = true;
                        stop_transport(&shared, &writer, failure, true);
                    } else {
                        frame.push(*byte);
                    }
                }
            }
        }
        if !frame.is_empty() && !lock(&shared.status).discard_protocol {
            let mut failure = error(
                "fan_helper_early_eof",
                "Helper output ended inside a response frame",
            );
            failure
                .details
                .push(serde_json::json!({"partial_response_bytes":frame}).to_string());
            stop_transport(&shared, &writer, failure, true);
        }
        let mut status = lock(&shared.status);
        status.stdout_done = true;
        if !status.terminal_received {
            let mut failure = error("fan_helper_early_eof", "Helper output ended without a terminal receipt; closing input is not proof of restoration");
            if let Some(reply) = &status.response {
                failure
                    .details
                    .push(serde_json::json!({"last_valid_response":reply}).to_string());
            }
            append_error(&mut status.error, failure);
        }
        shared.stop_heartbeat.store(true, Ordering::Release);
        shared.change(&mut status);
        drop(status);
        if let Some(writer) = writer.upgrade() {
            writer.close();
        }
    }
    fn read_stderr(mut output: impl Read, shared: Arc<Shared>) {
        let mut block = [0; 4096];
        loop {
            match output.read(&mut block) {
                Ok(0) => break,
                Ok(count) => {
                    let mut status = lock(&shared.status);
                    let remaining = STDERR_LIMIT.saturating_sub(status.stderr.len());
                    status
                        .stderr
                        .extend_from_slice(&block[..count.min(remaining)]);
                    status.stderr_truncated |= count > remaining;
                }
                Err(failure) if failure.kind() == io::ErrorKind::Interrupted => continue,
                Err(failure) => {
                    shared.fail(
                        error("fan_helper_stderr_failed", failure.to_string()),
                        false,
                    );
                    break;
                }
            }
        }
        let mut status = lock(&shared.status);
        status.stderr_done = true;
        shared.change(&mut status);
    }

    struct Session {
        shared: Arc<Shared>,
        writer: Arc<Writer>,
        threads: Vec<JoinHandle<()>>,
        heartbeat: Option<JoinHandle<()>>,
        child_slot: Arc<Mutex<Option<Child>>>,
        baseline_clean: bool,
        last_seen: u64,
        last_response_seen: u64,
        reaper_started: bool,
    }
    impl Session {
        fn start(
            mut child: Child,
            nonce: String,
            interval: Duration,
            baseline_clean: bool,
        ) -> Self {
            let input = child.stdin.take();
            let output = child.stdout.take();
            let stderr = child.stderr.take();
            let shared = Arc::new(Shared::default());
            let writer = Arc::new(Writer {
                state: Mutex::new(WriteState { input, next_id: 0 }),
                closed: AtomicBool::new(false),
                nonce: nonce.clone(),
                shared: shared.clone(),
            });
            let child_slot = Arc::new(Mutex::new(Some(child)));
            let mut session = Self {
                shared: shared.clone(),
                writer: writer.clone(),
                threads: Vec::new(),
                heartbeat: None,
                child_slot: child_slot.clone(),
                baseline_clean,
                last_seen: 0,
                last_response_seen: 0,
                reaper_started: false,
            };
            let reaper_shared = shared.clone();
            let result = thread::Builder::new()
                .name("fan-helper-reaper".into())
                .spawn(move || {
                    let Some(mut child) = lock(&child_slot).take() else {
                        return;
                    };
                    loop {
                        match child.wait() {
                            Ok(exit) => {
                                let mut status = lock(&reaper_shared.status);
                                status.exit = Some(exit);
                                status.wait_error = false;
                                reaper_shared.change(&mut status);
                                break;
                            }
                            Err(failure) if failure.kind() == io::ErrorKind::Interrupted => {
                                continue
                            }
                            Err(failure) => {
                                let mut status = lock(&reaper_shared.status);
                                status.wait_error = true;
                                if status.error.is_none() {
                                    status.error = Some(error(
                                        "fan_helper_liveness_unknown",
                                        failure.to_string(),
                                    ));
                                }
                                reaper_shared.change(&mut status);
                                drop(status);
                                // Retain the Child and keep a real reaper. No signal
                                // or synthetic successful wait replaces this failure.
                                thread::sleep(Duration::from_millis(250));
                            }
                        }
                    }
                });
            match result {
                Ok(handle) => {
                    session.reaper_started = true;
                    session.threads.push(handle);
                }
                Err(failure) => {
                    shared.fail(
                        error("fan_helper_thread_failed", failure.to_string()),
                        false,
                    );
                    writer.close();
                }
            }
            let flags = lock(&writer.state)
                .input
                .as_ref()
                .map(|input| unsafe { libc::fcntl(input.as_raw_fd(), libc::F_GETFL) });
            let nonblocking = match flags {
                Some(flags) if flags >= 0 => lock(&writer.state).input.as_ref().is_some_and(|input| unsafe { libc::fcntl(input.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0),
                _ => false,
            };
            if !nonblocking {
                shared.fail(
                    error(
                        "fan_helper_input_failed",
                        "Could not configure bounded nonblocking helper input",
                    ),
                    false,
                );
                writer.close();
            }
            if let Some(output) = output {
                let read_shared = shared.clone();
                let read_writer = Arc::downgrade(&writer);
                match thread::Builder::new()
                    .name("fan-helper-output".into())
                    .spawn(move || read_stdout(output, read_shared, read_writer, nonce))
                {
                    Ok(handle) => session.threads.push(handle),
                    Err(failure) => {
                        shared.fail(error("fan_helper_thread_failed", failure.to_string()), true);
                        lock(&shared.status).stdout_done = true;
                        writer.close();
                    }
                }
            } else {
                lock(&shared.status).stdout_done = true;
                shared.fail(
                    error("fan_helper_output_failed", "Helper stdout was not piped"),
                    true,
                );
                writer.close();
            }
            if let Some(stderr) = stderr {
                let read_shared = shared.clone();
                match thread::Builder::new()
                    .name("fan-helper-stderr".into())
                    .spawn(move || read_stderr(stderr, read_shared))
                {
                    Ok(handle) => session.threads.push(handle),
                    Err(failure) => {
                        shared.fail(
                            error("fan_helper_thread_failed", failure.to_string()),
                            false,
                        );
                        lock(&shared.status).stderr_done = true;
                        writer.close();
                    }
                }
            } else {
                lock(&shared.status).stderr_done = true;
                shared.fail(
                    error("fan_helper_stderr_failed", "Helper stderr was not piped"),
                    false,
                );
                writer.close();
            }
            let beat_shared = shared.clone();
            let beat_writer = writer.clone();
            match thread::Builder::new()
                .name("fan-helper-heartbeat".into())
                .spawn(move || loop {
                    let status = lock(&beat_shared.status);
                    if beat_shared.stop_heartbeat.load(Ordering::Acquire) {
                        break;
                    }
                    let (status, _) = beat_shared
                        .changed
                        .wait_timeout_while(status, interval, |_| {
                            !beat_shared.stop_heartbeat.load(Ordering::Acquire)
                        })
                        .unwrap_or_else(|e| e.into_inner());
                    if beat_shared.stop_heartbeat.load(Ordering::Acquire) {
                        break;
                    }
                    drop(status);
                    if let Err(failure) = beat_writer.send(BrokerCommand::Ping) {
                        beat_shared.fail(failure, false);
                        beat_writer.close();
                        break;
                    }
                }) {
                Ok(handle) => session.heartbeat = Some(handle),
                Err(failure) => {
                    shared.fail(
                        error("fan_helper_thread_failed", failure.to_string()),
                        false,
                    );
                    writer.close();
                }
            }
            session
        }
        fn stop_heartbeat(&mut self) {
            self.shared.stop_heartbeat.store(true, Ordering::Release);
            self.shared.changed.notify_all();
            if let Some(handle) = self.heartbeat.take() {
                let _ = handle.join();
            }
        }
        fn request(&self, command: BrokerCommand) -> Option<BrokerResponse> {
            let id = match self.writer.send(command) {
                Ok(id) => id,
                Err(failure) => {
                    self.shared.fail(failure, false);
                    self.writer.close();
                    return None;
                }
            };
            let mut status = lock(&self.shared.status);
            loop {
                if status
                    .pending
                    .get(&id)
                    .is_some_and(|pending| pending.reply.is_some())
                {
                    return status.pending.remove(&id).and_then(|pending| pending.reply);
                }
                if status.error.is_some() || status.stdout_done || status.wait_error {
                    return None;
                }
                status = self
                    .shared
                    .changed
                    .wait(status)
                    .unwrap_or_else(|e| e.into_inner());
            }
        }
        fn finish(&mut self) {
            self.stop_heartbeat();
            self.writer.close();
            // In the exceptional failure to create a reaper, retain ownership
            // and perform its actual wait here rather than dropping a live child.
            if !self.reaper_started {
                let retained_child = { lock(&self.child_slot).take() };
                if let Some(mut child) = retained_child {
                    loop {
                        match child.wait() {
                            Ok(exit) => {
                                let mut s = lock(&self.shared.status);
                                s.exit = Some(exit);
                                s.wait_error = false;
                                self.shared.change(&mut s);
                                break;
                            }
                            Err(failure) if failure.kind() == io::ErrorKind::Interrupted => {
                                continue
                            }
                            Err(failure) => {
                                self.shared.fail(
                                    error("fan_helper_liveness_unknown", failure.to_string()),
                                    false,
                                );
                                thread::sleep(Duration::from_millis(250));
                            }
                        }
                    }
                }
            }
            let mut status = lock(&self.shared.status);
            while !(status.exit.is_some() && status.stdout_done && status.stderr_done)
                && !status.wait_error
            {
                status = self
                    .shared
                    .changed
                    .wait(status)
                    .unwrap_or_else(|e| e.into_inner());
            }
            let complete = status.exit.is_some() && status.stdout_done && status.stderr_done;
            drop(status);
            if complete {
                for handle in self.threads.drain(..) {
                    let _ = handle.join();
                }
            }
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            self.stop_heartbeat();
            self.writer.close();
            // Detached drainers/reaper retain their own state until EOF/wait.
            // If the reaper could not even be started, wait synchronously.
            if !self.reaper_started {
                self.finish();
            }
        }
    }

    pub(super) struct Client {
        update: FanClientUpdate,
        session: Option<Session>,
        interval: Duration,
        #[cfg(test)]
        launcher: Option<Box<dyn FnMut() -> io::Result<Child> + Send>>,
    }
    impl Client {
        pub(super) fn new() -> Self {
            Self {
                update: Default::default(),
                session: None,
                interval: HEARTBEAT,
                #[cfg(test)]
                launcher: None,
            }
        }
        fn spawn(&mut self) -> Result<(), ControlError> {
            let nonce = nonce()
                .map_err(|failure| error("fan_helper_randomness_failed", failure.to_string()))?;
            #[cfg(test)]
            let child = if let Some(launcher) = &mut self.launcher {
                launcher()
            } else {
                spawn_installed()
            };
            #[cfg(not(test))]
            let child = spawn_installed();
            let child =
                child.map_err(|failure| error("fan_helper_spawn_failed", failure.to_string()))?;
            self.session = Some(Session::start(
                child,
                nonce,
                self.interval,
                self.update.cleanup_complete,
            ));
            self.update.helper_reaped = false;
            self.update.restoration_confirmed = false;
            Ok(())
        }
        fn absorb(&mut self, reply: &BrokerResponse) {
            if let Some(state) = &reply.state {
                self.update.state = Some(state.clone());
                if !clean_state(state) {
                    self.update.restoration_confirmed = false;
                }
                if clean_state(state) {
                    self.update.session_held = false;
                    self.update.cleanup_complete = true;
                } else if state.owner || self.update.session_held {
                    self.update.session_held = true;
                    self.update.cleanup_complete = false;
                }
            }
            // Inspect/Ping and an initial Idle GET may expose an OLD flag.
            // Only the server's operation-specific restoration_confirmed receipt is proof.
            if reply.restoration_confirmed
                && !reply.close_fallback_unconfirmed
                && reply
                    .state
                    .as_ref()
                    .is_some_and(|state| clean_state(state) && state.restored_verified)
            {
                self.update.restoration_confirmed = true;
            }
            if reply.close_fallback_unconfirmed {
                self.update.session_held = true;
                self.update.cleanup_complete = false;
            }
            if let Some(failure) = response_error(reply) {
                append_error(&mut self.update.error, failure);
            }
        }
        fn sync(&mut self, force: bool) -> bool {
            let Some(session) = &mut self.session else {
                return false;
            };
            let status = lock(&session.shared.status);
            if !force && status.revision == session.last_seen {
                return false;
            }
            session.last_seen = status.revision;
            let response = if status.response_revision != session.last_response_seen {
                session.last_response_seen = status.response_revision;
                status.response.clone()
            } else {
                None
            };
            let failure = status.error.clone();
            let exit = status.exit;
            let transport_uncertain =
                status.error.is_some() || (status.stdout_done && !status.terminal_received);
            let observed = status.observed_reply;
            let baseline_clean = session.baseline_clean;
            let no_acquisition = status
                .response
                .as_ref()
                .is_some_and(|reply| reply.terminal && reply.no_acquisition_attempted);
            let stderr = String::from_utf8_lossy(&status.stderr).into_owned();
            let truncated = status.stderr_truncated;
            drop(status);
            if let Some(response) = response {
                self.absorb(&response);
            }
            if let Some(failure) = failure {
                append_error(&mut self.update.error, failure);
            }
            if transport_uncertain {
                if let Some(state) = &mut self.update.state {
                    state.state_is_current = false;
                    state.applied_curve = None;
                }
            }
            self.update.helper_reaped = exit.is_some();
            if let Some(exit) = exit {
                if no_acquisition && baseline_clean {
                    self.update.session_held = false;
                    self.update.cleanup_complete = true;
                }
                if !observed && matches!(exit.code(), Some(126 | 127)) {
                    let mut failure = error(
                        if exit.code() == Some(126) {
                            "fan_helper_auth_cancelled"
                        } else {
                            "fan_helper_auth_failed"
                        },
                        "pkexec did not grant helper execution",
                    );
                    failure.details.push(serde_json::json!({"exit_code":exit.code(),"stderr":stderr,"stderr_truncated":truncated}).to_string());
                    self.update.error = Some(failure);
                    if baseline_clean {
                        self.update.session_held = false;
                        self.update.cleanup_complete = true;
                    }
                } else if !exit.success() {
                    let mut failure=error("fan_helper_exited","Helper exited unsuccessfully; process exit is separate from restoration proof");
                    failure.details.push(serde_json::json!({"exit_code":exit.code(),"stderr":stderr,"stderr_truncated":truncated}).to_string());
                    append_error(&mut self.update.error, failure);
                }
                if !self.update.cleanup_complete {
                    self.update.session_held = true;
                    if let Some(state) = &mut self.update.state {
                        state.state_is_current = false;
                        state.applied_curve = None;
                    }
                }
            }
            true
        }
        pub(super) fn poll(&mut self) -> Option<FanClientUpdate> {
            self.sync(false).then(|| self.update.clone())
        }
        pub(super) fn probe(&mut self) -> FanClientUpdate {
            if self.session.is_some() {
                return self.refresh();
            }
            let _ = self.sync(false);
            match linux::inspect() {
                Ok(state) => {
                    let modified =
                        state.applied || state.phase == linux::FanCurvePhase::RecoveryRequired;
                    let state = BrokerState::from_linux(&state, true, modified, None);
                    if self.session.is_none() && !self.update.session_held {
                        self.update.error = None;
                        // A fresh fd cannot own somebody else's active lease.
                        // An orphan is recoverable, but an external active fd
                        // must never become this UI's cleanup obligation.
                        self.update.session_held = false;
                        self.update.cleanup_complete = true;
                    }
                    if !clean_state(&state) {
                        self.update.restoration_confirmed = false;
                    }
                    self.update.state = Some(state);
                }
                Err(failure) => {
                    let mut result = error("fan_probe_failed", failure.to_string());
                    result.details.push(serde_json::json!({"operation":failure.operation,"errno":failure.raw_os_error(),
                        "invalid_response":failure.invalid_response.as_deref(),"state_is_current":failure.state_is_current}).to_string());
                    self.update.error = Some(result);
                    self.update.restoration_confirmed = false;
                }
            }
            self.update.clone()
        }
        pub(super) fn apply(&mut self, curve: &ValidatedFanCurve) -> FanClientUpdate {
            let _ = self.sync(false);
            if self
                .session
                .as_ref()
                .is_some_and(|session| lock(&session.shared.status).exit.is_some())
            {
                if let Some(mut session) = self.session.take() {
                    session.finish();
                }
            }
            if self.session.is_none() && self.update.session_held {
                self.update.error=Some(error("fan_recovery_required","Previous helper outcome is uncertain; explicitly restore before applying another curve"));
                return self.update.clone();
            }
            self.update.error = None;
            if self.session.is_none() {
                if let Err(failure) = self.spawn() {
                    self.update.error = Some(failure);
                    return self.update.clone();
                }
                // Nothing mutating is buffered behind a pending authentication
                // prompt. A client that exits during authentication leaves only
                // this read-only handshake (and heartbeats), never an Apply.
                let hello = self
                    .session
                    .as_ref()
                    .and_then(|session| session.request(BrokerCommand::Inspect));
                self.sync(true);
                if hello
                    .as_ref()
                    .is_none_or(|reply| reply.outcome != BrokerOutcome::Inspected)
                    || self.update.error.is_some()
                {
                    return self.update.clone();
                }
            }
            if self.update.state.as_ref().is_some_and(|state| {
                !state.owner
                    && state.lease_id != 0
                    && matches!(state.phase, BrokerPhase::Acquired | BrokerPhase::Active)
            }) {
                self.update.error = Some(error(
                    "fan_controller_busy",
                    "Another fd owns the fan controller; this client did not acquire or modify it",
                ));
                return self.update.clone();
            }
            self.update.session_held = true;
            self.update.cleanup_complete = false;
            self.update.restoration_confirmed = false;
            let reply = self.session.as_ref().and_then(|session| {
                session.request(BrokerCommand::Apply {
                    curve: curve.curve().clone(),
                })
            });
            let _ = reply;
            self.sync(true);
            self.update.clone()
        }
        pub(super) fn refresh(&mut self) -> FanClientUpdate {
            self.update.error = None;
            if let Some(session) = &self.session {
                let reply = session.request(BrokerCommand::Inspect);
                let _ = reply;
                self.sync(true);
                self.update.clone()
            } else {
                self.probe()
            }
        }
        pub(super) fn restore(&mut self) -> FanClientUpdate {
            let _ = self.sync(false);
            if self
                .session
                .as_ref()
                .is_some_and(|session| lock(&session.shared.status).exit.is_some())
            {
                if let Some(mut session) = self.session.take() {
                    session.finish();
                }
            }
            self.update.error = None;
            if self.session.is_none() {
                let orphan = self.update.state.as_ref().is_some_and(|state| {
                    state.phase == BrokerPhase::RecoveryRequired
                        && !state.owner
                        && state.lease_id == 0
                });
                if self.update.cleanup_complete && !orphan {
                    return self.update.clone();
                }
                // Only this explicit action may start a replacement helper. Its
                // Restore command can adopt an orphan, never submit a new curve.
                if let Err(failure) = self.spawn() {
                    self.update.error = Some(failure);
                    return self.update.clone();
                }
            }
            let reply = self
                .session
                .as_ref()
                .and_then(|session| session.request(BrokerCommand::Restore));
            let _ = reply;
            self.sync(true);
            if self.update.cleanup_complete {
                return self.shutdown();
            }
            self.update.clone()
        }
        pub(super) fn shutdown(&mut self) -> FanClientUpdate {
            let Some(session) = &mut self.session else {
                return self.update.clone();
            };
            session.stop_heartbeat();
            let may_send = {
                let status = lock(&session.shared.status);
                status.exit.is_none() && !status.terminal_received
            };
            let _reply = if may_send {
                session.request(BrokerCommand::Shutdown)
            } else {
                None
            };
            if let Some(session) = &mut self.session {
                session.finish();
            }
            self.sync(true);
            // Keep an unconfirmed reaper/session when wait itself failed. A
            // real completed wait, never Drop or closing input, permits removal.
            if self.update.helper_reaped {
                self.session.take();
            }
            self.update.clone()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::fan_curve::{broker, FanCurve, FanCurveChannel, FanCurveLayout};
        use std::{
            process::{Child, Command, Stdio},
            thread,
            time::{Duration, Instant},
        };

        // Real subprocess pipes, actual wait/reap and production reader/writer
        // threads. No system helper, privileges, device node or hardware is used.
        const HELPER: &str = r#"
import sys,json,time,threading,copy,signal
mode=sys.argv[1]
if mode in ('126','127'): sys.exit(int(mode))
if mode=='no_read':
 signal.signal(signal.SIGTERM,signal.SIG_IGN)
 time.sleep(2.4)
 sys.stderr.write('completed_without_kill\n');sys.stderr.flush();sys.exit(0)
state={'backend':'uniwill_fan_curve_ioctl_v1','device_path':'/dev/tuxedo_io','epoch':1,'generation':1,'lease_id':0,'last_request_id':0,'phase':'idle','capabilities':7,'owner':False,'revoked':False,'modified_or_uncertain':False,'state_is_current':True,'restored_verified':False,'live_error':0,'last_error':0,'restore_error':0,'live_table':[0]*96,'original_table':None,'live_controls':[0,0,0,1,0,0],'original_controls':None,'cached_original_targets':None,'applied_curve':None}
write_lock=threading.Lock();applied=False;acquired=False;workers=[];restores=0
if mode=='orphan_restore':state.update(phase='recovery_required',modified_or_uncertain=True,original_table=[0]*96,original_controls=[0,0,0,1,0,0],cached_original_targets=[0]*4)

def reply(req,outcome,terminal=False,proof=False,failure=None,fallback=False):
 global state
 value={'schema_version':1,'request_id':req['request_id'],'session_id':req['session_id'],'outcome':outcome,'state':copy.deepcopy(state),'error':failure,'recovery_error':None,'terminal':terminal,'restoration_confirmed':proof,'close_fallback_unconfirmed':fallback,'no_acquisition_attempted':not acquired}
 with write_lock:
  sys.stdout.write(json.dumps(value)+'\n');sys.stdout.flush()
 return value

def clean(proof):
 global state
 state.update(generation=state['generation']+1,lease_id=0,owner=False,phase='idle',modified_or_uncertain=False,restored_verified=proof,last_error=0,restore_error=0,applied_curve=None,original_table=None,original_controls=None,cached_original_targets=None,live_table=[0]*96,live_controls=[0,0,0,1,0,0])

def failure(code):
 return {'code':code,'message':'simulated real helper error','operation':'apply','errno':5,'state':None,'state_is_current':True,'mutation_may_have_occurred':True,'state_query_error':None,'state_query_errno':None,'invalid_response':None,'state_query_invalid_response':None}

def apply(req):
 global applied,state,acquired
 if mode in ('slow','out_of_order'): time.sleep(.16)
 if mode=='busy_after_hello':
  state.update(phase='active',owner=False,lease_id=99,modified_or_uncertain=True)
  busy=failure('fan_broker_recovery_or_busy');busy['mutation_may_have_occurred']=False
  reply(req,'terminated',True,False,busy);return
 acquired=True
 if mode=='terminated_proof':
  clean(True);reply(req,'terminated',True,True,failure('original_apply_failed'));return
 curve=req['command']['curve'];table=[0]*96
 for n,key in enumerate(('cpu','gpu')):
  channel=curve[key];base=48*n
  table[base:base+15]=channel['rise_celsius'];table[base+15]=255
  table[base+17:base+32]=channel['fall_celsius'];table[base+32:base+48]=channel['duty_raw']
 state.update(generation=2,lease_id=77,owner=True,phase='active',modified_or_uncertain=True,live_table=table,original_table=[0]*96,live_controls=[128 if curve['respective'] else 0,4,0,1,0,0],original_controls=[0,0,0,1,0,0],cached_original_targets=[0]*4,applied_curve=curve)
 applied=True
 if mode in ('unknown','version','nonce','id','target','utf8','oversize','duplicate'):
  value={'schema_version':1,'request_id':req['request_id'],'session_id':req['session_id'],'outcome':'applied','state':copy.deepcopy(state),'error':None,'recovery_error':None,'terminal':False,'restoration_confirmed':False,'close_fallback_unconfirmed':False,'no_acquisition_attempted':False}
  if mode=='unknown':value['unknown_field']=1
  if mode=='version':value['schema_version']=2
  if mode=='nonce':value['session_id']='0'*32
  if mode=='id':value['request_id']+=999
  if mode=='target':
   value['state']['applied_curve']['respective']=not curve['respective'];value['state']['live_controls'][0]^=128
  with write_lock:
   if mode=='utf8':sys.stdout.buffer.write(b'\xff\n')
   elif mode=='oversize':sys.stdout.buffer.write(b'x'*32769+b'\n')
   else:sys.stdout.write(json.dumps(value)+'\n')
   if mode=='duplicate':sys.stdout.write(json.dumps(value)+'\n')
   sys.stdout.flush()
   sys.stdout.buffer.write(b'z'*100000);sys.stdout.buffer.flush()
  return
 reply(req,'applied')
 if mode=='exit_after_apply':time.sleep(.04);sys.exit(0)

for line in sys.stdin:
 req=json.loads(line);action=req['command']['action']
 sys.stderr.write(str(req['request_id'])+':'+action+'\n');sys.stderr.flush()
 if action=='inspect':reply(req,'inspected')
 elif action=='ping':reply(req,'pong')
 elif action=='apply':
  if mode=='out_of_order':
   worker=threading.Thread(target=apply,args=(req,));workers.append(worker);worker.start()
  else:apply(req)
  if mode in ('terminated_proof','busy_after_hello'):break
 elif action in ('restore','shutdown'):
  restores+=1
  if mode=='restore_retry' and restores==1:
   state.update(phase='recovery_required',applied_curve=None,last_error=-5,restore_error=-5)
   reply(req,'failed',True,False,failure('first_restore_failed'),True);break
  if mode=='orphan_restore':applied=True;acquired=True
  clean(applied)
  reply(req,'shutdown' if action=='shutdown' else ('restored' if applied else 'released'),True,applied)
  break
for worker in workers:worker.join()
"#;
        fn curve() -> ValidatedFanCurve {
            FanCurve {
                schema_version: 1,
                layout: FanCurveLayout::UniwillRamFan1p5,
                respective: true,
                cpu: FanCurveChannel {
                    rise_celsius: [40; 15],
                    fall_celsius: [35; 15],
                    duty_raw: [101; 16],
                },
                gpu: FanCurveChannel {
                    rise_celsius: [255; 15],
                    fall_celsius: [255; 15],
                    duty_raw: [150; 16],
                },
            }
            .validate()
            .unwrap()
        }
        fn child(mode: &str) -> Child {
            Command::new("/usr/bin/python")
                .args(["-B", "-u", "-c", HELPER, mode])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        }
        fn client(mode: &str) -> Client {
            let mode = mode.to_owned();
            Client {
                launcher: Some(Box::new(move || Ok(child(&mode)))),
                interval: Duration::from_millis(10),
                ..Client::new()
            }
        }
        fn poll_until(
            client: &mut Client,
            predicate: impl Fn(&FanClientUpdate) -> bool,
        ) -> FanClientUpdate {
            let deadline = Instant::now() + Duration::from_secs(3);
            loop {
                if let Some(update) = client.poll() {
                    if predicate(&update) {
                        return update;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "lifecycle update was not delivered"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }
        #[test]
        fn installed_launcher_uses_only_fixed_program_and_arguments() {
            let command = installed_command();
            assert_eq!(command.get_program(), "/usr/bin/pkexec");
            assert_eq!(
                command.get_args().collect::<Vec<_>>(),
                vec!["--disable-internal-agent", broker::HELPER_PATH, "--serve"]
            );
        }
        #[test]
        fn constructor_does_not_start_or_authenticate() {
            let client = FanBrokerClient::new();
            assert!(client.inner.session.is_none());
            assert!(client.inner.update.helper_reaped && client.inner.update.cleanup_complete);
        }
        #[test]
        fn slow_apply_keeps_independent_heartbeats_ids_and_owned_probe_then_reaps() {
            for mode in ["slow", "out_of_order"] {
                let mut client = client(mode);
                let update = client.apply(&curve());
                assert!(update.error.is_none(), "{mode}: {:?}", update.error);
                assert!(update.session_held && !update.helper_reaped && !update.cleanup_complete);
                let session = client.session.as_ref().unwrap();
                let status = lock(&session.shared.status);
                let stderr = String::from_utf8_lossy(&status.stderr);
                assert!(
                    stderr.contains(":ping"),
                    "slow call did not independently receive heartbeats"
                );
                let ids: Vec<u64> = stderr
                    .lines()
                    .filter_map(|line| line.split_once(':'))
                    .map(|(id, _)| id.parse().unwrap())
                    .collect();
                assert!(
                    ids.windows(2).all(|pair| pair[0] < pair[1]),
                    "wire IDs reordered: {ids:?}"
                );
                assert!(
                    stderr.lines().next().unwrap().ends_with(":inspect"),
                    "mutating request was buffered before authentication handshake"
                );
                drop(status);
                let probe = client.probe();
                assert!(
                    probe.state.unwrap().owner,
                    "probe opened a foreign fd instead of using existing helper"
                );
                thread::sleep(Duration::from_millis(35));
                assert!(
                    client.poll().is_none(),
                    "Pong traffic must not replace GUI state"
                );
                let done = client.restore();
                assert!(done.error.is_none(), "{:?}", done.error);
                assert!(
                    done.helper_reaped
                        && done.cleanup_complete
                        && done.restoration_confirmed
                        && !done.session_held
                );
                assert!(client.session.is_none());
            }
        }
        #[test]
        fn malformed_and_duplicate_frames_preserve_unknown_outcome_but_are_drained_and_reaped() {
            for mode in [
                "unknown",
                "version",
                "nonce",
                "id",
                "target",
                "utf8",
                "oversize",
                "duplicate",
            ] {
                let mut client = client(mode);
                let _ = client.apply(&curve());
                let done = client.shutdown();
                assert!(done.helper_reaped, "{mode} child was not actually reaped");
                assert!(
                    done.session_held && !done.cleanup_complete && !done.restoration_confirmed,
                    "{mode}: {done:?}"
                );
                let failure = done.error.unwrap();
                assert_eq!(
                    failure.code, "invalid_fan_broker_protocol",
                    "{mode}: {failure:?}"
                );
                assert!(
                    !failure.details.is_empty(),
                    "{mode} lost invalid raw response evidence"
                );
            }
        }
        #[test]
        fn matched_terminal_failure_keeps_primary_error_and_real_restoration_proof() {
            let mut client = client("terminated_proof");
            let _ = client.apply(&curve());
            let done = client.shutdown();
            assert_eq!(done.error.as_ref().unwrap().code, "original_apply_failed");
            assert!(
                done.helper_reaped
                    && done.cleanup_complete
                    && done.restoration_confirmed
                    && !done.session_held
            );
            assert!(done
                .error
                .unwrap()
                .details
                .iter()
                .any(|detail| detail.contains("broker_response")));
        }
        #[test]
        fn auth_cancel_or_denial_is_reaped_without_claiming_a_restore_transaction() {
            for mode in ["126", "127"] {
                let mut client = client(mode);
                let _ = client.apply(&curve());
                let done = client.shutdown();
                assert!(
                    done.helper_reaped
                        && done.cleanup_complete
                        && !done.session_held
                        && !done.restoration_confirmed
                );
                assert_eq!(
                    done.error.unwrap().code,
                    if mode == "126" {
                        "fan_helper_auth_cancelled"
                    } else {
                        "fan_helper_auth_failed"
                    }
                );
            }
        }
        #[test]
        fn eof_after_a_successful_ack_invalidates_current_display_without_faking_cleanup() {
            let mut client = client("exit_after_apply");
            assert!(client.apply(&curve()).error.is_none());
            let update = poll_until(&mut client, |update| update.helper_reaped);
            assert!(update.error.is_some());
            assert!(!update.state.unwrap().state_is_current);
            assert!(
                update.session_held && !update.cleanup_complete && !update.restoration_confirmed
            );
            let done = client.shutdown();
            assert!(done.helper_reaped && !done.cleanup_complete);
        }
        #[test]
        fn failed_terminal_restore_needs_real_reap_before_an_explicit_orphan_retry() {
            let mut count = 0;
            let mut client = Client {
                launcher: Some(Box::new(move || {
                    count += 1;
                    Ok(child(if count == 1 {
                        "restore_retry"
                    } else {
                        "orphan_restore"
                    }))
                })),
                interval: Duration::from_millis(10),
                ..Client::new()
            };
            assert!(client.apply(&curve()).error.is_none());
            let first = client.restore();
            assert_eq!(first.error.unwrap().code, "first_restore_failed");
            assert!(first.session_held && !first.cleanup_complete);
            if !first.helper_reaped {
                let _ = poll_until(&mut client, |update| update.helper_reaped);
            }
            let second = client.restore();
            assert!(second.error.is_none(), "{:?}", second.error);
            assert!(
                second.helper_reaped && second.cleanup_complete && second.restoration_confirmed
            );
        }
        #[test]
        fn foreign_acquisition_between_hello_and_apply_does_not_trap_this_client() {
            let mut client = client("busy_after_hello");
            let _ = client.apply(&curve());
            let done = client.shutdown();
            assert_eq!(
                done.error.as_ref().unwrap().code,
                "fan_broker_recovery_or_busy",
                "{:?}",
                done.error
            );
            assert!(
                done.helper_reaped
                    && done.cleanup_complete
                    && !done.session_held
                    && !done.restoration_confirmed
            );
            let state = done.state.unwrap();
            assert!(!state.owner && state.lease_id == 99 && state.applied_curve.is_none());
        }
        #[test]
        fn missing_reader_pipe_does_not_leave_finish_waiting_for_a_nonexistent_drainer() {
            for stdout in [false, true] {
                let mut child = Command::new("/usr/bin/python")
                    .args(["-B", "-c", "import time;time.sleep(.05)"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                if stdout {
                    child.stdout.take();
                } else {
                    child.stderr.take();
                }
                let mut session =
                    Session::start(child, "b".repeat(32), Duration::from_millis(10), true);
                session.finish();
                assert!(lock(&session.shared.status).exit.is_some());
                assert!(session.threads.is_empty());
                assert!(session.heartbeat.is_none());
            }
        }
        #[test]
        fn nonreading_rootlike_child_cannot_hold_writer_lock_forever_and_is_never_killed() {
            let mut session = Session::start(
                child("no_read"),
                "a".repeat(32),
                Duration::from_secs(60),
                false,
            );
            {
                let writer = lock(&session.writer.state);
                let fd = writer.input.as_ref().unwrap().as_raw_fd();
                assert_ne!(
                    unsafe { libc::fcntl(fd, libc::F_GETFL) } & libc::O_NONBLOCK,
                    0
                );
                assert!(unsafe { libc::fcntl(fd, libc::F_SETPIPE_SZ, 4096) } >= 4096);
            }
            let started = Instant::now();
            let failure = loop {
                match session.writer.send(broker::BrokerCommand::Apply {
                    curve: curve().curve().clone(),
                }) {
                    Ok(_) => {}
                    Err(failure) => break failure,
                }
            };
            assert_eq!(failure.code, "fan_helper_input_stalled");
            assert!(
                started.elapsed() < Duration::from_millis(2300),
                "write deadline did not bound the full pipe"
            );
            session.finish();
            let status = lock(&session.shared.status);
            assert!(status.exit.unwrap().success());
            assert!(String::from_utf8_lossy(&status.stderr).contains("completed_without_kill"));
            assert!(
                started.elapsed() >= Duration::from_millis(2300),
                "actual child wait was replaced by cancellation"
            );
        }
        #[test]
        fn drop_closes_input_but_background_drainers_and_reaper_live_until_real_exit() {
            let session = Session::start(
                child("no_read"),
                "a".repeat(32),
                Duration::from_secs(60),
                false,
            );
            let shared = session.shared.clone();
            drop(session);
            assert!(
                lock(&shared.status).exit.is_none(),
                "Drop fabricated a successful wait"
            );
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                {
                    let status = lock(&shared.status);
                    if status.exit.is_some() && status.stdout_done && status.stderr_done {
                        break;
                    }
                }
                assert!(
                    Instant::now() < deadline,
                    "retained reaper/drainers did not finish"
                );
                thread::sleep(Duration::from_millis(10));
            }
            assert!(lock(&shared.status).exit.unwrap().success());
        }
    }
}
