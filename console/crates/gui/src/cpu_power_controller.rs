//! Independent, explicit CPU policy transactions. No discovery or sampling hook.
use super::*;

impl Controller {
    pub(super) fn cpu_foreign_preview_pending(&self) -> bool {
        self.preview.is_some()
            || self.brightness_preview.is_some()
            || self.oem_preview.is_some()
            || self.fan_preview.is_some()
            || self.windows_fn_preview.is_some()
    }

    pub fn cpu_power_preview_pending(&self) -> bool {
        self.cpu_power_preview.is_some()
    }

    pub fn cpu_power_activity(&self) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|job| job.cpu_power_operation)
    }

    /// This does not reject unknown configuration: an explicit probe is the
    /// only way to obtain fresh evidence that unlocks ordinary writes.
    pub fn cpu_power_unavailable_reason(&self) -> Option<&str> {
        if !self.cpu_power_helper_reaped {
            return Some("未确认 CPU 授权辅助进程退出，不能启动新检查或关闭窗口。");
        }
        if self.close_requested {
            return Some("窗口正在等待后台退出。");
        }
        if !self.worker_available {
            return Some("后台线程已停止，请重新打开控制台。");
        }
        if self.fan_session_held || !self.fan_helper_reaped || !self.fan_cleanup_complete {
            return Some("风扇会话尚未恢复并退出，暂不执行 CPU 功耗操作。");
        }
        if self.cpu_foreign_preview_pending() {
            return Some("其他控制的预览待确认，请先完成或取消该预览。");
        }
        if self.busy() {
            return Some("当前后台操作尚未完成。");
        }
        None
    }

    pub fn probe_cpu_power(&mut self) -> Result<(), SubmitError> {
        if self.cpu_power_unavailable_reason().is_some() {
            return Err(SubmitError::Busy);
        }
        self.submit(Job::ProbeCpuPower)?;
        self.invalidate_cpu_power_preview();
        Ok(())
    }

    pub(super) fn invalidate_cpu_power_preview(&mut self) {
        self.cpu_power_preview = None;
        self.cpu_power_preview_revision = None;
        self.cpu_power_revision = self.cpu_power_revision.wrapping_add(1);
    }

    pub fn cancel_cpu_power_preview(&mut self) {
        self.invalidate_cpu_power_preview();
    }

    pub fn set_cpu_power_draft(&mut self, policy: CpuPolicy) {
        if self.cpu_power_draft.as_ref() != Some(&policy) {
            self.cpu_power_draft = Some(policy);
            self.invalidate_cpu_power_preview();
            self.cpu_power_draft_error = None;
        }
    }

    /// All options are explicit. This validates cached evidence without
    /// invoking the backend, authorization, a service command or a device.
    pub fn preview_cpu_power(&mut self, action: CpuAction) -> Result<(), SubmitError> {
        if self.cpu_power_unavailable_reason().is_some() {
            return Err(SubmitError::Busy);
        }
        if self.cpu_power_state_unknown || self.cpu_power_needs_probe {
            self.cpu_power_draft_error = Some(ControlError::new(
                "cpu_power_probe_required",
                "需要先完成一次新的授权检查，再预览 CPU 功耗策略。",
            ));
            return Err(SubmitError::Unsupported);
        }
        self.invalidate_cpu_power_preview();
        let result = self
            .cpu_power_status
            .as_ref()
            .ok_or_else(|| {
                ControlError::new("cpu_power_not_probed", "先显式授权检查 CPU 功耗策略。")
            })
            .and_then(|status| cpu_power::preview(status, action.clone()));
        match result {
            Ok(plan) => {
                if let CpuAction::Set { policy } = &action {
                    self.cpu_power_draft = Some(policy.clone());
                }
                self.cpu_power_preview = Some(plan);
                self.cpu_power_preview_revision = Some(self.cpu_power_revision);
                self.cpu_power_draft_error = None;
                Ok(())
            }
            Err(error) => {
                self.cpu_power_draft_error = Some(error);
                Err(SubmitError::Unsupported)
            }
        }
    }

    pub fn apply_cpu_power_preview(&mut self) -> Result<(), SubmitError> {
        if self.cpu_power_unavailable_reason().is_some() {
            return Err(SubmitError::Busy);
        }
        if self.cpu_power_state_unknown || self.cpu_power_needs_probe {
            return Err(SubmitError::Unsupported);
        }
        let plan = self
            .cpu_power_preview
            .as_ref()
            .filter(|plan| {
                self.cpu_power_preview_revision == Some(self.cpu_power_revision)
                    && match &plan.action {
                        CpuAction::Set { policy } => self.cpu_power_draft.as_ref() == Some(policy),
                        _ => true,
                    }
            })
            .cloned()
            .ok_or(SubmitError::PreviewRequired)?;
        let still_valid = self
            .cpu_power_status
            .as_ref()
            .and_then(|status| cpu_power::preview(status, plan.action.clone()).ok())
            .is_some_and(|current| current == plan);
        if !still_valid {
            self.invalidate_cpu_power_preview();
            return Err(SubmitError::PreviewRequired);
        }
        self.submit(Job::ExecuteCpuPower(plan))?;
        self.invalidate_cpu_power_preview();
        Ok(())
    }

    pub(super) fn accept_cpu_power_update(
        &mut self,
        probe: bool,
        update: CpuPowerUpdate,
        finished_at: Instant,
    ) {
        let confirmed_probe = probe
            && update.helper_reaped
            && update.error.is_none()
            && update.outcome == CpuOutcome::Observed
            && update.status.as_ref().is_some_and(|status| {
                status.qualified
                    && status.management != CpuManagement::Unknown
                    && status.blocking_errors.is_empty()
                    && status.revision.as_ref().is_some_and(|revision| {
                        revision.len() == 64
                            && revision
                                .bytes()
                                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                    })
            });
        if probe {
            self.cpu_power_needs_probe = !confirmed_probe;
        } else if matches!(
            update.outcome,
            CpuOutcome::FailedRestoredVerified | CpuOutcome::FailedStateUnknown
        ) || !update.helper_reaped
        {
            self.cpu_power_needs_probe = true;
        } else if update.outcome == CpuOutcome::AppliedVerified
            && update.error.is_none()
            && update.helper_reaped
        {
            self.cpu_power_needs_probe = false;
        }
        if !update.helper_reaped
            || update.outcome == CpuOutcome::FailedStateUnknown
            || update
                .status
                .as_ref()
                .is_some_and(|status| status.management == CpuManagement::Unknown)
        {
            self.cpu_power_state_unknown = true;
        } else if confirmed_probe {
            self.cpu_power_state_unknown = false;
        }
        self.cpu_power_helper_reaped = update.helper_reaped;
        self.cpu_power_response_finished = Some(finished_at);
        if let Some(status) = update.status {
            // A late observation must not overwrite a draft edited while its
            // authorization dialog or worker task was running.
            if self.cpu_power_draft.is_none() {
                self.cpu_power_draft = status.current_policy.clone();
            }
            self.cpu_power_status = Some(status);
            self.cpu_power_received_at = Some(std::time::SystemTime::now());
        }
        self.cpu_power_outcome = Some(update.outcome);
        self.cpu_power_error = update.error;
        // Keep earlier bounded recovery evidence if this operation has no
        // receipt (for example, authorization was cancelled before execution).
        if let Some(receipt) = update.receipt {
            self.cpu_power_receipt = Some(receipt);
        }
    }
}
