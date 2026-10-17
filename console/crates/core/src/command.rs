//! Bounded subprocess execution for platform helpers.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub status: i32,
}

enum PipeEvent {
    Data(bool, Vec<u8>),
    Closed,
    Failed(String),
}

fn read_pipe(mut pipe: impl Read + Send + 'static, stdout: bool, sender: SyncSender<PipeEvent>) {
    std::thread::spawn(move || {
        let mut buffer = [0; 8192];
        loop {
            match pipe.read(&mut buffer) {
                Ok(0) => {
                    let _ = sender.send(PipeEvent::Closed);
                    break;
                }
                Ok(length) => {
                    if sender
                        .send(PipeEvent::Data(stdout, buffer[..length].to_vec()))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = sender.send(PipeEvent::Failed(error.to_string()));
                    break;
                }
            }
        }
    });
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        // process_group(0) gives this helper its own group. Killing the group also
        // closes pipes inherited by grandchildren after the group leader exits.
        unsafe extern "C" {
            fn kill(pid: i32, signal: i32) -> i32;
        }
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: kill takes only integer values; a negative PID addresses
            // the process group created for this child, and 9 is SIGKILL.
            unsafe {
                kill(-pid, 9);
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn failure(code: &str, message: String, stdout: &[u8], stderr: &[u8]) -> crate::ControlError {
    let mut error = crate::ControlError::new(code, message);
    if !stdout.is_empty() {
        error
            .details
            .push(format!("stdout: {}", String::from_utf8_lossy(stdout)));
    }
    if !stderr.is_empty() {
        error
            .details
            .push(format!("stderr: {}", String::from_utf8_lossy(stderr)));
    }
    error
}

/// Execute a program directly, bounding wall time and the combined raw output.
///
/// Callers provide arguments separately: no shell is invoked or interpolated.
/// A nonzero exit is an error whose details retain the bounded helper output.
pub fn run_command(
    program: &str,
    args: &[&str],
    timeout: Duration,
    output_limit: usize,
) -> Result<CommandOutput, crate::ControlError> {
    let started = Instant::now();
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = command.spawn().map_err(|error| {
        crate::ControlError::new(
            "command_unavailable",
            format!("Cannot start {program}: {error}"),
        )
    })?;
    // A small bounded channel prevents unbounded allocations even when a helper
    // continuously fills both pipes. The receiver enforces the combined limit.
    let (sender, receiver) = mpsc::sync_channel(4);
    read_pipe(
        child.stdout.take().expect("stdout was piped"),
        true,
        sender.clone(),
    );
    read_pipe(
        child.stderr.take().expect("stderr was piped"),
        false,
        sender,
    );
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut closed = 0;
    let mut exit_status = None;
    loop {
        if exit_status.is_none() {
            match child.try_wait() {
                Ok(status) => exit_status = status,
                Err(error) => {
                    terminate(&mut child);
                    return Err(failure(
                        "command_failed",
                        format!("Cannot wait for {program}: {error}"),
                        &stdout,
                        &stderr,
                    ));
                }
            }
        }
        if let Some(status) = exit_status {
            if closed == 2 {
                let code = status.code().unwrap_or(-1);
                if !status.success() {
                    return Err(failure(
                        "command_failed",
                        format!("{program} exited with status {code}"),
                        &stdout,
                        &stderr,
                    ));
                }
                return Ok(CommandOutput {
                    stdout: String::from_utf8_lossy(&stdout).into_owned(),
                    stderr: String::from_utf8_lossy(&stderr).into_owned(),
                    status: code,
                });
            }
        }
        if started.elapsed() >= timeout {
            terminate(&mut child);
            return Err(failure(
                "command_timeout",
                format!("{program} exceeded its {} ms timeout", timeout.as_millis()),
                &stdout,
                &stderr,
            ));
        }
        let wait = timeout
            .saturating_sub(started.elapsed())
            .min(Duration::from_millis(10));
        match receiver.recv_timeout(wait) {
            Ok(PipeEvent::Data(is_stdout, bytes)) => {
                let available = output_limit.saturating_sub(stdout.len() + stderr.len());
                let destination = if is_stdout { &mut stdout } else { &mut stderr };
                destination.extend_from_slice(&bytes[..bytes.len().min(available)]);
                if bytes.len() > available {
                    terminate(&mut child);
                    return Err(failure(
                        "command_output_limit",
                        format!("{program} exceeded its {output_limit} byte output limit"),
                        &stdout,
                        &stderr,
                    ));
                }
            }
            Ok(PipeEvent::Closed) => closed += 1,
            Ok(PipeEvent::Failed(error)) => {
                terminate(&mut child);
                return Err(failure(
                    "command_failed",
                    format!("Cannot read output from {program}: {error}"),
                    &stdout,
                    &stderr,
                ));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Both reader threads have finished; an exit status can arrive
                // on the next iteration if pipe closure preceded process exit.
                closed = 2;
                std::thread::sleep(wait);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn captures_both_streams_and_exact_boundary() {
        let output = run_command(
            "/bin/sh",
            &["-c", "printf 1234; printf 5678 >&2"],
            Duration::from_secs(2),
            8,
        )
        .unwrap();
        assert_eq!(output.stdout, "1234");
        assert_eq!(output.stderr, "5678");
        assert_eq!(output.status, 0);
    }

    #[test]
    fn arguments_are_literal() {
        let output = run_command(
            "/usr/bin/printf",
            &["%s", "a; $(printf injected)"],
            Duration::from_secs(2),
            128,
        )
        .unwrap();
        assert_eq!(output.stdout, "a; $(printf injected)");
    }

    #[test]
    fn missing_program_has_distinct_error() {
        let error =
            run_command("/no/such/mechrevo-helper", &[], Duration::from_secs(1), 128).unwrap_err();
        assert_eq!(error.code, "command_unavailable");
    }

    #[test]
    fn nonzero_retains_stderr() {
        let error = run_command(
            "/bin/sh",
            &["-c", "printf denied >&2; exit 7"],
            Duration::from_secs(2),
            128,
        )
        .unwrap_err();
        assert_eq!(error.code, "command_failed");
        assert!(error
            .details
            .iter()
            .any(|detail| detail == "stderr: denied"));
    }

    #[test]
    fn output_limit_is_shared_across_streams() {
        let error = run_command(
            "/bin/sh",
            &["-c", "printf 1234; printf 56789 >&2"],
            Duration::from_secs(2),
            8,
        )
        .unwrap_err();
        assert_eq!(error.code, "command_output_limit");
        let captured: usize = error
            .details
            .iter()
            .map(|detail| detail.split_once(": ").unwrap().1.len())
            .sum();
        assert_eq!(captured, 8);
    }

    #[test]
    fn large_stderr_does_not_block_on_stdout() {
        let started = Instant::now();
        let error = run_command(
            "/bin/sh",
            &["-c", "while :; do printf 1234567890 >&2; done"],
            Duration::from_secs(3),
            64,
        )
        .unwrap_err();
        assert_eq!(error.code, "command_output_limit");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn zero_limit_allows_empty_output() {
        let output = run_command("/bin/sh", &["-c", "exit 0"], Duration::from_secs(2), 0).unwrap();
        assert_eq!(output.stdout, "");
        let error =
            run_command("/bin/sh", &["-c", "printf x"], Duration::from_secs(2), 0).unwrap_err();
        assert_eq!(error.code, "command_output_limit");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn timeout_kills_descendant_after_leader_has_exited() {
        let started = Instant::now();
        let error = run_command(
            "/bin/sh",
            &["-c", "sleep 60 & printf '%s' \"$!\"; exit 0"],
            Duration::from_millis(150),
            128,
        )
        .unwrap_err();
        assert_eq!(error.code, "command_timeout");
        assert!(started.elapsed() < Duration::from_secs(2));
        let pid: u32 = error
            .details
            .iter()
            .find_map(|detail| detail.strip_prefix("stdout: "))
            .unwrap()
            .parse()
            .unwrap();
        // Reparented children can briefly remain zombies until PID 1 reaps them.
        // A zombie cannot run or keep either output pipe open.
        for _ in 0..100 {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"));
            if stat.as_ref().is_err() || stat.unwrap().split_once(") ").unwrap().1.starts_with('Z')
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("helper descendant {pid} survived timeout");
    }
}
