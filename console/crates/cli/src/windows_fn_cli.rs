//! Explicitly authorized Windows Fn-lock commands. Normal OEM discovery and
//! its dry-run path remain separate and never start an elevated helper.
use super::{text, usage, write_json};
use mechrevo_console_core::{
    windows_oem::{Client, ClientHelperLifetime, Outcome, Update},
    ControlError, OemPlan,
};
use serde_json::{json, Value};
use std::{ffi::OsString, io::Write};

pub(super) const HELP: &str = "\nWindows Fn lock (separate explicit authorization):\n  mechrevo-console oem windows-fn probe\n  mechrevo-console oem windows-fn set <on|off> [--dry-run]\n\nThese commands authorize a Windows helper for the read-only Probe, including\nwindows-fn set --dry-run. Preview itself is local and dry-run never applies.\nThis differs from oem set --dry-run, which never elevates. Qualification uses\nthe fixed release contract and installed official driver; unsupported systems\nare refused. Writes persist after exit. EC-byte verification does not prove\nphysical key behavior or reboot persistence. Failed or unknown writes are not\nretried. The command waits for its helper's actual exit when a handle exists.\n";

const CONTEXT_MARKER: &str = "windows_fn_cli_context=";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Action {
    Probe,
    Set { target: bool, dry_run: bool },
}

pub(super) fn parse(args: &[OsString]) -> Result<Action, ControlError> {
    match args {
        [command] if command == "probe" => Ok(Action::Probe),
        [command, target] if command == "set" => Ok(Action::Set {
            target: parse_target(target)?,
            dry_run: false,
        }),
        [command, target, option] if command == "set" && option == "--dry-run" => Ok(Action::Set {
            target: parse_target(target)?,
            dry_run: true,
        }),
        _ => Err(usage(
            "Use oem windows-fn probe or oem windows-fn set on|off [--dry-run]",
        )),
    }
}

fn parse_target(arg: &OsString) -> Result<bool, ControlError> {
    match text(arg)? {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(usage("Windows Fn lock accepts exactly on or off")),
    }
}

trait Backend {
    fn probe_authorized(&mut self) -> Update;
    fn preview(&self, target: bool) -> Result<OemPlan, ControlError>;
    fn apply_authorized(&mut self, plan: &OemPlan) -> Update;
    fn helper_reaped(&self) -> bool;
    fn helper_lifetime(&self) -> ClientHelperLifetime;
    fn finish_pending(&mut self) -> Update;
}

impl Backend for Client {
    fn probe_authorized(&mut self) -> Update {
        Client::probe_authorized(self)
    }
    fn preview(&self, target: bool) -> Result<OemPlan, ControlError> {
        Client::preview(self, target)
    }
    fn apply_authorized(&mut self, plan: &OemPlan) -> Update {
        Client::apply_authorized(self, plan)
    }
    fn helper_reaped(&self) -> bool {
        Client::helper_reaped(self)
    }
    fn helper_lifetime(&self) -> ClientHelperLifetime {
        Client::helper_lifetime(self)
    }
    fn finish_pending(&mut self) -> Update {
        Client::finish_pending(self)
    }
}

struct Step {
    initial: Update,
    completion: Option<Update>,
}

impl Step {
    fn completed(&self) -> &Update {
        self.completion.as_ref().unwrap_or(&self.initial)
    }
    fn document(&self) -> Value {
        json!({"initial":self.initial,"completion":self.completion})
    }
}

fn settle(backend: &mut impl Backend, initial: Update) -> Step {
    // A response and the helper's actual process exit are independent. This
    // only drains/waits for the existing helper; it never retries an operation.
    let completion = if !initial.helper_reaped {
        Some(backend.finish_pending())
    } else {
        None
    };
    Step {
        initial,
        completion,
    }
}

fn update_error(update: &Update, expected: Outcome) -> Option<ControlError> {
    if let Some(error) = &update.error {
        return Some(error.clone());
    }
    if !update.helper_reaped
        || matches!(
            update.helper_lifetime,
            ClientHelperLifetime::Running | ClientHelperLifetime::Unknown
        )
    {
        return Some(ControlError::new(
            "windows_fnlock_helper_liveness_unknown",
            "The helper has not been confirmed exited; no later operation was attempted",
        ));
    }
    if update.receipt.blocks_further_writes() {
        return Some(ControlError::new(
            "windows_fnlock_state_unknown",
            "The transaction receipt does not permit another write",
        ));
    }
    if update.outcome != expected {
        return Some(ControlError::new(
            "windows_fnlock_unverified_outcome",
            "The typed outcome did not confirm the requested operation",
        ));
    }
    if expected == Outcome::Observed && update.status.is_none() {
        return Some(ControlError::new(
            "windows_fnlock_status_unavailable",
            "The authorized probe returned no complete Fn-lock observation",
        ));
    }
    None
}

fn with_context(mut error: ControlError, context: &Value) -> ControlError {
    error.details.push(format!("{CONTEXT_MARKER}{context}"));
    error
}

/// Preserve the typed transaction and primary error independently from a
/// broken stdout. Neither output failure nor error reporting retries hardware.
fn emit(
    writer: &mut impl Write,
    document: Value,
    primary: Option<ControlError>,
) -> Result<(), ControlError> {
    if let Err(error) = write_json(writer, &document) {
        return Err(with_context(
            error,
            &json!({"completed_document":document,"primary_error":primary}),
        ));
    }
    match primary {
        Some(error) => Err(with_context(error, &document)),
        None => Ok(()),
    }
}

pub(super) fn error_document(error: &ControlError, mut document: Value) -> Value {
    if let Some(context) = error.details.iter().rev().find_map(|detail| {
        detail
            .strip_prefix(CONTEXT_MARKER)
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    }) {
        document["windows_fn_context"] = context;
    }
    document
}

// @mark mechrevo.windows_fnlock role=cli
fn run_with(
    command: Action,
    backend: &mut impl Backend,
    writer: &mut impl Write,
) -> Result<(), ControlError> {
    let previous_lifetime = backend.helper_lifetime();
    let previous_reaped = backend.helper_reaped();
    if !previous_reaped
        || matches!(
            previous_lifetime,
            ClientHelperLifetime::Running | ClientHelperLifetime::Unknown
        )
    {
        let completion = backend.finish_pending();
        let error = ControlError::new(
            "windows_fnlock_previous_helper_pending",
            "An earlier helper still owned the lifecycle gate; the requested command was not started",
        );
        return emit(
            writer,
            json!({"phase":"existing_helper","probe":null,"plan":null,"previous_lifetime":previous_lifetime,"previous_helper_reaped":previous_reaped,"completion":completion,"error":error}),
            Some(error),
        );
    }

    // parse() has already validated every token before this explicit Probe.
    // In this command family a dry-run authorizes Probe, never Apply.
    let initial = backend.probe_authorized();
    let probe = settle(backend, initial);
    let probe_error = update_error(probe.completed(), Outcome::Observed);
    if command == Action::Probe || probe_error.is_some() {
        return emit(
            writer,
            json!({"phase":"probe","probe":probe.document(),"plan":null,"error":probe_error}),
            probe_error,
        );
    }
    let Action::Set { target, dry_run } = command else {
        unreachable!("probe returned above")
    };
    let plan = match backend.preview(target) {
        Ok(plan) => plan,
        Err(error) => {
            return emit(
                writer,
                json!({"phase":"preview","dry_run":dry_run,"probe":probe.document(),"plan":null,"error":error}),
                Some(error),
            )
        }
    };
    if dry_run {
        return emit(
            writer,
            json!({"phase":"preview","dry_run":true,"probe":probe.document(),"plan":plan,"error":null}),
            None,
        );
    }
    let initial = backend.apply_authorized(&plan);
    let applied = settle(backend, initial);
    let error = update_error(applied.completed(), Outcome::AppliedVerified);
    emit(
        writer,
        json!({"phase":"apply","dry_run":false,"probe":probe.document(),"plan":plan,"apply":applied.document(),"error":error}),
        error,
    )
}

pub(super) fn run(command: Action, writer: &mut impl Write) -> Result<(), ControlError> {
    run_with(command, &mut Client::new(), writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mechrevo_console_core::{
        windows_oem::{
            self, contract, IdentityEvidence, LoadedModuleEvidence, Receipt, Recovery, Status,
            SupportStage,
        },
        CapabilityStatus, EvidenceSource, OemApplyResult, OemControl, OemControlKind, OemDomain,
        OemValue,
    };
    use std::{cell::RefCell, io};

    const INSTANCE_ID: &str = r"ACPI\INOU0000\OFFLINE-CLI-FIXTURE";

    fn status(previous: bool) -> Status {
        Status {
            contract_id: Some("fixture-exact-contract".into()),
            support_stage: Some(SupportStage::Candidate),
            qualified: true,
            control: Some(OemControl {
                id: contract::CONTROL_ID.into(),
                kind: OemControlKind::FnLock,
                label: contract::CONTROL_LABEL.into(),
                current: Some(OemValue::Boolean(previous)),
                domain: OemDomain::Boolean,
                status: CapabilityStatus::Available,
                writable: true,
                source: EvidenceSource {
                    backend: contract::SOURCE_BACKEND.into(),
                    path: contract::SOURCE_PATH.into(),
                    device_id: Some(INSTANCE_ID.into()),
                },
                binding: "a".repeat(64),
                readback_scope: contract::READBACK_SCOPE.into(),
                reason: Some("Offline dispatch fixture; not Windows runtime acceptance".into()),
            }),
            evidence: IdentityEvidence {
                disk_file_sha256: Some(
                    "6e360ee4a0c42b2a4eaf46208c1592c87f8cd71aca2a6c52831a2ad356e61235".into(),
                ),
                loaded_module_metadata: Some(LoadedModuleEvidence {
                    service: "UWACPIDriver".into(),
                    device_instance_id: INSTANCE_ID.into(),
                    driver_inf: "oem-offline-cli.inf".into(),
                    driver_version: "10.48.11.331".into(),
                    loaded_path: r"C:\Windows\System32\drivers\UWACPIDriver.sys".into(),
                    association_level: "offline synthetic association, not Windows acceptance"
                        .into(),
                }),
                ..IdentityEvidence::default()
            },
            blocking_errors: vec![],
            diagnostics: vec!["fixture diagnostic retained".into()],
        }
    }

    fn observed(previous: bool) -> Update {
        Update {
            status: Some(status(previous)),
            outcome: Outcome::Observed,
            result: None,
            receipt: Receipt::default(),
            error: None,
            helper_reaped: true,
            helper_lifetime: ClientHelperLifetime::Reaped,
        }
    }

    struct Stub {
        probe: Update,
        applied: Update,
        pending: Update,
        cached: Option<Status>,
        lifetime: ClientHelperLifetime,
        reaped: bool,
        probes: usize,
        previews: RefCell<Vec<bool>>,
        applications: Vec<OemPlan>,
        finishes: usize,
        preview_error: Option<ControlError>,
    }

    impl Stub {
        fn new(previous: bool) -> Self {
            let probe = observed(previous);
            let mut applied = probe.clone();
            applied.outcome = Outcome::AppliedVerified;
            applied.receipt.write_attempted = true;
            applied.receipt.write_call_succeeded = Some(true);
            applied.receipt.full_byte_verified = Some(true);
            applied.receipt.protected_bits_preserved = Some(true);
            Self {
                pending: probe.clone(),
                probe,
                applied,
                cached: None,
                lifetime: ClientHelperLifetime::NotStarted,
                reaped: true,
                probes: 0,
                previews: RefCell::new(vec![]),
                applications: vec![],
                finishes: 0,
                preview_error: None,
            }
        }
        fn take_update(&mut self, update: Update) -> Update {
            self.cached = update.status.clone();
            self.lifetime = update.helper_lifetime;
            self.reaped = update.helper_reaped;
            update
        }
    }

    impl Backend for Stub {
        fn probe_authorized(&mut self) -> Update {
            self.probes += 1;
            self.take_update(self.probe.clone())
        }
        fn preview(&self, target: bool) -> Result<OemPlan, ControlError> {
            self.previews.borrow_mut().push(target);
            if let Some(error) = &self.preview_error {
                return Err(error.clone());
            }
            windows_oem::preview(
                self.cached.as_ref().expect("completed probe fixture"),
                target,
            )
        }
        fn apply_authorized(&mut self, plan: &OemPlan) -> Update {
            self.applications.push(plan.clone());
            let mut update = self.applied.clone();
            if update.outcome == Outcome::AppliedVerified && update.error.is_none() {
                // The no-op case still confirms the full byte, without a write.
                update.receipt.write_attempted = plan.previous != plan.target;
                update.receipt.write_call_succeeded =
                    update.receipt.write_attempted.then_some(true);
                update.result = Some(OemApplyResult {
                    control_id: plan.control_id.clone(),
                    kind: plan.kind,
                    previous: plan.previous.clone(),
                    confirmed: plan.target.clone(),
                    source: plan.source.clone(),
                    readback_scope: plan.readback_scope.clone(),
                });
                if let Some(control) = update.status.as_mut().and_then(|s| s.control.as_mut()) {
                    control.current = Some(plan.target.clone());
                }
            }
            self.take_update(update)
        }
        fn helper_reaped(&self) -> bool {
            self.reaped
        }
        fn helper_lifetime(&self) -> ClientHelperLifetime {
            self.lifetime
        }
        fn finish_pending(&mut self) -> Update {
            self.finishes += 1;
            self.take_update(self.pending.clone())
        }
    }

    fn run_args(
        args: &[&str],
        backend: &mut Stub,
        writer: &mut impl Write,
    ) -> Result<(), ControlError> {
        let parsed = super::super::parse(args.iter().map(OsString::from).collect())?;
        let super::super::Action::WindowsFn(command) = parsed else {
            panic!("Windows Fn-lock command expected")
        };
        run_with(command, backend, writer)
    }

    fn document(bytes: &[u8]) -> Value {
        serde_json::from_slice(bytes).unwrap()
    }

    #[test]
    fn windows_fn_syntax_is_exact_and_validated_before_authorization() {
        for args in [
            vec!["oem", "windows-fn"],
            vec!["oem", "windows-fn", "list"],
            vec!["oem", "windows-fn", "probe", "--dry-run"],
            vec!["oem", "windows-fn", "probe", "--json"],
            vec!["oem", "windows-fn", "set"],
            vec!["oem", "windows-fn", "set", ""],
            vec!["oem", "windows-fn", "set", "true"],
            vec!["oem", "windows-fn", "set", "1"],
            vec!["oem", "windows-fn", "set", "ON"],
            vec!["oem", "windows-fn", "set", " on"],
            vec!["oem", "windows-fn", "set", "off \u{200B}"],
            vec!["oem", "windows-fn", "set", "--dry-run"],
            vec!["oem", "windows-fn", "set", "on", "--force"],
            vec!["oem", "windows-fn", "set", "on", "--dry-run", "--dry-run"],
            vec!["oem", "windows-fn", "set", "--dry-run", "on"],
        ] {
            let mut backend = Stub::new(false);
            let mut bytes = vec![];
            assert!(
                run_args(&args, &mut backend, &mut bytes).is_err(),
                "Accepted {args:?}"
            );
            assert_eq!(backend.probes, 0);
            assert!(backend.previews.borrow().is_empty());
            assert!(backend.applications.is_empty());
            assert_eq!(backend.finishes, 0);
            assert!(bytes.is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn non_unicode_target_is_rejected_during_parse() {
        use std::os::unix::ffi::OsStringExt;
        assert!(super::super::parse(vec![
            "oem".into(),
            "windows-fn".into(),
            "set".into(),
            OsString::from_vec(vec![0xff]),
        ])
        .is_err());
    }

    #[test]
    fn probe_only_authorizes_read_and_preserves_status_evidence() {
        let mut backend = Stub::new(false);
        let mut bytes = vec![];
        run_args(&["oem", "windows-fn", "probe"], &mut backend, &mut bytes).unwrap();
        let result = document(&bytes);
        assert_eq!(backend.probes, 1);
        assert!(backend.previews.borrow().is_empty());
        assert!(backend.applications.is_empty());
        assert_eq!(result["probe"]["initial"]["outcome"], "observed");
        assert_eq!(
            result["probe"]["initial"]["status"]["support_stage"],
            "candidate"
        );
        assert_eq!(
            result["probe"]["initial"]["status"]["diagnostics"][0],
            "fixture diagnostic retained"
        );
        assert_eq!(result["probe"]["initial"]["helper_lifetime"], "reaped");
        assert!(result["probe"]["completion"].is_null());
    }

    #[test]
    fn dry_run_authorizes_probe_then_previews_on_and_off_without_apply() {
        for (target, word) in [(true, "on"), (false, "off")] {
            let mut backend = Stub::new(!target);
            let mut bytes = vec![];
            run_args(
                &["oem", "windows-fn", "set", word, "--dry-run"],
                &mut backend,
                &mut bytes,
            )
            .unwrap();
            let result = document(&bytes);
            assert_eq!(backend.probes, 1);
            assert_eq!(*backend.previews.borrow(), vec![target]);
            assert!(backend.applications.is_empty());
            assert_eq!(result["dry_run"], true);
            assert_eq!(
                result["plan"]["previous"],
                json!(OemValue::Boolean(!target))
            );
            assert_eq!(result["plan"]["target"], json!(OemValue::Boolean(target)));
            assert_eq!(result["plan"]["source"]["path"], contract::SOURCE_PATH);
            assert_eq!(result["plan"]["binding"], "a".repeat(64));
            assert_eq!(result["plan"]["requires_privilege"], true);
        }
    }

    #[test]
    fn execute_uses_the_exact_completed_probe_plan_once() {
        for (previous, target, word) in [
            (false, true, "on"),
            (true, false, "off"),
            (false, false, "off"),
            (true, true, "on"),
        ] {
            let mut backend = Stub::new(previous);
            let mut bytes = vec![];
            run_args(
                &["oem", "windows-fn", "set", word],
                &mut backend,
                &mut bytes,
            )
            .unwrap();
            assert_eq!(backend.probes, 1);
            assert_eq!(backend.applications.len(), 1);
            assert_eq!(
                backend.applications[0],
                windows_oem::preview(&status(previous), target).unwrap()
            );
            let result = document(&bytes);
            assert_eq!(result["apply"]["initial"]["outcome"], "applied_verified");
            assert_eq!(
                result["apply"]["initial"]["result"]["confirmed"],
                json!(OemValue::Boolean(target))
            );
            assert_eq!(
                result["apply"]["initial"]["receipt"]["write_attempted"],
                previous != target
            );
            assert_eq!(
                result["apply"]["initial"]["receipt"]["full_byte_verified"],
                true
            );
            assert_eq!(
                result["apply"]["initial"]["receipt"]["protected_bits_preserved"],
                true
            );
            assert_eq!(backend.finishes, 0);
        }
    }

    #[test]
    fn failed_or_unknown_probe_never_previews_or_applies() {
        for outcome in [Outcome::RejectedBeforeWrite, Outcome::FailedStateUnknown] {
            let mut backend = Stub::new(false);
            backend.probe.outcome = outcome;
            backend.probe.error = Some(ControlError::new(
                "fixture_probe_failed",
                "original probe failure",
            ));
            backend.probe.receipt.primary_error = backend.probe.error.clone();
            let mut bytes = vec![];
            let error = run_args(
                &["oem", "windows-fn", "set", "on"],
                &mut backend,
                &mut bytes,
            )
            .unwrap_err();
            assert_eq!(error.code, "fixture_probe_failed");
            assert_eq!(backend.probes, 1);
            assert!(backend.previews.borrow().is_empty());
            assert!(backend.applications.is_empty());
            let result = document(&bytes);
            assert!(result["plan"].is_null());
            assert_eq!(result["probe"]["initial"]["outcome"], json!(outcome));
            assert_eq!(
                result["probe"]["initial"]["receipt"]["primary_error"]["code"],
                "fixture_probe_failed"
            );
        }
    }

    #[test]
    fn malformed_observation_and_unqualified_cache_block_apply() {
        let mut missing = Stub::new(false);
        missing.probe.status = None;
        let error = run_args(
            &["oem", "windows-fn", "set", "on"],
            &mut missing,
            &mut vec![],
        )
        .unwrap_err();
        assert_eq!(error.code, "windows_fnlock_status_unavailable");
        assert!(missing.previews.borrow().is_empty());
        assert!(missing.applications.is_empty());

        let mut unqualified = Stub::new(false);
        unqualified.probe.status.as_mut().unwrap().qualified = false;
        let error = run_args(
            &["oem", "windows-fn", "set", "on"],
            &mut unqualified,
            &mut vec![],
        )
        .unwrap_err();
        assert_eq!(error.code, "windows_fnlock_not_qualified");
        assert_eq!(*unqualified.previews.borrow(), vec![true]);
        assert!(unqualified.applications.is_empty());
    }

    #[test]
    fn preview_failure_retains_completed_probe_without_apply() {
        let mut backend = Stub::new(false);
        backend.preview_error = Some(ControlError::new(
            "fixture_preview_failed",
            "preview detail",
        ));
        let mut bytes = vec![];
        let error = run_args(
            &["oem", "windows-fn", "set", "on", "--dry-run"],
            &mut backend,
            &mut bytes,
        )
        .unwrap_err();
        assert_eq!(error.code, "fixture_preview_failed");
        assert!(backend.applications.is_empty());
        let result = document(&bytes);
        assert_eq!(result["phase"], "preview");
        assert_eq!(result["probe"]["initial"]["outcome"], "observed");
        assert!(result["plan"].is_null());
    }

    #[test]
    fn failed_apply_retains_recovery_and_all_errors_without_retry() {
        for outcome in [Outcome::FailedRestoredVerified, Outcome::FailedStateUnknown] {
            let mut backend = Stub::new(false);
            backend.applied.outcome = outcome;
            backend.applied.error = Some(ControlError::new(
                "fixture_write_failed",
                "primary write failure",
            ));
            backend.applied.receipt.primary_error = backend.applied.error.clone();
            backend.applied.receipt.recovery = if outcome == Outcome::FailedRestoredVerified {
                Recovery::RestoredVerified
            } else {
                Recovery::Failed
            };
            backend.applied.receipt.readback_errors = vec![ControlError::new(
                "fixture_readback_failed",
                "readback detail",
            )];
            if outcome == Outcome::FailedStateUnknown {
                backend.applied.receipt.restore_error = Some(ControlError::new(
                    "fixture_restore_failed",
                    "restore detail",
                ));
            }
            let mut bytes = vec![];
            let error = run_args(
                &["oem", "windows-fn", "set", "on"],
                &mut backend,
                &mut bytes,
            )
            .unwrap_err();
            assert_eq!(error.code, "fixture_write_failed");
            assert_eq!(backend.applications.len(), 1);
            let result = document(&bytes);
            assert_eq!(result["apply"]["initial"]["outcome"], json!(outcome));
            assert_eq!(
                result["apply"]["initial"]["receipt"]["primary_error"]["code"],
                "fixture_write_failed"
            );
            assert_eq!(
                result["apply"]["initial"]["receipt"]["readback_errors"][0]["code"],
                "fixture_readback_failed"
            );
            assert_eq!(result["plan"]["target"], json!(OemValue::Boolean(true)));
        }
    }

    #[test]
    fn response_before_exit_waits_only_for_the_existing_helper() {
        let mut backend = Stub::new(false);
        backend.probe.helper_reaped = false;
        backend.probe.helper_lifetime = ClientHelperLifetime::Running;
        backend.pending = observed(false);
        let mut bytes = vec![];
        run_args(
            &["oem", "windows-fn", "set", "on", "--dry-run"],
            &mut backend,
            &mut bytes,
        )
        .unwrap();
        assert_eq!(backend.probes, 1);
        assert_eq!(backend.finishes, 1);
        assert_eq!(*backend.previews.borrow(), vec![true]);
        assert!(backend.applications.is_empty());
        let result = document(&bytes);
        assert_eq!(result["probe"]["initial"]["helper_lifetime"], "running");
        assert_eq!(result["probe"]["completion"]["helper_lifetime"], "reaped");
    }

    #[test]
    fn unknown_lifetime_keeps_gate_and_does_not_start_another_operation() {
        let mut backend = Stub::new(false);
        backend.probe.helper_reaped = false;
        backend.probe.helper_lifetime = ClientHelperLifetime::Unknown;
        backend.pending = backend.probe.clone();
        let mut bytes = vec![];
        let error = run_args(
            &["oem", "windows-fn", "set", "on"],
            &mut backend,
            &mut bytes,
        )
        .unwrap_err();
        assert_eq!(error.code, "windows_fnlock_helper_liveness_unknown");
        assert_eq!(backend.probes, 1);
        assert_eq!(backend.finishes, 1);
        assert!(backend.previews.borrow().is_empty());
        assert!(backend.applications.is_empty());
        let error =
            run_args(&["oem", "windows-fn", "probe"], &mut backend, &mut vec![]).unwrap_err();
        assert_eq!(error.code, "windows_fnlock_previous_helper_pending");
        assert_eq!(backend.probes, 1, "No new Probe while lifetime is unknown");
        assert_eq!(
            backend.finishes, 2,
            "Only the original helper may be drained again"
        );
    }

    #[test]
    fn unknown_writer_receipt_blocks_apply_even_with_success_shaped_outcome() {
        let mut backend = Stub::new(false);
        backend.probe.receipt.writer_may_continue = true;
        let error = run_args(
            &["oem", "windows-fn", "set", "on"],
            &mut backend,
            &mut vec![],
        )
        .unwrap_err();
        assert_eq!(error.code, "windows_fnlock_state_unknown");
        assert!(backend.previews.borrow().is_empty());
        assert!(backend.applications.is_empty());
    }

    #[test]
    fn apply_with_unknown_lifetime_retains_both_updates_and_never_restarts() {
        let mut backend = Stub::new(false);
        backend.applied.helper_reaped = false;
        backend.applied.helper_lifetime = ClientHelperLifetime::Unknown;
        backend.applied.receipt.writer_may_continue = true;
        backend.pending = backend.applied.clone();
        backend.pending.outcome = Outcome::FailedStateUnknown;
        backend.pending.error = Some(ControlError::new(
            "fixture_wait_failed",
            "actual wait failed",
        ));
        let mut bytes = vec![];
        let error = run_args(
            &["oem", "windows-fn", "set", "on"],
            &mut backend,
            &mut bytes,
        )
        .unwrap_err();
        assert_eq!(error.code, "fixture_wait_failed");
        assert_eq!(backend.probes, 1);
        assert_eq!(backend.applications.len(), 1);
        assert_eq!(backend.finishes, 1);
        let result = document(&bytes);
        assert_eq!(result["apply"]["initial"]["outcome"], "applied_verified");
        assert_eq!(
            result["apply"]["completion"]["outcome"],
            "failed_state_unknown"
        );
        assert_eq!(
            result["apply"]["completion"]["receipt"]["writer_may_continue"],
            true
        );
        assert_eq!(result["apply"]["completion"]["helper_lifetime"], "unknown");
        assert!(run_args(
            &["oem", "windows-fn", "set", "off"],
            &mut backend,
            &mut vec![]
        )
        .is_err());
        assert_eq!(backend.probes, 1);
        assert_eq!(backend.applications.len(), 1);
    }

    struct BrokenOutput {
        remaining: usize,
    }
    impl Write for BrokenOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "fixture output closed",
                ));
            }
            let count = bytes.len().min(self.remaining);
            self.remaining -= count;
            Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn output_failure_after_successful_probe_or_dry_run_never_applies() {
        for args in [
            vec!["oem", "windows-fn", "probe"],
            vec!["oem", "windows-fn", "set", "on", "--dry-run"],
        ] {
            let mut backend = Stub::new(false);
            let error =
                run_args(&args, &mut backend, &mut BrokenOutput { remaining: 0 }).unwrap_err();
            assert_eq!(error.code, "output_failed");
            assert_eq!(backend.probes, 1);
            assert!(backend.applications.is_empty());
            let report = error_document(&error, json!({"error":error}));
            assert_eq!(
                report["windows_fn_context"]["completed_document"]["probe"]["initial"]["outcome"],
                "observed"
            );
            assert!(report["windows_fn_context"]["primary_error"].is_null());
        }
    }

    #[test]
    fn output_failure_keeps_completed_apply_receipt_and_never_retries() {
        let args = ["oem", "windows-fn", "set", "on"];
        let mut normal = vec![];
        run_args(&args, &mut Stub::new(false), &mut normal).unwrap();
        for remaining in [0, normal.len() - 1] {
            let mut backend = Stub::new(false);
            let error = run_args(&args, &mut backend, &mut BrokenOutput { remaining }).unwrap_err();
            assert_eq!(error.code, "output_failed");
            assert_eq!(backend.probes, 1);
            assert_eq!(backend.applications.len(), 1);
            let report = error_document(&error, json!({"error":error}));
            let completed = &report["windows_fn_context"]["completed_document"];
            assert_eq!(completed["apply"]["initial"]["outcome"], "applied_verified");
            assert_eq!(completed["apply"]["initial"]["helper_reaped"], true);
            assert_eq!(
                completed["apply"]["initial"]["receipt"]["full_byte_verified"],
                true
            );
            assert_eq!(
                completed["apply"]["initial"]["result"]["confirmed"],
                json!(OemValue::Boolean(true))
            );
        }
    }

    #[test]
    fn broken_output_preserves_original_probe_or_preview_failure() {
        for probe_fails in [true, false] {
            let mut backend = Stub::new(false);
            let primary = ControlError::new("fixture_original_failure", "primary detail retained");
            if probe_fails {
                backend.probe.error = Some(primary.clone());
            } else {
                backend.preview_error = Some(primary);
            }
            let error = run_args(
                &["oem", "windows-fn", "set", "on", "--dry-run"],
                &mut backend,
                &mut BrokenOutput { remaining: 0 },
            )
            .unwrap_err();
            assert_eq!(error.code, "output_failed");
            assert!(backend.applications.is_empty());
            let report = error_document(&error, json!({"error":error}));
            assert_eq!(
                report["windows_fn_context"]["primary_error"]["code"],
                "fixture_original_failure"
            );
            assert_eq!(
                report["windows_fn_context"]["completed_document"]["probe"]["initial"]
                    ["helper_lifetime"],
                "reaped"
            );
        }
    }

    #[test]
    fn help_distinguishes_explicit_probe_authorization_from_normal_oem_dry_run() {
        assert!(HELP.contains("including\nwindows-fn set --dry-run"));
        assert!(HELP.contains("oem set --dry-run, which never elevates"));
        let parsed = super::super::parse(
            ["oem", "set", "fn_lock", "on", "--dry-run"]
                .into_iter()
                .map(OsString::from)
                .collect(),
        )
        .unwrap();
        assert_eq!(
            parsed,
            super::super::Action::OemSet("fn_lock".into(), "on".into(), true)
        );
    }
}
