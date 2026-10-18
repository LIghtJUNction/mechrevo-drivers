//! Explicit Windows Fn-lock transactions. Discovery never authorizes a helper.
use super::*;

pub(super) fn uncertain_update(error: ControlError) -> WindowsFnUpdate {
    WindowsFnUpdate {
        status: None,
        outcome: WindowsFnOutcome::FailedStateUnknown,
        result: None,
        receipt: WindowsFnReceipt {
            writer_may_continue: true,
            recovery: WindowsFnRecovery::Unknown,
            primary_error: Some(error.clone()),
            ..Default::default()
        },
        error: Some(error),
        helper_reaped: false,
        helper_lifetime: WindowsFnLifetime::Unknown,
    }
}

impl Controller {
    pub fn windows_fn_visible(&self) -> bool {
        cfg!(target_os = "windows")
            || self
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.platform.eq_ignore_ascii_case("windows"))
            || self.windows_fn_status.is_some()
            || self.windows_fn_outcome.is_some()
    }

    pub fn windows_fn_lifetime_clean(&self) -> bool {
        self.windows_fn_helper_reaped
            && matches!(
                self.windows_fn_helper_lifetime,
                WindowsFnLifetime::NotStarted | WindowsFnLifetime::Reaped
            )
    }

    pub fn windows_fn_activity(&self) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|job| job.windows_fn_operation.is_some())
    }

    fn windows_fn_foreign_preview(&self) -> bool {
        self.preview.is_some()
            || self.brightness_preview.is_some()
            || self.oem_preview.is_some()
            || self.fan_preview.is_some()
            || self.cpu_power_preview.is_some()
    }

    /// State uncertainty permits an explicit new probe; lifetime uncertainty
    /// permits only waiting for the already-started helper.
    pub fn windows_fn_unavailable_reason(&self) -> Option<&str> {
        if !self.windows_fn_lifetime_clean() {
            return Some("Windows 授权辅助进程尚未确认退出；不能启动新检查、写入或关闭窗口。");
        }
        if self.close_requested {
            return Some("窗口正在等待后台退出。");
        }
        if !self.worker_available {
            return Some("后台已停止，请重新打开控制台。");
        }
        if self.fan_session_held || !self.fan_helper_reaped || !self.fan_cleanup_complete {
            return Some("风扇会话尚未恢复并退出，暂不能检查或调节 Windows Fn 锁。");
        }
        if self.cpu_power_state_unknown || !self.cpu_power_helper_reaped {
            return Some("CPU 功耗事务尚未确认，请先检查其状态。");
        }
        if self.windows_fn_foreign_preview() {
            return Some("其他控制的预览待确认，请先完成或取消该预览。");
        }
        if self.busy() {
            return Some("当前后台操作尚未完成。");
        }
        None
    }

    pub(super) fn invalidate_windows_fn_preview(&mut self) {
        self.windows_fn_preview = None;
        self.windows_fn_preview_revision = None;
        self.windows_fn_revision = self.windows_fn_revision.wrapping_add(1);
    }

    pub fn cancel_windows_fn_preview(&mut self) {
        self.invalidate_windows_fn_preview();
    }

    pub fn set_windows_fn_target(&mut self, target: bool) {
        if self.windows_fn_requested != Some(target) {
            self.windows_fn_requested = Some(target);
            self.invalidate_windows_fn_preview();
            self.windows_fn_draft_error = None;
        }
    }

    pub fn probe_windows_fn(&mut self) -> Result<(), SubmitError> {
        if self.windows_fn_unavailable_reason().is_some() {
            return Err(SubmitError::Busy);
        }
        self.submit(Job::ProbeWindowsFn)?;
        self.invalidate_windows_fn_preview();
        Ok(())
    }

    /// This consumes cached authorized status only; no Backend method is used.
    pub fn preview_windows_fn(&mut self) -> Result<(), SubmitError> {
        if self.windows_fn_unavailable_reason().is_some() {
            return Err(SubmitError::Busy);
        }
        if self.windows_fn_state_unknown || self.windows_fn_needs_probe {
            self.windows_fn_draft_error = Some(ControlError::new(
                "windows_fn_probe_required",
                "请先完成一次新的 Windows Fn 锁授权检查，再预览更改。",
            ));
            return Err(SubmitError::Unsupported);
        }
        self.invalidate_windows_fn_preview();
        let plan = self
            .windows_fn_status
            .as_ref()
            .zip(self.windows_fn_requested)
            .ok_or_else(|| {
                ControlError::new(
                    "windows_fn_not_probed",
                    "尚未获得可用的 Windows Fn 锁检查结果。",
                )
            })
            .and_then(|(status, target)| windows_oem::preview(status, target));
        match plan {
            Ok(plan) => {
                self.windows_fn_preview = Some(plan);
                self.windows_fn_preview_revision = Some(self.windows_fn_revision);
                self.windows_fn_draft_error = None;
                Ok(())
            }
            Err(error) => {
                self.windows_fn_draft_error = Some(error);
                Err(SubmitError::Unsupported)
            }
        }
    }

    pub fn apply_windows_fn_preview(&mut self) -> Result<(), SubmitError> {
        if self.windows_fn_unavailable_reason().is_some() {
            return Err(SubmitError::Busy);
        }
        if self.windows_fn_state_unknown || self.windows_fn_needs_probe {
            return Err(SubmitError::Unsupported);
        }
        let plan = self
            .windows_fn_preview
            .as_ref()
            .filter(|_| self.windows_fn_preview_revision == Some(self.windows_fn_revision))
            .cloned()
            .ok_or(SubmitError::PreviewRequired)?;
        let valid = self
            .windows_fn_status
            .as_ref()
            .zip(self.windows_fn_requested)
            .and_then(|(status, target)| windows_oem::preview(status, target).ok())
            .is_some_and(|current| current == plan);
        if !valid {
            self.invalidate_windows_fn_preview();
            return Err(SubmitError::PreviewRequired);
        }
        self.submit(Job::ApplyWindowsFn(plan))?;
        self.invalidate_windows_fn_preview();
        Ok(())
    }

    /// No authorization or new process: wait/drain the retained child only.
    pub fn finish_windows_fn_pending(&mut self) -> Result<(), SubmitError> {
        if self.windows_fn_lifetime_clean() {
            return Err(SubmitError::Unsupported);
        }
        if self.in_flight.is_some()
            || self.oem_write_uncertain
            || !self.cpu_power_helper_reaped
            || !self.worker_available
            || self.fan_session_held
            || !self.fan_helper_reaped
            || !self.fan_cleanup_complete
        {
            return Err(SubmitError::Busy);
        }
        self.enqueue(Job::FinishWindowsFn)
    }

    pub(super) fn accept_windows_fn_update(
        &mut self,
        operation: WindowsFnOperation,
        update: WindowsFnUpdate,
        finished_at: Instant,
    ) {
        let clean = update.helper_reaped
            && matches!(
                update.helper_lifetime,
                WindowsFnLifetime::NotStarted | WindowsFnLifetime::Reaped
            );
        let was_clean = self.windows_fn_lifetime_clean();
        // A new probe can establish device state, never erase an unknown child.
        if was_clean
            || (operation == WindowsFnOperation::FinishPending
                && update.helper_reaped
                && update.helper_lifetime == WindowsFnLifetime::Reaped)
        {
            self.windows_fn_helper_reaped = update.helper_reaped;
            self.windows_fn_helper_lifetime = update.helper_lifetime;
        }
        let confirmed_probe = operation == WindowsFnOperation::Probe
            && was_clean
            && clean
            && update.outcome == WindowsFnOutcome::Observed
            && update.error.is_none()
            && update.status.as_ref().is_some_and(|status| {
                status.qualified
                    && status.blocking_errors.is_empty()
                    && status
                        .control
                        .as_ref()
                        .and_then(|control| match &control.current {
                            Some(OemValue::Boolean(value)) => Some(*value),
                            _ => None,
                        })
                        .is_some_and(|current| windows_oem::preview(status, current).is_ok())
            });
        let incomplete_success = operation == WindowsFnOperation::Apply
            && update.outcome == WindowsFnOutcome::AppliedVerified
            && (update.error.is_some()
                || update.result.is_none()
                || update.receipt.full_byte_verified != Some(true)
                || update.receipt.protected_bits_preserved != Some(true));
        if !clean
            || update.outcome == WindowsFnOutcome::FailedStateUnknown
            || update.receipt.blocks_further_writes()
            || incomplete_success
        {
            self.windows_fn_state_unknown = true;
            self.windows_fn_needs_probe = true;
        } else if confirmed_probe {
            self.windows_fn_state_unknown = false;
            self.windows_fn_needs_probe = false;
            self.windows_fn_draft_error = None;
        } else if operation == WindowsFnOperation::Probe
            || update.outcome == WindowsFnOutcome::FailedRestoredVerified
        {
            self.windows_fn_needs_probe = true;
        }
        if let Some(status) = update.status {
            if self.windows_fn_requested.is_none() {
                self.windows_fn_requested =
                    status
                        .control
                        .as_ref()
                        .and_then(|control| match &control.current {
                            Some(OemValue::Boolean(value)) => Some(*value),
                            _ => None,
                        });
            }
            self.windows_fn_status = Some(status);
            self.windows_fn_received_at = Some(std::time::SystemTime::now());
        }
        let receipt = update.receipt;
        let has_receipt = receipt.write_attempted
            || receipt.writer_may_continue
            || receipt.primary_error.is_some()
            || receipt.restore_error.is_some()
            || receipt.identity_error.is_some()
            || !receipt.readback_errors.is_empty()
            || receipt.recovery != WindowsFnRecovery::NotNeeded;
        if operation == WindowsFnOperation::Apply || receipt.write_attempted {
            self.windows_fn_receipt = Some(receipt);
            self.windows_fn_followup_receipt = None;
        } else if has_receipt {
            self.windows_fn_followup_receipt = Some(receipt);
        }
        if operation == WindowsFnOperation::Apply || update.result.is_some() {
            self.windows_fn_result = update.result;
        }
        self.windows_fn_outcome = Some(update.outcome);
        self.windows_fn_error = update.error;
        self.windows_fn_response_finished = Some(finished_at);
    }
}
