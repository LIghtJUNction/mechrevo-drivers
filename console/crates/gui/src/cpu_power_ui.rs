//! Explicit CPU package PL1 policy, within the existing Controls surface.
use super::*;
use mechrevo_console_core::{
    cpu_power::{self, Action, ActionKind, Interface, Management, Outcome, Policy, Status},
    ControlError,
};
use std::time::{SystemTime, UNIX_EPOCH};

impl DesktopApp {
    pub(super) fn cpu_power_controls(&mut self, ui: &mut egui::Ui) {
        let status = self.controller.cpu_power_status.clone();
        let unavailable = self
            .controller
            .cpu_power_unavailable_reason()
            .map(str::to_owned);
        let state_unknown = self.controller.cpu_power_state_unknown;
        let needs_probe = self.controller.cpu_power_needs_probe;
        let activity = self.controller.cpu_power_activity();

        surface(ui, |ui| {
            ui.label(RichText::new("CPU 功耗上限").size(18.0));
            cpu_note(
                ui,
                "控制 CPU package 的长期功耗上限（PL1）。系统电源档位与短时功耗上限（PL2）独立。 ",
            );
            ui.add_space(8.0);
            if ui
                .add_enabled(
                    unavailable.is_none(),
                    egui::Button::new("检查 CPU 功耗策略（需授权）"),
                )
                .clicked()
            {
                if let Err(error) = self.controller.probe_cpu_power() {
                    self.cpu_power_submit_error("检查未启动", error);
                }
            }
            if activity {
                cpu_note(
                    ui,
                    "正在等待 CPU 授权操作完成；窗口会等待实际辅助进程退出。",
                );
            } else if self.controller.cpu_power_status.is_none() {
                cpu_note(ui, "尚未检查。检查只读取策略与资格，不会设置功耗上限。");
            }
            if let Some(received_at) = self.controller.cpu_power_received_at {
                let epoch = received_at
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_secs().to_string())
                    .unwrap_or_else(|_| "Unix epoch 之前".into());
                ui.label(RichText::new(cpu_received_time(received_at)).size(13.0).color(secondary(ui)))
                    .on_hover_text(format!("界面收到上次 CPU 状态的时间，不代表硬件采集时刻。此页不会自动授权检查。\n接收时间戳：{epoch}"));
            }

            ui.add_space(12.0);
            ui.separator();
            ui.add_space(12.0);
            if let Some(status) = &status {
                cpu_status(ui, status);
            } else {
                cpu_note(ui, "托管状态：尚未检查");
                cpu_readings(ui, None);
            }
            if let Some(outcome) = self.controller.cpu_power_outcome {
                let (text, warning) = match outcome {
                    Outcome::Observed => ("上次检查已返回；这是只读观察。", false),
                    Outcome::AppliedVerified => ("上次事务已应用并回读确认。", false),
                    Outcome::RejectedBeforeWrite => ("上次操作在写入前被拒绝。", true),
                    Outcome::FailedRestoredVerified => ("上次事务失败，已回读确认恢复。", true),
                    Outcome::FailedStateUnknown => ("上次事务的 CPU 功耗策略状态未知。", true),
                };
                ui.add(
                    egui::Label::new(RichText::new(text).size(13.0).color(if warning {
                        amber(ui)
                    } else {
                        secondary(ui)
                    }))
                    .wrap(),
                );
            }
            if state_unknown {
                ui.add(
                    egui::Label::new(
                        RichText::new("CPU 状态未知：请重新授权检查，确认前禁止预览与应用策略。")
                            .size(13.0)
                            .color(amber(ui)),
                    )
                    .wrap(),
                );
            } else if needs_probe && status.is_some() {
                ui.add(egui::Label::new(RichText::new(
                    "需要重新授权检查：保留的状态供查看，新的检查确认前不能预览或应用策略。"
                ).size(13.0).color(amber(ui))).wrap());
            }
            if !self.controller.cpu_power_helper_reaped {
                ui.add(
                    egui::Label::new(
                        RichText::new(
                            "尚未确认 CPU 授权辅助进程退出；新的检查、写入和关闭窗口已阻止。",
                        )
                        .size(13.0)
                        .color(amber(ui)),
                    )
                    .wrap(),
                );
            }
            if let Some(reason) = &unavailable {
                ui.add(
                    egui::Label::new(RichText::new(reason).size(13.0).color(secondary(ui))).wrap(),
                );
            }
            if let Some(error) = &self.controller.cpu_power_error {
                cpu_error_summary(ui, "上次操作", error);
            }

            let domain = status.as_ref().and_then(cpu_edit_domain);
            let can_set = domain.is_some();
            ui.add_space(14.0);
            ui.label(RichText::new("完整策略草稿").size(15.0).strong());
            cpu_note(
                ui,
                "修改输入只保留草稿；温度、轮询和递进选项会完整进入预览。",
            );
            if let Some(domain) = domain {
                if let Some(mut policy) = self.controller.cpu_power_draft.clone() {
                    let previous = policy.clone();
                    cpu_policy_editor(ui, &mut policy, domain);
                    if policy != previous {
                        self.controller.set_cpu_power_draft(policy);
                    }
                } else {
                    let current = status
                        .as_ref()
                        .and_then(|status| status.current_policy.clone());
                    let label = if current.is_some() {
                        "编辑已检查的完整策略"
                    } else {
                        "从已检查的上限开始编辑"
                    };
                    if ui.button(label).clicked() {
                        let policy = current.unwrap_or(Policy {
                            cap_uw: domain.1 * 1_000_000,
                            target_celsius: None,
                            polling_seconds: None,
                            rapl_debounce_seconds: None,
                        });
                        self.controller.set_cpu_power_draft(policy);
                    }
                    cpu_note(ui, "起始值来自已检查的策略或运行上限，只用于建立草稿。");
                }
            } else {
                cpu_note(
                    ui,
                    "当前检查没有提供可设置的完整资格与运行范围，暂不提供数字输入。",
                );
                if let Some(policy) = &self.controller.cpu_power_draft {
                    cpu_note(ui, "先前草稿仍保留，未应用。");
                    cpu_policy_values(ui, policy);
                }
            }
            if let Some(error) = &self.controller.cpu_power_draft_error {
                cpu_error_summary(ui, "预览未完成", error);
            }

            let preview = self.controller.cpu_power_preview.clone();
            if let Some(plan) = &preview {
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(10.0);
                ui.label(
                    RichText::new(format!("待授权预览 · {}", cpu_action_name(&plan.action)))
                        .size(15.0)
                        .strong(),
                );
                cpu_policy_comparison(ui, status.as_ref(), &plan.action);
                if matches!(plan.action, Action::Clear | Action::Recover) {
                    cpu_note(
                        ui,
                        "返回原生温控策略，由原生策略动态决定功耗；不会固定写回某个瓦数。",
                    );
                }
                if matches!(plan.action, Action::Reapply) {
                    cpu_note(
                        ui,
                        "重新保持已检查的完整托管策略，保留其温度、轮询和递进选项。",
                    );
                }
            }

            ui.add_space(10.0);
            egui::CollapsingHeader::new("检查详情")
                .id_salt("cpu-power-observation-details")
                .show(ui, |ui| {
                    if let Some(error) = &self.controller.cpu_power_error {
                        cpu_error_details(ui, "操作错误", error);
                    }
                    if let Some(error) = &self.controller.cpu_power_draft_error {
                        cpu_error_details(ui, "本地预览错误", error);
                    }
                    if let Some(status) = &status {
                        if !status.blocking_errors.is_empty() {
                            cpu_wrapped_text(
                                ui,
                                &format!("资格阻断项\n{}", status.blocking_errors.join("\n")),
                                false,
                            );
                        }
                        if !status.diagnostic_errors.is_empty() {
                            cpu_wrapped_text(
                                ui,
                                &format!("附加诊断\n{}", status.diagnostic_errors.join("\n")),
                                false,
                            );
                        }
                        cpu_note(
                            ui,
                            "以下为最近收到的原始状态；当前输入草稿不会改变这些读数。",
                        );
                        if let Ok(json) = serde_json::to_string_pretty(status) {
                            cpu_wrapped_text(ui, &json, true);
                        }
                    } else {
                        cpu_note(ui, "尚未收到 CPU 状态。");
                    }
                    if let Some(plan) = &preview {
                        cpu_wrapped_text(
                            ui,
                            &format!("预览绑定的检查版本：{}", plan.revision),
                            true,
                        );
                    }
                });
            if let Some(receipt) = &self.controller.cpu_power_receipt {
                egui::CollapsingHeader::new("事务回执")
                    .id_salt("cpu-power-transaction-receipt")
                    .show(ui, |ui| {
                        cpu_note(ui, "保留的历史事务证据，不代表当前硬件读数；当前状态以上方最近收到的检查结果为准。");
                        if let Ok(json) = serde_json::to_string_pretty(receipt) {
                            cpu_wrapped_text(ui, &json, true);
                        }
                    });
            }

            // Keep authorization below all expandable evidence, so scrolling
            // through a long diagnostic never overlays the real action row.
            ui.add_space(10.0);
            let actions_enabled = unavailable.is_none() && !state_unknown && !needs_probe;
            ui.horizontal_wrapped(|ui| {
                if can_set
                    && ui
                        .add_enabled(
                            actions_enabled && self.controller.cpu_power_draft.is_some(),
                            egui::Button::new("预览完整策略"),
                        )
                        .clicked()
                {
                    if let Some(policy) = self.controller.cpu_power_draft.clone() {
                        if let Err(error) =
                            self.controller.preview_cpu_power(Action::Set { policy })
                        {
                            self.cpu_power_submit_error("完整策略预览未完成", error);
                        }
                    }
                }
                for (kind, label, action) in [
                    (ActionKind::Clear, "预览撤销托管", Action::Clear),
                    (ActionKind::Recover, "预览恢复原生策略", Action::Recover),
                    (ActionKind::Reapply, "预览重新保持上限", Action::Reapply),
                ] {
                    if status.as_ref().is_some_and(|status| {
                        status.qualified && status.allowed_actions.contains(&kind)
                    }) && ui
                        .add_enabled(actions_enabled, egui::Button::new(label))
                        .clicked()
                    {
                        if let Err(error) = self.controller.preview_cpu_power(action) {
                            self.cpu_power_submit_error("操作预览未完成", error);
                        }
                    }
                }
            });
            if let Some(plan) = &preview {
                let current_plan = status
                    .as_ref()
                    .and_then(|status| cpu_power::preview(status, plan.action.clone()).ok());
                let draft_matches = match &plan.action {
                    Action::Set { policy } => {
                        self.controller.cpu_power_draft.as_ref() == Some(policy)
                    }
                    _ => true,
                };
                let preview_valid = current_plan.as_ref() == Some(plan) && draft_matches;
                if !preview_valid && !state_unknown {
                    cpu_note(ui, "此预览与当前检查或草稿不一致，请重新检查或预览。");
                }
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            actions_enabled && preview_valid,
                            primary_button(ui, "授权应用并回读"),
                        )
                        .clicked()
                    {
                        if let Err(error) = self.controller.apply_cpu_power_preview() {
                            self.cpu_power_submit_error("应用未启动", error);
                        }
                    }
                    if ui.button("取消此预览").clicked() {
                        self.controller.cancel_cpu_power_preview();
                    }
                });
            }
        });
    }

    fn cpu_power_submit_error(&mut self, context: &str, error: crate::SubmitError) {
        // Core validation already supplied the precise original error. Do not
        // replace it, nor erase a previous helper/rollback diagnostic.
        if error == crate::SubmitError::Unsupported
            && self.controller.cpu_power_draft_error.is_some()
        {
            return;
        }
        let reason = self
            .controller
            .cpu_power_unavailable_reason()
            .map(str::to_owned)
            .unwrap_or_else(|| match error {
                crate::SubmitError::Busy => "当前操作尚未完成。".into(),
                crate::SubmitError::WorkerStopped => "后台线程已停止。".into(),
                crate::SubmitError::Unsupported => "当前 CPU 状态不允许此操作，请重新检查。".into(),
                crate::SubmitError::PreviewRequired => {
                    "预览已失效，请根据最新检查重新预览。".into()
                }
            });
        self.controller.cpu_power_draft_error = Some(ControlError::new(
            "cpu_power_ui_submission",
            format!("{context}：{reason}"),
        ));
    }
}

/// The UI has no guessed ceiling. Whole-watt input is only available when the
/// qualified status explicitly permits Set and contains a whole-watt domain.
fn cpu_edit_domain(status: &Status) -> Option<(u64, u64)> {
    if !status.qualified || !status.allowed_actions.contains(&ActionKind::Set) {
        return None;
    }
    let ceiling = status.operational_ceiling_uw?;
    let minimum = status.minimum_limit_uw;
    if minimum == 0 || ceiling < minimum {
        return None;
    }
    let minimum_watts = minimum.div_ceil(1_000_000);
    let maximum_watts = ceiling / 1_000_000;
    (minimum_watts <= maximum_watts).then_some((minimum_watts, maximum_watts))
}

fn cpu_policy_editor(ui: &mut egui::Ui, policy: &mut Policy, domain: (u64, u64)) {
    ui.add_space(8.0);
    let label = ui.label(
        RichText::new("长期上限（W）")
            .size(13.0)
            .color(label_color(ui)),
    );
    let mut watts = policy.cap_uw / 1_000_000;
    let response = ui
        .add(
            egui::DragValue::new(&mut watts)
                .range(domain.0..=domain.1)
                .speed(1.0)
                .clamp_existing_to_range(false),
        )
        .labelled_by(label.id);
    response.widget_info(|| {
        let mut info = egui::WidgetInfo::drag_value(ui.is_enabled(), watts as f64);
        info.label = Some("长期上限（W）".into());
        info
    });
    if response.changed() {
        policy.cap_uw = watts * 1_000_000;
    }
    cpu_note(
        ui,
        &format!(
            "已检查的可用范围：{}–{} W。运行上限不代表推荐设置。",
            domain.0, domain.1
        ),
    );
    if !policy.cap_uw.is_multiple_of(1_000_000) {
        cpu_note(
            ui,
            &format!(
                "草稿仍保留原始值 {}；调整后才能变为整数瓦。",
                cpu_watts(policy.cap_uw)
            ),
        );
    }
    ui.add_space(8.0);
    cpu_note(ui, "温度策略");
    ui.horizontal_wrapped(|ui| {
        if ui
            .add(egui::Button::selectable(
                policy.target_celsius.is_none(),
                "原生温度策略",
            ))
            .clicked()
        {
            policy.target_celsius = None;
        }
        if ui
            .add(egui::Button::selectable(
                policy.target_celsius == Some(90),
                "90°C",
            ))
            .clicked()
        {
            policy.target_celsius = Some(90);
        }
    });
    cpu_note(ui, "温控轮询");
    ui.horizontal_wrapped(|ui| {
        if ui
            .add(egui::Button::selectable(
                policy.polling_seconds.is_none(),
                "原生轮询",
            ))
            .clicked()
        {
            policy.polling_seconds = None;
        }
        if ui
            .add(egui::Button::selectable(
                policy.polling_seconds == Some(1),
                "1 秒",
            ))
            .clicked()
        {
            policy.polling_seconds = Some(1);
        }
    });
    cpu_note(ui, "RAPL 递进等待");
    ui.horizontal_wrapped(|ui| {
        if ui
            .add(egui::Button::selectable(
                policy.rapl_debounce_seconds.is_none(),
                "原生递进",
            ))
            .clicked()
        {
            policy.rapl_debounce_seconds = None;
        }
        if ui
            .add(egui::Button::selectable(
                policy.rapl_debounce_seconds == Some(0),
                "0 秒",
            ))
            .clicked()
        {
            policy.rapl_debounce_seconds = Some(0);
        }
    });
    cpu_note(
        ui,
        "原生选项会沿用原生温控策略；0 秒是显式设置，和原生递进不同。",
    );
    let invalid_option = policy.target_celsius.is_some_and(|value| value != 90)
        || policy.polling_seconds.is_some_and(|value| value != 1)
        || policy.rapl_debounce_seconds.is_some_and(|value| value != 0);
    let invalid_dependency = (policy.polling_seconds.is_some()
        && policy.target_celsius != Some(90))
        || (policy.rapl_debounce_seconds.is_some()
            && (policy.target_celsius != Some(90) || policy.polling_seconds != Some(1)));
    if invalid_option || invalid_dependency {
        ui.add(egui::Label::new(RichText::new(
            "此组合无法通过预览：1 秒轮询要求 90°C；0 秒递进还要求 1 秒轮询。各字段已保留，请明确选择完整组合。"
        ).size(13.0).color(amber(ui))).wrap());
        cpu_policy_values(ui, policy);
    }
}

fn cpu_status(ui: &mut egui::Ui, status: &Status) {
    let management = match status.management {
        Management::Native => "原生策略",
        Management::Managed => "已托管",
        Management::RecoveryPending => "等待恢复",
        Management::Unknown => "未知",
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("托管状态 · {management}")).size(15.0));
        ui.label(
            RichText::new(if status.holding_now {
                "检查时正在保持上限"
            } else {
                "检查时未确认正在保持上限"
            })
            .size(13.0)
            .color(if status.holding_now {
                green(ui)
            } else {
                secondary(ui)
            }),
        );
    });
    cpu_note(
        ui,
        match status.thermald_active {
            Some(true) => "原生温控服务：运行中",
            Some(false) => "原生温控服务：未运行",
            None => "原生温控服务：—",
        },
    );
    if !status.qualified {
        ui.add(
            egui::Label::new(
                RichText::new("当前检查未建立可写资格。")
                    .size(13.0)
                    .color(amber(ui)),
            )
            .wrap(),
        );
    }
    ui.add_space(8.0);
    cpu_readings(ui, Some(status));
    cpu_note(
        ui,
        &format!(
            "已检查运行上限：{}",
            status
                .operational_ceiling_uw
                .map(cpu_watts)
                .unwrap_or_else(|| "—".into())
        ),
    );
    cpu_note(
        ui,
        &format!(
            "散热规格参考：{} · 仅供参考，不是推荐设置",
            cpu_watts(status.thermal_spec_reference_uw)
        ),
    );
    if let Some(policy) = &status.current_policy {
        ui.add_space(8.0);
        cpu_note(ui, "已检查的完整托管策略");
        cpu_policy_values(ui, policy);
    }
}

fn cpu_readings(ui: &mut egui::Ui, status: Option<&Status>) {
    ui.horizontal_wrapped(|ui| {
        for (interface, name) in [(Interface::Msr, "MSR"), (Interface::Mmio, "MMIO")] {
            let value = status
                .and_then(|status| {
                    status
                        .observed
                        .iter()
                        .find(|reading| reading.interface == interface)
                })
                .and_then(|reading| reading.pl1_uw)
                .map(cpu_watts)
                .unwrap_or_else(|| "—".into());
            ui.label(
                RichText::new(format!("{name}  {value}"))
                    .size(19.0)
                    .color(foreground(ui)),
            );
            ui.add_space(12.0);
        }
    });
    cpu_note(ui, "以上为检查返回的 PL1 读数，不随概览采样自动更新。");
}

fn cpu_policy_values(ui: &mut egui::Ui, policy: &Policy) {
    for (name, value) in cpu_policy_fields(Some(policy), false) {
        cpu_note(ui, &format!("{name}：{value}"));
    }
}

fn cpu_policy_comparison(ui: &mut egui::Ui, status: Option<&Status>, action: &Action) {
    let before = status.and_then(|status| status.current_policy.as_ref());
    let before_native = status.is_some_and(|status| status.management == Management::Native);
    let (after, after_native) = match action {
        Action::Set { policy } => (Some(policy), false),
        Action::Clear | Action::Recover => (None, true),
        Action::Reapply => (before, false),
    };
    for ((name, previous), (_, target)) in cpu_policy_fields(before, before_native)
        .into_iter()
        .zip(cpu_policy_fields(after, after_native))
    {
        ui.add(
            egui::Label::new(
                RichText::new(format!("{name} · {previous} → {target}"))
                    .size(13.0)
                    .color(label_color(ui)),
            )
            .wrap(),
        );
    }
}

fn cpu_policy_fields(policy: Option<&Policy>, native: bool) -> [(&'static str, String); 4] {
    if let Some(policy) = policy {
        [
            ("长期上限", cpu_watts(policy.cap_uw)),
            (
                "温度策略",
                policy
                    .target_celsius
                    .map(|value| format!("{value}°C"))
                    .unwrap_or_else(|| "原生温度策略".into()),
            ),
            (
                "温控轮询",
                policy
                    .polling_seconds
                    .map(|value| format!("{value} 秒"))
                    .unwrap_or_else(|| "原生轮询".into()),
            ),
            (
                "RAPL 递进",
                policy
                    .rapl_debounce_seconds
                    .map(|value| format!("{value} 秒"))
                    .unwrap_or_else(|| "原生递进".into()),
            ),
        ]
    } else if native {
        [
            ("长期上限", "原生动态策略".into()),
            ("温度策略", "原生温度策略".into()),
            ("温控轮询", "原生轮询".into()),
            ("RAPL 递进", "原生递进".into()),
        ]
    } else {
        [
            ("长期上限", "—".into()),
            ("温度策略", "—".into()),
            ("温控轮询", "—".into()),
            ("RAPL 递进", "—".into()),
        ]
    }
}

fn cpu_action_name(action: &Action) -> &'static str {
    match action {
        Action::Set { .. } => "设置完整策略",
        Action::Clear => "撤销托管",
        Action::Recover => "恢复原生策略",
        Action::Reapply => "重新保持上限",
    }
}

fn cpu_watts(value: u64) -> String {
    let whole = value / 1_000_000;
    let fraction = value % 1_000_000;
    if fraction == 0 {
        format!("{whole} W")
    } else {
        let digits = format!("{fraction:06}");
        format!("{whole}.{} W", digits.trim_end_matches('0'))
    }
}

fn cpu_received_time(time: SystemTime) -> String {
    match time.duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            let seconds = duration.as_secs();
            format!(
                "状态接收时间：{:02}:{:02}:{:02} UTC",
                seconds / 3600 % 24,
                seconds / 60 % 60,
                seconds % 60
            )
        }
        Err(_) => "状态接收时间：Unix epoch 之前".into(),
    }
}

fn cpu_note(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(13.0).color(secondary(ui))).wrap());
}

fn cpu_error_summary(ui: &mut egui::Ui, prefix: &str, error: &ControlError) {
    let mut message: String = error.message.chars().take(110).collect();
    if error.message.chars().count() > 110 {
        message.push('…');
    }
    ui.add(
        egui::Label::new(
            RichText::new(format!("{prefix}：{message}；完整内容见检查详情。"))
                .size(13.0)
                .color(error_color(ui)),
        )
        .wrap(),
    );
}

fn cpu_error_details(ui: &mut egui::Ui, title: &str, error: &ControlError) {
    let mut detail = format!("{title}\n{}\n{}", error.code, error.message);
    for line in &error.details {
        detail.push('\n');
        detail.push_str(line);
    }
    cpu_wrapped_text(ui, &detail, false);
}

fn cpu_wrapped_text(ui: &mut egui::Ui, text: &str, monospace: bool) {
    let text = RichText::new(text).size(13.0).color(secondary(ui));
    ui.add(egui::Label::new(if monospace { text.monospace() } else { text }).wrap());
}
