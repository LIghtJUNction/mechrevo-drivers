use super::*;
use serde_json::json;

fn policy(cap: u64) -> Policy {
    Policy {
        cap_uw: cap,
        target_celsius: Some(90),
        polling_seconds: Some(1),
        rapl_debounce_seconds: Some(0),
    }
}

fn status(management: Management) -> Status {
    Status {
        management,
        revision: Some("a".repeat(64)),
        qualified: true,
        allowed_actions: match management {
            Management::Native => vec![ActionKind::Set],
            Management::Managed => vec![ActionKind::Set, ActionKind::Clear, ActionKind::Reapply],
            Management::RecoveryPending => vec![ActionKind::Recover],
            Management::Unknown => vec![],
        },
        current_policy: if management == Management::Native {
            None
        } else {
            Some(policy(55_000_000))
        },
        operational_ceiling_uw: Some(140_000_000),
        minimum_limit_uw: 35_000_000,
        thermal_spec_reference_uw: 55_000_000,
        observed: vec![
            ObservedLimit {
                interface: Interface::Msr,
                pl1_uw: Some(54_000_000),
            },
            ObservedLimit {
                interface: Interface::Mmio,
                pl1_uw: Some(55_000_000),
            },
        ],
        thermald_active: Some(true),
        holding_now: management == Management::Managed,
        blocking_errors: vec![],
        diagnostic_errors: vec![],
    }
}

fn request(operation: Operation) -> Request {
    Request {
        schema_version: 1,
        request_id: "test-1".into(),
        operation,
    }
}

fn reply(
    request: &Request,
    outcome: Outcome,
    status: Option<&Status>,
    receipt: Value,
    error: Value,
) -> Value {
    json!({"schema_version":1,"request_id":request.request_id,"outcome":outcome,
        "status":status,"receipt":receipt,"error":error})
}

fn output(value: Value, exit: i32) -> CommandOutput {
    CommandOutput {
        stdout: value.to_string(),
        stderr: "diagnostic stderr".into(),
        status: exit,
    }
}

fn receipt(action: &Action) -> Value {
    let operation = match action {
        Action::Set { .. } => "set",
        Action::Clear => "clear",
        Action::Recover => "recover",
        Action::Reapply => "apply",
    };
    let cap = match action {
        Action::Set { policy } => policy.cap_uw,
        _ => 55_000_000,
    };
    let mut receipt = json!({"version":1,"operation":operation,"success":true,"phase":"completed",
        "transaction":"actual-transaction","cap_uw":cap,"writes":[],
        "observed":{(ZONES[0]):{"pl1_uw":54_000_000}, (ZONES[1]):{"pl1_uw":55_000_000}}});
    if matches!(action, Action::Reapply) {
        receipt.as_object_mut().unwrap().remove("phase");
        receipt["transaction"] = Value::Null;
    }
    receipt
}

fn failure() -> Value {
    json!({"code":"power_control_failed","message":"primary","details":["primary_error: disk","receipt_error: disk","cpu_power_restore_confirmed"]})
}

fn observed(request: &Request, status: &Status) -> CommandOutput {
    output(
        reply(
            request,
            Outcome::Observed,
            Some(status),
            Value::Null,
            Value::Null,
        ),
        0,
    )
}

fn primed() -> Client {
    let mut client = Client::new();
    let update = client.perform(Operation::Probe, |request| {
        Ok(observed(request, &status(Management::Managed)))
    });
    assert_eq!(update.outcome, Outcome::Observed);
    client
}

#[test]
fn construction_and_local_preview_do_not_start_helpers() {
    let client = Client::new();
    assert!(client.cached_status().is_none());
    assert!(client.helper_reaped());
    assert_eq!(
        client.preview(Action::Clear).unwrap_err().code,
        "cpu_power_probe_required"
    );
    let current = status(Management::Managed);
    let plan = preview(
        &current,
        Action::Set {
            policy: current.current_policy.clone().unwrap(),
        },
    )
    .unwrap();
    assert_eq!(
        plan.action,
        Action::Set {
            policy: policy(55_000_000)
        }
    );
}

#[test]
fn policy_nullable_fields_are_required_and_serialized_as_null() {
    let native = Policy {
        cap_uw: 55_000_000,
        target_celsius: None,
        polling_seconds: None,
        rapl_debounce_seconds: None,
    };
    let value = serde_json::to_value(&native).unwrap();
    for key in ["target_celsius", "polling_seconds", "rapl_debounce_seconds"] {
        assert!(value[key].is_null());
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<Policy>(missing).is_err(),
            "accepted missing {key}"
        );
    }
    assert_eq!(serde_json::from_value::<Policy>(value).unwrap(), native);
}

#[test]
fn every_status_field_is_required_even_when_nullable() {
    let value = serde_json::to_value(status(Management::Native)).unwrap();
    for key in value.as_object().unwrap().keys() {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<Status>(missing).is_err(),
            "accepted missing {key}"
        );
    }
}

#[test]
fn status_observed_null_is_explicit_and_missing_value_is_rejected() {
    assert!(serde_json::from_str::<ObservedLimit>(r#"{"interface":"msr"}"#).is_err());
    assert_eq!(
        serde_json::from_str::<ObservedLimit>(r#"{"interface":"msr","pl1_uw":null}"#)
            .unwrap()
            .pl1_uw,
        None
    );
}

#[test]
fn policy_rejects_boolean_float_negative_and_unknown_fields() {
    let value = serde_json::to_value(policy(55_000_000)).unwrap();
    for invalid in [
        json!(true),
        json!(55_000_000.0),
        json!(-1),
        json!("55000000"),
    ] {
        let mut candidate = value.clone();
        candidate["cap_uw"] = invalid;
        assert!(serde_json::from_value::<Policy>(candidate).is_err());
    }
    let mut candidate = value;
    candidate["root"] = json!("/tmp");
    assert!(serde_json::from_value::<Policy>(candidate).is_err());
}

#[test]
fn explicit_native_options_and_each_qualified_prefix_are_preserved() {
    for options in [
        (None, None, None),
        (Some(90), None, None),
        (Some(90), Some(1), None),
        (Some(90), Some(1), Some(0)),
    ] {
        let candidate = Policy {
            cap_uw: 55_000_000,
            target_celsius: options.0,
            polling_seconds: options.1,
            rapl_debounce_seconds: options.2,
        };
        let plan = preview(
            &status(Management::Managed),
            Action::Set {
                policy: candidate.clone(),
            },
        )
        .unwrap();
        assert_eq!(plan.action, Action::Set { policy: candidate });
    }
}

#[test]
fn whole_watts_and_qualified_operating_interval_are_enforced_locally() {
    for cap in [0, 1, 34_000_000, 55_000_001, 141_000_000, u64::MAX] {
        assert!(
            preview(
                &status(Management::Managed),
                Action::Set {
                    policy: policy(cap)
                }
            )
            .is_err(),
            "accepted {cap}"
        );
    }
    for cap in [35_000_000, 55_000_000, 140_000_000] {
        assert!(preview(
            &status(Management::Managed),
            Action::Set {
                policy: policy(cap)
            }
        )
        .is_ok());
    }
}

#[test]
fn preview_obeys_the_controller_absolute_wire_limit_as_well_as_qualified_range() {
    let mut current = status(Management::Managed);
    current.operational_ceiling_uw = Some(2_147_483_647);
    assert!(preview(
        &current,
        Action::Set {
            policy: policy(2_147_000_000)
        }
    )
    .is_ok());
    for cap in [2_147_483_647, 2_148_000_000, 3_000_000_000] {
        assert!(
            preview(
                &current,
                Action::Set {
                    policy: policy(cap)
                }
            )
            .is_err(),
            "accepted {cap}"
        );
    }
    current.operational_ceiling_uw = Some(3_000_000_000);
    assert!(preview(
        &current,
        Action::Set {
            policy: policy(55_000_000)
        }
    )
    .is_err());
    current.current_policy = Some(policy(3_000_000_000));
    assert!(preview(&current, Action::Clear).is_err());
}

#[test]
fn native_observed_bound_below_set_minimum_is_valid_status_without_write_actions() {
    let mut native = status(Management::Native);
    native.operational_ceiling_uw = Some(30_000_000);
    native.allowed_actions.clear();
    native.observed[0].pl1_uw = Some(30_000_000);
    native.observed[1].pl1_uw = Some(45_000_000);
    validate_status(&native).unwrap();
    assert!(preview(
        &native,
        Action::Set {
            policy: policy(35_000_000)
        }
    )
    .is_err());
    let probe = request(Operation::Probe);
    let update = decode_reply(&probe, None, observed(&probe, &native));
    assert_eq!(update.outcome, Outcome::Observed);
    assert_eq!(update.status, Some(native.clone()));
    for (management, action) in [
        (Management::Managed, Action::Clear),
        (Management::RecoveryPending, Action::Recover),
    ] {
        let previous = status(management);
        let request = request(Operation::Execute {
            plan: preview(&previous, action.clone()).unwrap(),
        });
        let value = reply(
            &request,
            Outcome::AppliedVerified,
            Some(&native),
            receipt(&action),
            Value::Null,
        );
        assert_eq!(
            decode_reply(&request, Some(&previous), output(value, 0)).outcome,
            Outcome::AppliedVerified
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn production_python_controller_wire_passes_actual_rust_decoder() {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Cases {
        schema_version: u32,
        cases: Vec<Case>,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Case {
        case: String,
        request: Request,
        previous_status: Option<Status>,
        stdout: String,
        stderr: String,
        exit_status: i32,
        expected_outcome: Outcome,
    }
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/power_console_wire.py");
    let output = crate::command::run_command(
        "/usr/bin/python3",
        &["-B", fixture.to_str().expect("fixture path is UTF-8")],
        std::time::Duration::from_secs(30),
        512 * 1024,
    )
    .unwrap_or_else(|error| panic!("actual Python wire fixture failed: {error:?}"));
    let cases: Cases = serde_json::from_str(&output.stdout).expect("Python wire fixture JSON");
    assert_eq!(cases.schema_version, 1);
    assert_eq!(cases.cases.len(), 15);
    let mut names = Vec::new();
    for case in cases.cases {
        let update = decode_reply(
            &case.request,
            case.previous_status.as_ref(),
            CommandOutput {
                stdout: case.stdout,
                stderr: case.stderr,
                status: case.exit_status,
            },
        );
        assert_eq!(
            update.outcome, case.expected_outcome,
            "{}: {update:?}",
            case.case
        );
        assert!(update.helper_reaped, "{}", case.case);
        if [
            "clear",
            "native-below-minimum-probe",
            "recover-below-minimum",
        ]
        .contains(&case.case.as_str())
        {
            let status = update.status.as_ref().unwrap();
            assert_eq!(status.management, Management::Native);
            assert_eq!(status.operational_ceiling_uw, Some(30_000_000));
            assert!(status.qualified && status.allowed_actions.is_empty());
            assert!(preview(
                status,
                Action::Set {
                    policy: policy(35_000_000)
                }
            )
            .is_err());
        }
        names.push(case.case);
    }
    names.sort();
    let mut expected = [
        "native-probe",
        "set-native-options",
        "managed-probe",
        "set-fast-options",
        "downward-only-reapply",
        "clear",
        "native-below-minimum-probe",
        "recovery-pending-probe",
        "recover",
        "recover-below-minimum",
        "failed-restored",
        "failed-restored-null-receipt",
        "stale-revision",
        "partial-unqualified-probe",
        "malformed-request-rejection",
    ];
    expected.sort();
    assert_eq!(
        names.iter().map(String::as_str).collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn strict_reply_bounds_live_limits_and_ceiling_but_retains_explicit_unknowns() {
    let request = request(Operation::Probe);
    let mut current = status(Management::Native);
    current.operational_ceiling_uw = Some(2_147_483_647);
    for reading in &mut current.observed {
        reading.pl1_uw = Some(2_147_483_647);
    }
    let valid = reply(
        &request,
        Outcome::Observed,
        Some(&current),
        Value::Null,
        Value::Null,
    );
    assert_eq!(
        decode_reply(&request, None, output(valid.clone(), 0)).outcome,
        Outcome::Observed
    );
    let mut ceiling_overflow = valid.clone();
    ceiling_overflow["status"]["operational_ceiling_uw"] = json!(2_147_483_648u64);
    assert_eq!(
        decode_reply(&request, None, output(ceiling_overflow, 0)).outcome,
        Outcome::FailedStateUnknown
    );
    for index in 0..2 {
        let mut reading_overflow = valid.clone();
        reading_overflow["status"]["observed"][index]["pl1_uw"] = json!(2_147_483_648u64);
        assert_eq!(
            decode_reply(&request, None, output(reading_overflow, 0)).outcome,
            Outcome::FailedStateUnknown
        );
    }
    current.management = Management::Unknown;
    current.qualified = false;
    current.allowed_actions.clear();
    current.revision = None;
    current.operational_ceiling_uw = None;
    for reading in &mut current.observed {
        reading.pl1_uw = None;
    }
    let partial = reply(
        &request,
        Outcome::Observed,
        Some(&current),
        Value::Null,
        failure(),
    );
    let update = decode_reply(&request, None, output(partial, 1));
    assert_eq!(update.outcome, Outcome::Observed);
    assert_eq!(update.status, Some(current));
}

#[test]
fn successful_clear_receipt_cannot_claim_an_out_of_domain_readback() {
    let previous = status(Management::Managed);
    let request = request(Operation::Execute {
        plan: preview(&previous, Action::Clear).unwrap(),
    });
    let mut proof = receipt(&Action::Clear);
    proof["observed"][ZONES[0]]["pl1_uw"] = json!(2_147_483_648u64);
    let value = reply(
        &request,
        Outcome::AppliedVerified,
        Some(&status(Management::Native)),
        proof,
        Value::Null,
    );
    assert_eq!(
        decode_reply(&request, Some(&previous), output(value, 0)).outcome,
        Outcome::FailedStateUnknown
    );
}

#[test]
fn thermal_option_dependencies_and_exact_values_are_enforced() {
    for (target, poll, debounce) in [
        (Some(89), None, None),
        (None, Some(1), None),
        (Some(90), Some(2), None),
        (Some(90), None, Some(0)),
        (Some(90), Some(1), Some(1)),
    ] {
        let candidate = Policy {
            cap_uw: 55_000_000,
            target_celsius: target,
            polling_seconds: poll,
            rapl_debounce_seconds: debounce,
        };
        assert!(preview(
            &status(Management::Managed),
            Action::Set { policy: candidate }
        )
        .is_err());
    }
}

#[test]
fn management_and_allowed_actions_gate_each_action() {
    assert!(preview(&status(Management::Native), Action::Clear).is_err());
    assert!(preview(&status(Management::Managed), Action::Recover).is_err());
    assert!(preview(&status(Management::RecoveryPending), Action::Recover).is_ok());
    assert!(preview(
        &status(Management::RecoveryPending),
        Action::Set {
            policy: policy(55_000_000)
        }
    )
    .is_err());
}

#[test]
fn malformed_status_never_qualifies_a_preview() {
    let base = status(Management::Managed);
    let mut cases = vec![];
    let mut value = base.clone();
    value.revision = Some("A".repeat(64));
    cases.push(value);
    let mut value = base.clone();
    value.minimum_limit_uw = 1;
    cases.push(value);
    let mut value = base.clone();
    value.thermal_spec_reference_uw = 140_000_000;
    cases.push(value);
    let mut value = base.clone();
    value.operational_ceiling_uw = None;
    cases.push(value);
    let mut value = base.clone();
    value.observed[0].pl1_uw = Some(0);
    cases.push(value);
    let mut value = base.clone();
    value.observed[1].interface = Interface::Msr;
    cases.push(value);
    let mut value = base.clone();
    value.allowed_actions.push(ActionKind::Set);
    cases.push(value);
    let mut value = base.clone();
    value.blocking_errors.push("qualification failed".into());
    cases.push(value);
    let mut value = base.clone();
    value.thermald_active = Some(false);
    cases.push(value);
    for value in cases {
        assert!(
            preview(&value, Action::Clear).is_err(),
            "accepted {value:?}"
        );
    }
}

#[test]
fn diagnostics_and_downward_pl1_fluctuation_do_not_invent_an_operating_ceiling() {
    let mut current = status(Management::Managed);
    current.observed[0].pl1_uw = Some(35_000_000);
    current
        .diagnostic_errors
        .push("optional temperature unavailable".into());
    assert!(preview(
        &current,
        Action::Set {
            policy: policy(140_000_000)
        }
    )
    .is_ok());
    assert_eq!(current.operational_ceiling_uw, Some(140_000_000));
    assert_eq!(current.thermal_spec_reference_uw, 55_000_000);
}

#[test]
fn request_payload_has_only_fixed_protocol_fields() {
    let plan = preview(
        &status(Management::Managed),
        Action::Set {
            policy: policy(55_000_000),
        },
    )
    .unwrap();
    let value = serde_json::to_value(request(Operation::Execute { plan })).unwrap();
    assert_eq!(value.as_object().unwrap().len(), 3);
    assert_eq!(
        value["operation"]["plan"]["action"]["policy"]["target_celsius"],
        90
    );
    let text = value.to_string();
    for forbidden in ["raw_register", "root", "sysfs", "executable", "path"] {
        assert!(!text.contains(forbidden));
    }
}

#[test]
fn changed_cached_revision_rejects_without_calling_runner() {
    let mut client = primed();
    let plan = client.preview(Action::Clear).unwrap();
    client.cached.as_mut().unwrap().revision = Some("b".repeat(64));
    let update = client.perform(Operation::Execute { plan }, |_| {
        panic!("stale plan started helper")
    });
    assert_eq!(update.outcome, Outcome::RejectedBeforeWrite);
    assert_eq!(update.error.unwrap().code, "cpu_power_stale_plan");
}

#[test]
fn execute_without_probe_rejects_without_calling_runner() {
    let mut client = Client::new();
    let plan = Plan {
        revision: "a".repeat(64),
        action: Action::Clear,
    };
    let update = client.perform(Operation::Execute { plan }, |_| {
        panic!("unprobed action started helper")
    });
    assert_eq!(update.outcome, Outcome::RejectedBeforeWrite);
}

#[test]
fn valid_probe_preserves_observation_and_partial_probe_blocks_old_cached_plan() {
    let mut client = primed();
    let mut partial = status(Management::Managed);
    partial.qualified = false;
    partial.revision = None;
    partial.holding_now = false;
    partial.allowed_actions.clear();
    partial.blocking_errors.push("read failed".into());
    let update = client.perform(Operation::Probe, |request| {
        Ok(output(
            reply(
                request,
                Outcome::Observed,
                Some(&partial),
                Value::Null,
                failure(),
            ),
            1,
        ))
    });
    assert_eq!(update.outcome, Outcome::Observed);
    assert_eq!(update.status.unwrap(), partial);
    assert!(client.preview(Action::Clear).is_err());
}

#[test]
fn malformed_missing_duplicate_unknown_and_mismatched_replies_are_unknown() {
    let request = request(Operation::Probe);
    let good = reply(
        &request,
        Outcome::Observed,
        Some(&status(Management::Managed)),
        Value::Null,
        Value::Null,
    );
    let mut texts = vec!["".into(), "{unfinished".into()];
    for key in [
        "schema_version",
        "request_id",
        "outcome",
        "status",
        "receipt",
        "error",
    ] {
        let mut value = good.clone();
        value.as_object_mut().unwrap().remove(key);
        texts.push(value.to_string());
    }
    for (key, value) in [
        ("schema_version", json!(2)),
        ("request_id", json!("another")),
        ("foreign", json!(true)),
    ] {
        let mut candidate = good.clone();
        candidate[key] = value;
        texts.push(candidate.to_string());
    }
    texts.push(good.to_string().replacen(
        "\"schema_version\":1",
        "\"schema_version\":1,\"schema_version\":1",
        1,
    ));
    texts.push(good.to_string().replacen(
        "\"cap_uw\":55000000",
        "\"cap_uw\":55000000,\"cap_uw\":55000000",
        1,
    ));
    for text in texts {
        let update = decode_reply(
            &request,
            None,
            CommandOutput {
                stdout: text.clone(),
                stderr: "kept".into(),
                status: 0,
            },
        );
        assert_eq!(
            update.outcome,
            Outcome::FailedStateUnknown,
            "accepted {text}"
        );
        let error = update.error.unwrap();
        assert!(error.details.iter().any(|v| v == "stderr: kept"));
        assert!(error.details.iter().any(|v| v == "helper_exit_status: 0"));
        if !text.is_empty() {
            assert!(error
                .details
                .iter()
                .any(|v| v == &format!("stdout: {text}")));
        }
        assert!(update.helper_reaped);
    }
}

#[test]
fn duplicate_receipt_keys_are_not_silently_overwritten() {
    let request = request(Operation::Execute {
        plan: Plan {
            revision: "a".repeat(64),
            action: Action::Clear,
        },
    });
    let text = reply(
        &request,
        Outcome::AppliedVerified,
        Some(&status(Management::Native)),
        receipt(&Action::Clear),
        Value::Null,
    )
    .to_string()
    .replacen("\"success\":true", "\"success\":false,\"success\":true", 1);
    assert_eq!(
        decode_reply(
            &request,
            None,
            CommandOutput {
                stdout: text,
                stderr: String::new(),
                status: 0
            }
        )
        .outcome,
        Outcome::FailedStateUnknown
    );
}

#[test]
fn actual_exit_status_must_agree_with_terminal_outcome() {
    let request = request(Operation::Probe);
    let value = reply(
        &request,
        Outcome::Observed,
        Some(&status(Management::Managed)),
        Value::Null,
        Value::Null,
    );
    for exit in [1, 126, 127, -1] {
        assert_eq!(
            decode_reply(&request, None, output(value.clone(), exit)).outcome,
            Outcome::FailedStateUnknown
        );
    }
}

#[test]
fn authorization_failure_only_without_terminal_output_is_prewrite() {
    for (exit, code) in [
        (126, "cpu_power_authorization_cancelled"),
        (127, "cpu_power_authorization_unavailable"),
    ] {
        let request = request(Operation::Probe);
        let update = decode_reply(
            &request,
            None,
            CommandOutput {
                stdout: " \n".into(),
                stderr: "denied".into(),
                status: exit,
            },
        );
        assert_eq!(update.outcome, Outcome::RejectedBeforeWrite);
        let error = update.error.unwrap();
        assert_eq!(error.code, code);
        assert!(error.details.iter().any(|v| v == "stderr: denied"));
        assert!(error
            .details
            .iter()
            .any(|v| v == &format!("helper_exit_status: {exit}")));
        let update = decode_reply(
            &request,
            None,
            CommandOutput {
                stdout: "possible writer evidence".into(),
                stderr: String::new(),
                status: exit,
            },
        );
        assert_eq!(update.outcome, Outcome::FailedStateUnknown);
    }
}

#[test]
fn set_success_requires_matching_complete_policy_transaction_and_post_readback() {
    let previous = status(Management::Managed);
    let action = Action::Set {
        policy: policy(55_000_000),
    };
    let request = request(Operation::Execute {
        plan: preview(&previous, action.clone()).unwrap(),
    });
    let value = reply(
        &request,
        Outcome::AppliedVerified,
        Some(&previous),
        receipt(&action),
        Value::Null,
    );
    assert_eq!(
        decode_reply(&request, Some(&previous), output(value.clone(), 0)).outcome,
        Outcome::AppliedVerified
    );
    let mut cases = vec![];
    let mut v = value.clone();
    v["receipt"] = Value::Null;
    cases.push(v);
    let mut v = value.clone();
    v["receipt"]["transaction"] = json!("");
    cases.push(v);
    let mut v = value.clone();
    v["receipt"]["operation"] = json!("clear");
    cases.push(v);
    let mut v = value.clone();
    v["receipt"]["cap_uw"] = json!(54_000_000);
    cases.push(v);
    let mut v = value.clone();
    v["receipt"]["observed"][ZONES[1]]["pl1_uw"] = json!(56_000_000);
    cases.push(v);
    let mut v = value.clone();
    v["status"]["current_policy"]["target_celsius"] = Value::Null;
    v["status"]["current_policy"]["polling_seconds"] = Value::Null;
    v["status"]["current_policy"]["rapl_debounce_seconds"] = Value::Null;
    cases.push(v);
    let mut v = value.clone();
    v["status"]["holding_now"] = json!(false);
    cases.push(v);
    let mut v = value;
    v["error"] = failure();
    cases.push(v);
    for value in cases {
        assert_eq!(
            decode_reply(&request, Some(&previous), output(value, 0)).outcome,
            Outcome::FailedStateUnknown
        );
    }
}

#[test]
fn clear_and_recover_require_native_qualified_post_state() {
    for (management, action) in [
        (Management::Managed, Action::Clear),
        (Management::RecoveryPending, Action::Recover),
    ] {
        let previous = status(management);
        let plan = preview(&previous, action.clone()).unwrap();
        let request = request(Operation::Execute { plan });
        let native = status(Management::Native);
        let valid = reply(
            &request,
            Outcome::AppliedVerified,
            Some(&native),
            receipt(&action),
            Value::Null,
        );
        assert_eq!(
            decode_reply(&request, Some(&previous), output(valid, 0)).outcome,
            Outcome::AppliedVerified
        );
        let invalid = reply(
            &request,
            Outcome::AppliedVerified,
            Some(&previous),
            receipt(&action),
            Value::Null,
        );
        assert_eq!(
            decode_reply(&request, Some(&previous), output(invalid, 0)).outcome,
            Outcome::FailedStateUnknown
        );
    }
}

#[test]
fn reapply_retains_complete_policy_and_validates_actual_apply_receipt() {
    let previous = status(Management::Managed);
    let action = Action::Reapply;
    let request = request(Operation::Execute {
        plan: preview(&previous, action.clone()).unwrap(),
    });
    let valid = reply(
        &request,
        Outcome::AppliedVerified,
        Some(&previous),
        receipt(&action),
        Value::Null,
    );
    assert_eq!(
        decode_reply(&request, Some(&previous), output(valid.clone(), 0)).outcome,
        Outcome::AppliedVerified
    );
    let mut wrong = valid.clone();
    wrong["status"]["current_policy"]["cap_uw"] = json!(56_000_000);
    assert_eq!(
        decode_reply(&request, Some(&previous), output(wrong, 0)).outcome,
        Outcome::FailedStateUnknown
    );
    let mut wrong = valid;
    wrong["receipt"]["writes"] = json!(["/arbitrary/path"]);
    assert_eq!(
        decode_reply(&request, Some(&previous), output(wrong, 0)).outcome,
        Outcome::FailedStateUnknown
    );
}

#[test]
fn failed_restored_requires_previous_complete_policy_even_with_null_receipt() {
    let previous = status(Management::Managed);
    let action = Action::Set {
        policy: policy(54_000_000),
    };
    let request = request(Operation::Execute {
        plan: preview(&previous, action).unwrap(),
    });
    let valid = reply(
        &request,
        Outcome::FailedRestoredVerified,
        Some(&previous),
        Value::Null,
        failure(),
    );
    let update = decode_reply(&request, Some(&previous), output(valid.clone(), 1));
    assert_eq!(update.outcome, Outcome::FailedRestoredVerified);
    assert!(update
        .error
        .unwrap()
        .details
        .iter()
        .any(|v| v == "receipt_error: disk"));
    let mut wrong = valid;
    wrong["status"]["current_policy"]["cap_uw"] = json!(56_000_000);
    assert_eq!(
        decode_reply(&request, Some(&previous), output(wrong, 1)).outcome,
        Outcome::FailedStateUnknown
    );
}

#[test]
fn failed_restored_receipt_must_state_restoration_without_rollback_failure() {
    let previous = status(Management::Managed);
    let action = Action::Clear;
    let request = request(Operation::Execute {
        plan: preview(&previous, action).unwrap(),
    });
    let proof = json!({"version":1,"operation":"clear","success":false,"restoration_verified":true,"rollback_error":null});
    let valid = reply(
        &request,
        Outcome::FailedRestoredVerified,
        Some(&previous),
        proof,
        failure(),
    );
    assert_eq!(
        decode_reply(&request, Some(&previous), output(valid.clone(), 1)).outcome,
        Outcome::FailedRestoredVerified
    );
    for (key, value) in [
        ("restoration_verified", json!(false)),
        ("rollback_error", json!("failed")),
        ("success", json!(true)),
    ] {
        let mut wrong = valid.clone();
        wrong["receipt"][key] = value;
        assert_eq!(
            decode_reply(&request, Some(&previous), output(wrong, 1)).outcome,
            Outcome::FailedStateUnknown
        );
    }
}

#[test]
fn unknown_outcome_blocks_execute_until_new_successful_probe() {
    let mut client = primed();
    let plan = client.preview(Action::Clear).unwrap();
    let update = client.perform(Operation::Execute { plan: plan.clone() }, |_| {
        Ok(CommandOutput {
            stdout: "truncated".into(),
            stderr: String::new(),
            status: 0,
        })
    });
    assert_eq!(update.outcome, Outcome::FailedStateUnknown);
    assert!(client.preview(Action::Clear).is_err());
    let blocked = client.perform(Operation::Execute { plan }, |_| {
        panic!("unknown state started another writer")
    });
    assert_eq!(blocked.outcome, Outcome::RejectedBeforeWrite);
    client.perform(Operation::Probe, |request| {
        Ok(observed(request, &status(Management::Managed)))
    });
    assert!(client.preview(Action::Clear).is_ok());
}

#[test]
fn rejection_with_successful_write_evidence_is_unknown_and_blocks_cached_writes() {
    let mut client = primed();
    let plan = client.preview(Action::Clear).unwrap();
    let update = client.perform(Operation::Execute { plan }, |request| {
        Ok(output(
            reply(
                request,
                Outcome::RejectedBeforeWrite,
                Some(&status(Management::Native)),
                receipt(&Action::Clear),
                failure(),
            ),
            1,
        ))
    });
    assert_eq!(update.outcome, Outcome::FailedStateUnknown);
    assert!(client.preview(Action::Clear).is_err());
    let previous = status(Management::Managed);
    let request = request(Operation::Execute {
        plan: preview(&previous, Action::Reapply).unwrap(),
    });
    let failure_receipt = json!({"version":1,"operation":"apply","success":false,"writes":[],"transaction":null,"error":"read failed"});
    let value = reply(
        &request,
        Outcome::RejectedBeforeWrite,
        Some(&previous),
        failure_receipt,
        failure(),
    );
    assert_eq!(
        decode_reply(&request, Some(&previous), output(value, 1)).outcome,
        Outcome::RejectedBeforeWrite
    );
}

#[test]
fn actual_runner_error_codes_map_exactly_and_preserve_original_details() {
    for (code, expected, reaped, outcome) in [
        (
            "oem_outcome_unknown",
            "cpu_power_outcome_unknown",
            true,
            Outcome::FailedStateUnknown,
        ),
        (
            "oem_helper_liveness_unknown",
            "cpu_power_helper_liveness_unknown",
            false,
            Outcome::FailedStateUnknown,
        ),
        (
            "command_unavailable",
            "cpu_power_helper_unavailable",
            true,
            Outcome::RejectedBeforeWrite,
        ),
        (
            "new_runner_error",
            "cpu_power_runner_unknown",
            false,
            Outcome::FailedStateUnknown,
        ),
    ] {
        let mut error = ControlError::new(code, "retained original message");
        error.details.push("stdout_hex: ff00".into());
        let update = runner_failure(error);
        assert_eq!(update.outcome, outcome);
        assert_eq!(update.helper_reaped, reaped);
        let error = update.error.unwrap();
        assert_eq!(error.code, expected);
        assert_eq!(error.message, "retained original message");
        assert!(error.details.iter().any(|v| v == "stdout_hex: ff00"));
        assert!(error
            .details
            .iter()
            .any(|v| v == &format!("runner_error_code: {code}")));
    }
}

#[test]
fn unknown_lifetime_cannot_be_cleared_by_another_probe_or_request() {
    let mut client = primed();
    let update = client.perform(Operation::Probe, |_| {
        Err(ControlError::new(
            "oem_helper_liveness_unknown",
            "wait failed",
        ))
    });
    assert!(!update.helper_reaped);
    assert!(!client.helper_reaped());
    let update = client.perform(Operation::Probe, |_| {
        panic!("unreaped helper allowed another process")
    });
    assert!(!update.helper_reaped);
    assert_eq!(update.outcome, Outcome::FailedStateUnknown);
}

#[test]
fn unsupported_and_missing_program_are_prewrite_without_losing_lifetime() {
    for code in [
        "cpu_power_unsupported",
        "cpu_power_helper_unavailable",
        "cpu_power_helper_untrusted",
    ] {
        let update = runner_failure(ControlError::new(code, "not started"));
        assert_eq!(update.outcome, Outcome::RejectedBeforeWrite);
        assert!(update.helper_reaped);
    }
}

#[test]
fn no_english_message_can_promote_unknown_to_restored_or_rejected() {
    let update = runner_failure(ControlError::new(
        "unrecognized",
        "rejected before write; restoration verified; all good",
    ));
    assert_eq!(update.outcome, Outcome::FailedStateUnknown);
    assert!(!update.helper_reaped);
}

#[cfg(unix)]
#[test]
fn real_non_utf8_runner_failure_is_reaped_and_keeps_exact_bytes_at_cpu_boundary() {
    let mut client = primed();
    let update = client.perform(Operation::Probe, |_| {
        crate::command::run_control_helper(
            "/bin/sh",
            &["-c", "printf '\\377\\000A'; printf '\\376B' >&2; exit 7"],
            64 * 1024,
        )
    });
    assert_eq!(update.outcome, Outcome::FailedStateUnknown);
    assert!(update.helper_reaped);
    let error = update.error.unwrap();
    assert_eq!(error.code, "cpu_power_outcome_unknown");
    assert!(error.message.contains("exit status 7"));
    assert!(error.details.iter().any(|v| v == "stdout_hex: ff0041"));
    assert!(error.details.iter().any(|v| v == "stderr_hex: fe42"));
    assert!(client.preview(Action::Clear).is_err());
    client.perform(Operation::Probe, |request| {
        Ok(observed(request, &status(Management::Managed)))
    });
    assert!(client.preview(Action::Clear).is_ok());
}

#[test]
fn every_generated_nonce_is_distinct_and_uses_protocol_charset() {
    let first = request_id();
    let second = request_id();
    assert_ne!(first, second);
    for value in [first, second] {
        assert!(!value.is_empty() && value.len() <= 128);
        assert!(value
            .bytes()
            .all(|v| v.is_ascii_alphanumeric() || v == b'-'));
    }
}
