//! Windows-only Fn control within the existing Controls surface.
use super::*;
use mechrevo_console_core::windows_oem::{Outcome, Recovery, SupportStage};

impl DesktopApp {
    pub(super) fn windows_fn_controls(&mut self, ui: &mut egui::Ui) {
        let status = self.controller.windows_fn_status.clone();
        let unavailable = self
            .controller
            .windows_fn_unavailable_reason()
            .map(str::to_owned);
        surface(ui, |ui| {
            ui.label(RichText::new("Windows Fn 锁").size(18.0));
            fn_note(ui, "检查与应用分别请求授权，普通刷新不会启动授权辅助进程。");
            let probe_clicked = ui
                .add_enabled(
                    unavailable.is_none(),
                    egui::Button::new("检查 Windows Fn 锁（需授权）"),
                )
                .clicked();
            if let Some(Err(error)) = probe_clicked.then(|| self.controller.probe_windows_fn()) {
                self.windows_fn_submit_error(error);
            }
            if self.controller.windows_fn_activity() {
                fn_note(ui, "正在等待授权操作及辅助进程实际退出。");
            }
            if !self.controller.windows_fn_lifetime_clean() {
                ui.add(egui::Label::new(RichText::new("尚未确认辅助进程退出。不能再次授权、写入或关闭窗口；继续等待只处理已有进程。").size(13.0).color(amber(ui))).wrap());
                let wait_clicked = ui
                    .add_enabled(
                        !self.controller.windows_fn_activity(),
                        egui::Button::new("继续等待辅助进程"),
                    )
                    .clicked();
                if let Some(Err(error)) =
                    wait_clicked.then(|| self.controller.finish_windows_fn_pending())
                {
                    self.windows_fn_submit_error(error);
                }
            }
            if self.controller.windows_fn_state_unknown
                || self.controller.windows_fn_needs_probe && status.is_some()
            {
                ui.add(
                    egui::Label::new(
                        RichText::new("请求状态未确认；重新授权检查成功前不能预览或应用更改。")
                            .size(13.0)
                            .color(amber(ui)),
                    )
                    .wrap(),
                );
            }
            if let Some(reason) = &unavailable {
                fn_note(ui, reason);
            }
            if let Some(error) = &self.controller.windows_fn_error {
                fn_error(ui, "上次操作", error);
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(12.0);
            let current = status
                .as_ref()
                .and_then(|status| status.control.as_ref())
                .and_then(|control| match &control.current {
                    Some(OemValue::Boolean(value)) => Some(*value),
                    _ => None,
                });
            ui.label(
                RichText::new(format!("上次检查的请求状态：{}", fn_state(current))).size(15.0),
            );
            match status.as_ref().and_then(|status| status.support_stage) {
                Some(SupportStage::Candidate) => {
                    fn_note(ui, "候选支持：尚未完成 Windows 实机验收。")
                }
                Some(SupportStage::RuntimeAccepted) => {
                    fn_note(ui, "此检查匹配已记录的 Windows 运行验收条件。")
                }
                None => fn_note(ui, "尚未获得经过资格核验的检查结果。"),
            }
            fn_note(ui, "回读确认的是控制字节，不代表物理按键效果或重启后保持。");
            if let Some(received) = self.controller.windows_fn_received_at {
                let time = received
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_secs().to_string())
                    .unwrap_or_else(|_| "未知".into());
                ui.label(
                    RichText::new("检查结果已接收 · 手动授权更新")
                        .size(13.0)
                        .color(secondary(ui)),
                )
                .on_hover_text(format!(
                    "界面收到结果的时间戳：{time}。本页不自动授权，也不以普通刷新更新此缓存。"
                ));
            }
            if status.is_none() {
                fn_note(
                    ui,
                    "先显式检查 Windows Fn 锁。检查只读取资格与当前请求状态。",
                );
            }
            let can_edit = unavailable.is_none()
                && !self.controller.windows_fn_state_unknown
                && !self.controller.windows_fn_needs_probe
                && status.as_ref().is_some_and(|status| {
                    mechrevo_console_core::windows_oem::preview(
                        status,
                        self.controller.windows_fn_requested.unwrap_or(false),
                    )
                    .is_ok()
                });
            if let Some(mut target) = self.controller.windows_fn_requested {
                ui.add_space(12.0);
                if ui
                    .add_enabled(can_edit, egui::Checkbox::new(&mut target, "锁定 Fn 功能键"))
                    .changed()
                {
                    self.controller.set_windows_fn_target(target);
                }
            }
            if let Some(plan) = &self.controller.windows_fn_preview {
                let previous = match &plan.previous {
                    OemValue::Boolean(value) => Some(*value),
                    _ => None,
                };
                let target = match &plan.target {
                    OemValue::Boolean(value) => Some(*value),
                    _ => None,
                };
                ui.label(
                    RichText::new(format!(
                        "待授权更改：{} → {}",
                        fn_state(previous),
                        fn_state(target)
                    ))
                    .color(amber(ui)),
                );
                fn_note(
                    ui,
                    "预览只使用上次授权检查的缓存；应用会重新核对设备身份与完整控制字节。",
                );
            }
            if let Some(error) = &self.controller.windows_fn_draft_error {
                fn_error(ui, "请求未提交", error);
            }
            let complete = self.controller.windows_fn_outcome == Some(Outcome::AppliedVerified)
                && self.controller.windows_fn_lifetime_clean()
                && !self.controller.windows_fn_state_unknown
                && self.controller.windows_fn_error.is_none()
                && self
                    .controller
                    .windows_fn_receipt
                    .as_ref()
                    .is_some_and(|receipt| {
                        receipt.full_byte_verified == Some(true)
                            && receipt.protected_bits_preserved == Some(true)
                    });
            if complete {
                ui.label(
                    RichText::new("Fn 锁设置已回读确认。")
                        .size(13.0)
                        .color(green(ui)),
                );
            }

            egui::CollapsingHeader::new("Windows Fn 锁详情")
                .id_salt("windows-fn-details")
                .show(ui, |ui| {
                    fn_note(
                        ui,
                        "以下保留检查与事务证据；缓存回执不代表新的授权或当前硬件测量。",
                    );
                    if let Some(status) = &status {
                        ui.add(
                            egui::Label::new(format!(
                                "契约：{} · 资格核验：{}",
                                status.contract_id.as_deref().unwrap_or("未匹配"),
                                if status.qualified {
                                    "通过"
                                } else {
                                    "未通过"
                                }
                            ))
                            .wrap(),
                        );
                        if let Some(control) = &status.control {
                            evidence(ui, &control.source.backend, &control.source.path);
                            ui.add(egui::Label::new(&control.binding).wrap());
                            ui.add(egui::Label::new(&control.readback_scope).wrap());
                        }
                        for error in &status.blocking_errors {
                            fn_error_details(ui, "资格限制", error);
                        }
                        for detail in &status.diagnostics {
                            ui.add(egui::Label::new(detail).wrap());
                        }
                        if let Ok(json) = serde_json::to_string_pretty(&status.evidence) {
                            ui.add(egui::Label::new(json).wrap());
                        }
                    }
                    ui.label(format!(
                        "辅助进程：{:?} · 已确认退出：{}",
                        self.controller.windows_fn_helper_lifetime,
                        self.controller.windows_fn_helper_reaped
                    ));
                    if let Some(receipt) = &self.controller.windows_fn_receipt {
                        fn_receipt_details(ui, "上次应用事务", receipt);
                    }
                    if let Some(receipt) = &self.controller.windows_fn_followup_receipt {
                        fn_receipt_details(ui, "后续检查或等待回执", receipt);
                    }
                    if let Some(error) = &self.controller.windows_fn_error {
                        fn_error_details(ui, "上次操作", error);
                    }
                });
            ui.add_space(12.0);
            ui.horizontal_wrapped(|ui| {
                let preview_clicked = ui
                    .add_enabled(can_edit, egui::Button::new("预览 Fn 锁更改"))
                    .clicked();
                if let Some(Err(error)) =
                    preview_clicked.then(|| self.controller.preview_windows_fn())
                {
                    self.windows_fn_submit_error(error);
                }
                if self.controller.windows_fn_preview.is_some() {
                    let apply_clicked = ui
                        .add_enabled(can_edit, primary_button(ui, "应用 Fn 锁（需授权）"))
                        .clicked();
                    if let Some(Err(error)) =
                        apply_clicked.then(|| self.controller.apply_windows_fn_preview())
                    {
                        self.windows_fn_submit_error(error);
                    }
                    if ui.button("取消 Fn 锁预览").clicked() {
                        self.controller.cancel_windows_fn_preview();
                    }
                }
            });
        });
    }

    fn windows_fn_submit_error(&mut self, error: SubmitError) {
        let message = match error {
            SubmitError::Busy => self
                .controller
                .windows_fn_unavailable_reason()
                .unwrap_or("其他后台操作尚未完成。")
                .to_owned(),
            SubmitError::WorkerStopped => "后台已停止，请重新打开控制台。".into(),
            SubmitError::Unsupported => "请先完成新的授权检查，再预览更改。".into(),
            SubmitError::PreviewRequired => "Windows Fn 锁预览已失效，请重新预览。".into(),
        };
        self.controller.windows_fn_draft_error = Some(mechrevo_console_core::ControlError::new(
            "windows_fn_request_not_submitted",
            message,
        ));
    }
}

fn fn_note(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(13.0).color(secondary(ui))).wrap());
}
fn fn_state(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "开启",
        Some(false) => "关闭",
        None => "未确认",
    }
}
fn fn_proof(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "是",
        Some(false) => "否",
        None => "未确认",
    }
}
fn fn_error(ui: &mut egui::Ui, label: &str, error: &mechrevo_console_core::ControlError) {
    ui.add(
        egui::Label::new(
            RichText::new(format!("{label} · {}", error.message))
                .size(13.0)
                .color(error_color(ui)),
        )
        .wrap(),
    );
}
fn fn_error_details(ui: &mut egui::Ui, label: &str, error: &mechrevo_console_core::ControlError) {
    fn_error(ui, label, error);
    ui.add(egui::Label::new(&error.code).wrap());
    for detail in &error.details {
        ui.add(egui::Label::new(detail).wrap());
    }
}
fn fn_receipt_details(
    ui: &mut egui::Ui,
    label: &str,
    receipt: &mechrevo_console_core::windows_oem::Receipt,
) {
    ui.label(label);
    ui.label(format!(
        "上次写入尝试：{} · 完整字节回读：{} · 其他位保持：{}",
        receipt.write_attempted,
        fn_proof(receipt.full_byte_verified),
        fn_proof(receipt.protected_bits_preserved)
    ));
    ui.label(format!(
        "恢复：{} · 原始状态回读：{}",
        match receipt.recovery {
            Recovery::NotNeeded => "无需恢复",
            Recovery::OriginalObserved => "观察到原始状态",
            Recovery::RestoredVerified => "已回读确认恢复",
            Recovery::Skipped => "未执行恢复",
            Recovery::Failed => "恢复失败",
            Recovery::Unknown => "结果未知",
        },
        fn_proof(receipt.original_state_verified)
    ));
    ui.label(format!(
        "写入调用：{} · 恢复尝试：{} · 恢复调用：{} · 写入可能继续：{}",
        fn_proof(receipt.write_call_succeeded),
        receipt.restore_attempted,
        fn_proof(receipt.restore_call_succeeded),
        receipt.writer_may_continue
    ));
    if let Some(reason) = &receipt.recovery_reason {
        ui.add(egui::Label::new(reason).wrap());
    }
    if let Some(error) = &receipt.primary_error {
        fn_error_details(ui, "写入错误", error);
    }
    for error in &receipt.readback_errors {
        fn_error_details(ui, "回读错误", error);
    }
    if let Some(error) = &receipt.restore_error {
        fn_error_details(ui, "恢复错误", error);
    }
    if let Some(error) = &receipt.identity_error {
        fn_error_details(ui, "身份核验错误", error);
    }
}
