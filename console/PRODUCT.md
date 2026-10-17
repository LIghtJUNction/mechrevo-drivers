# 机械革命控制台

<!-- impeccable:product-schema 1 -->

## Platform

Native desktop: Windows and Linux. The renderer is Rust egui, not a web page.

## Users

机械革命笔记本用户，需要查看温度、风扇和供电状态，并调整系统支持的电源策略与屏幕亮度。

## Product Purpose

让用户迅速了解电脑正在怎样运行，找到可用的控制项，并在应用后看到实际回读结果。

## Capabilities and Constraints

- 独立 Rust workspace，GUI 与 CLI 共用核心。现有后端与操作预览、回读、恢复流程必须保留。
- 温度与风扇来自实际传感器；缺失、睡眠和未知不能伪造成零。趋势只能由真实采样构成。
- 官方型号目录不代表硬件控制兼容。EC、风扇曲线、MUX、RGB 和水冷仍未实现。
- 原生窗口可缩放，高 DPI、长设备名和大字体下仍需可操作。
- Windows 仅完成交叉编译链接，Linux 只读启动已验证；不得把视觉修改当作硬件验收。

## Brand Commitments

- 用户已否定旧的紫青色 Cyber-HUD 仪表盘，要求现代界面。
- 用户选择石墨深色，并明确不喜欢蓝色。
- 用户要求系统活一点：真实读数变化、短交互过渡和明确刷新反馈；不能死寂，也不能用假数据装饰。
- 简体中文为主要界面语言，使用正常简洁的表达。

## Evidence on Hand

- 现有实现：crates/gui/src/desktop.rs、crates/gui/src/lib.rs。
- 核心数据模型：crates/core/src/lib.rs。
- 原生启动、只读采样和已运行的布局测试见 ../docs/verification/console-0.1.0.json。

## Product Principles

- 首页回答电脑当前状态以及用户现在能做什么。
- 技术详情随时可查，路径和诊断不占据默认首页。
- 有限的颜色表达状态，留白与字体建立层级。
- 交互反馈有生命力，未刷新时不制造运行活动。
