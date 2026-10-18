//! One authenticated request per elevated helper. Invalid input cannot reach EC.
use super::{
    protocol::{self, Operation, Response},
    HelperResult, Outcome,
};
use crate::ControlError;
use std::ffi::OsString;

#[derive(Debug, PartialEq, Eq)]
struct ServeArgs {
    nonce: String,
    parent_pid: u32,
    parent_creation_time: u64,
}

fn invalid_args() -> ControlError {
    ControlError::new(
        "windows_fnlock_helper_arguments",
        "Expected exactly --serve NONCE PARENT_PID PARENT_CREATION_TIME",
    )
}

fn parse_args(args: &[OsString]) -> Result<ServeArgs, ControlError> {
    if args.len() != 4 || args[0] != "--serve" {
        return Err(invalid_args());
    }
    let nonce = args[1].to_str().ok_or_else(invalid_args)?;
    if nonce.len() != 64
        || !nonce
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
    {
        return Err(invalid_args());
    }
    let pid = args[2].to_str().ok_or_else(invalid_args)?;
    let parent_pid: u32 = pid.parse().map_err(|_| invalid_args())?;
    let created = args[3].to_str().ok_or_else(invalid_args)?;
    let parent_creation_time: u64 = created.parse().map_err(|_| invalid_args())?;
    if parent_pid == 0
        || parent_creation_time == 0
        || parent_pid.to_string() != pid
        || parent_creation_time.to_string() != created
    {
        return Err(invalid_args());
    }
    Ok(ServeArgs {
        nonce: nonce.into(),
        parent_pid,
        parent_creation_time,
    })
}

#[cfg(any(target_os = "windows", test))]
fn handle_request_with(
    payload: &[u8],
    execute: impl FnOnce(&Operation) -> HelperResult,
) -> Result<(Vec<u8>, u32), ControlError> {
    let request = protocol::decode_request(payload)?;
    let result = execute(&request.operation);
    let response = Response::from_result(request.request_id, result);
    let exit = if response.error.is_none()
        && matches!(
            response.outcome,
            Outcome::Observed | Outcome::AppliedVerified
        ) {
        0
    } else {
        2
    };
    // Encoding validates the semantic response. A failure after execution must
    // escape as a send failure, never initiate a second request or recovery.
    Ok((protocol::encode_response(&response)?, exit))
}

/// The GUI and CLI never invoke this entry point in their own process.
#[cfg(target_os = "windows")]
pub fn run(args: &[OsString]) -> u32 {
    use std::io::Write;
    let args = match parse_args(args) {
        Ok(args) => args,
        Err(error) => {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}",
                serde_json::json!({"error":error})
            );
            return 2;
        }
    };
    let result = super::native::ipc::serve_authenticated(
        &args.nonce,
        args.parent_pid,
        args.parent_creation_time,
        |payload| {
            handle_request_with(payload, |operation| match operation {
                Operation::Probe => super::native::probe(),
                Operation::Apply { plan } => super::native::apply(plan),
            })
        },
    );
    match result {
        Ok(exit) => exit,
        Err(error) => {
            // No phase claim: failure may have happened after a hardware write.
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}",
                serde_json::json!({"error":error})
            );
            3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Receipt, Recovery};
    use super::*;
    use std::cell::Cell;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }
    #[test]
    fn serve_arguments_are_canonical_and_cannot_choose_a_path_or_operation() {
        let nonce = "a1".repeat(32);
        assert_eq!(
            parse_args(&args(&["--serve", &nonce, "42", "123"]))
                .unwrap()
                .parent_pid,
            42
        );
        for values in [
            vec![],
            vec!["--apply-plan", "{}"],
            vec!["--serve", &nonce, "0", "123"],
            vec!["--serve", &nonce, "042", "123"],
            vec!["--serve", &nonce, "4294967296", "123"],
            vec!["--serve", &nonce, "42", "+123"],
            vec!["--serve", &nonce, "42", "0"],
            vec!["--serve", "../escape", "42", "123"],
            vec!["--serve", &nonce, "42", "123", "--path"],
        ] {
            assert!(parse_args(&args(&values)).is_err(), "{values:?}");
        }
        assert!(parse_args(&args(&["--serve", &"AA".repeat(32), "42", "123"])).is_err());
    }
    fn rejected() -> HelperResult {
        let error = ControlError::new(
            "windows_fnlock_contract_disabled",
            "Candidate requires an explicit qualification build",
        );
        HelperResult {
            status: None,
            outcome: Outcome::RejectedBeforeWrite,
            result: None,
            receipt: Receipt {
                primary_error: Some(error.clone()),
                ..Default::default()
            },
            error: Some(error),
        }
    }
    #[test]
    fn malformed_or_unknown_request_never_reaches_execution() {
        for bytes in [b"{}".as_slice(), b"{\"schema_version\":1,\"request_id\":\"r1\",\"operation\":{\"kind\":\"shell\"}}", b"{\"schema_version\":1,\"schema_version\":1,\"request_id\":\"r1\",\"operation\":{\"kind\":\"probe\"}}"] {
            let calls = Cell::new(0);
            assert!(handle_request_with(bytes, |_| { calls.set(calls.get()+1); rejected() }).is_err());
            assert_eq!(calls.get(), 0);
        }
    }
    #[test]
    fn one_valid_request_retains_typed_rejection_and_exit() {
        let request = protocol::Request {
            schema_version: 1,
            request_id: "r1".into(),
            operation: Operation::Probe,
        };
        let calls = Cell::new(0);
        let (bytes, exit) =
            handle_request_with(&protocol::encode_request(&request).unwrap(), |operation| {
                assert!(matches!(operation, Operation::Probe));
                calls.set(calls.get() + 1);
                rejected()
            })
            .unwrap();
        assert_eq!(calls.get(), 1);
        assert_eq!(exit, 2);
        let response = protocol::decode_response(&bytes).unwrap();
        protocol::validate_exchange(&request, &response, exit).unwrap();
        assert_eq!(
            response.error.unwrap().code,
            "windows_fnlock_contract_disabled"
        );
    }
    #[test]
    fn invalid_post_execution_result_is_not_retried_or_reported_as_no_write() {
        let request = protocol::Request {
            schema_version: 1,
            request_id: "r2".into(),
            operation: Operation::Probe,
        };
        let calls = Cell::new(0);
        let result = handle_request_with(&protocol::encode_request(&request).unwrap(), |_| {
            calls.set(calls.get() + 1);
            HelperResult {
                status: None,
                outcome: Outcome::AppliedVerified,
                result: None,
                receipt: Receipt {
                    write_attempted: true,
                    recovery: Recovery::NotNeeded,
                    ..Default::default()
                },
                error: None,
            }
        });
        assert!(result.is_err());
        assert_eq!(calls.get(), 1);
    }
}
