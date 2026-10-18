//! Fixed entry point for the installed fan broker. Standard output belongs to
//! its protocol; this executable accepts no device, backend or environment option.
use mechrevo_console_core::{fan_curve::broker, ControlError};
use serde_json::{json, Value};
use std::ffi::{OsStr, OsString};
use std::io::Write;

const MAX_CODE_CHARS: usize = 128;
const MAX_MESSAGE_CHARS: usize = 512;
const MAX_DETAILS: usize = 4;
const MAX_DETAIL_CHARS: usize = 128;

fn parse_args(mut args: impl Iterator<Item = OsString>) -> Result<(), ControlError> {
    if args.next().as_deref() == Some(OsStr::new("--serve")) && args.next().is_none() {
        Ok(())
    } else {
        Err(ControlError::new(
            "invalid_arguments",
            "Expected exactly --serve",
        ))
    }
}

fn bounded_text(text: &str, limit: usize) -> (String, bool) {
    let mut chars = text.chars();
    let prefix = chars.by_ref().take(limit).collect();
    (prefix, chars.next().is_some())
}

/// At most 1,152 source characters are serialized. JSON escaping needs at most
/// six bytes per character, so this remains below 8 KiB including its newline.
/// Never echo rejected arguments or allow an unbounded broker error on stderr.
fn error_json(error: &ControlError, phase: &'static str) -> Value {
    let (code, code_truncated) = bounded_text(&error.code, MAX_CODE_CHARS);
    let (message, message_truncated) = bounded_text(&error.message, MAX_MESSAGE_CHARS);
    let mut truncated = code_truncated || message_truncated || error.details.len() > MAX_DETAILS;
    let details: Vec<_> = error
        .details
        .iter()
        .take(MAX_DETAILS)
        .map(|detail| {
            let (text, was_truncated) = bounded_text(detail, MAX_DETAIL_CHARS);
            truncated |= was_truncated;
            text
        })
        .collect();
    json!({
        "error": {"code": code, "message": message, "details": details, "truncated": truncated},
        "phase": phase,
    })
}

// @mark mechrevo.fan_curve role=entrypoint contract=fixed_privileged_helper
fn main() {
    let (result, phase) = match parse_args(std::env::args_os().skip(1)) {
        Ok(()) => (broker::serve_stdio(), "serve_failed"),
        Err(error) => (Err(error), "rejected_before_serve"),
    };
    if let Err(error) = result {
        let _ = writeln!(std::io::stderr().lock(), "{}", error_json(&error, phase));
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<(), ControlError> {
        parse_args(args.iter().map(OsString::from))
    }

    #[test]
    fn accepts_only_the_single_fixed_serve_argument() {
        assert!(parse(&["--serve"]).is_ok());
        for args in [
            vec![],
            vec![""],
            vec!["--help"],
            vec!["--version"],
            vec!["serve"],
            vec!["--serve=true"],
            vec!["--serve "],
            vec!["--Serve"],
            vec!["--"],
            vec!["--serve", "--serve"],
            vec!["--serve", ""],
            vec!["--serve", "--backend", "fake"],
            vec!["--serve", "--device", "/dev/tuxedo_io"],
            vec!["--serve", "--root", "/tmp/fixture"],
            vec!["--backend", "fake", "--serve"],
        ] {
            let error = parse(&args).unwrap_err();
            assert_eq!(error.code, "invalid_arguments", "{args:?}");
            assert_eq!(error.message, "Expected exactly --serve");
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_unicode_arguments_without_lossy_conversion() {
        use std::os::unix::ffi::OsStringExt;

        assert!(parse_args([OsString::from_vec(vec![0xff])].into_iter()).is_err());
        assert!(parse_args(
            [OsString::from("--serve"), OsString::from_vec(vec![0xff])].into_iter()
        )
        .is_err());
    }

    #[test]
    fn errors_remain_single_line_bounded_json_with_explicit_truncation() {
        let mut error = ControlError::new(&"\0".repeat(4096), "\n".repeat(4096));
        error.details = vec!["\t".repeat(4096); 1024];
        let line = format!("{}\n", error_json(&error, "serve_failed"));
        assert!(line.len() < 8192, "{} bytes", line.len());
        assert_eq!(line.bytes().filter(|byte| *byte == b'\n').count(), 1);
        let decoded: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(decoded["error"]["truncated"], true);
        assert_eq!(decoded["error"]["details"].as_array().unwrap().len(), 4);
        assert_eq!(decoded["phase"], "serve_failed");
    }

    #[test]
    fn short_errors_preserve_unicode_and_report_no_truncation() {
        let mut error = ControlError::new("unsupported", "此平台不支持风扇代理");
        error.details.push("No hardware opened".into());
        let value = error_json(&error, "rejected_before_serve");
        assert_eq!(value["error"]["code"], error.code);
        assert_eq!(value["error"]["message"], error.message);
        assert_eq!(value["error"]["details"], json!(error.details));
        assert_eq!(value["error"]["truncated"], false);
    }

    #[test]
    fn truncation_respects_unicode_character_boundaries_and_exact_limits() {
        assert_eq!(bounded_text("风扇曲线", 2), ("风扇".into(), true));
        assert_eq!(bounded_text("风扇", 2), ("风扇".into(), false));
        let mut error = ControlError::new("x".repeat(MAX_CODE_CHARS).as_str(), "ok");
        error.details = vec!["x".repeat(MAX_DETAIL_CHARS); MAX_DETAILS];
        assert_eq!(
            error_json(&error, "serve_failed")["error"]["truncated"],
            false
        );
        error.details.push(String::new());
        assert_eq!(
            error_json(&error, "serve_failed")["error"]["truncated"],
            true
        );
    }
}
