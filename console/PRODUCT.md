# 机械革命控制台

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
- Windows 已有交叉编译链接记录，Linux 已有只读启动记录；本轮实现需要重新验证，不得把视觉修改当作硬件验收。
- 风扇噪声诊断与实际调速分开处理。显示转速、分析录音或修改界面都不代表已经消除异响，不以未经核验的寄存器或风扇参数补齐功能。

## Brand Commitments

- 用户已否定旧的紫青色 Cyber-HUD 和上一轮平面仪表盘；当前方向为磨砂、极简、留白、艺术字，并补充克制的材质与交互细节。
- 石墨深色为默认主题，不使用蓝色；暖白文字建立主要层级，鼠尾草绿和琥珀只承担读数与反馈区分。
- 参考 X 上优秀设计师作品的完成度；实际查阅的公开参考为 [Rauno Freiberg 的层次设计文章](https://rauno.me/craft/depth)和 [Emil Kowalski 的公开作品](https://emilkowal.ski/)，不声称查看了受限 X 帖子。
- 磨砂观感来自应用内部的柔和背景、固定颗粒、半透明面板、细边与阴影；不依赖桌面背景捕获或系统模糊效果。
- 艺术字体只用于单个英文页标题；内嵌 Cormorant Garamond Italic 和 Manrope 并附带许可证，可变字体字重分别固定为 400 和 450。中文由系统字体补全，运行时不下载字体。
- 用户要求系统活一点：真实读数变化、短交互过渡和明确刷新反馈；不能用假数据、循环噪点或无关的持续动画装饰运行状态。
- 简体中文为主要界面语言，使用正常简洁的表达。

## Evidence on Hand

- 现有实现：crates/gui/src/desktop.rs、crates/gui/src/appearance.rs、crates/gui/src/lib.rs。
- 核心数据模型：crates/core/src/lib.rs。
- 字体来源、用途与许可证见 crates/gui/assets/fonts/README.md。
- 历史原生启动、只读采样和布局测试见 ../docs/verification/console-0.1.0.json；这些记录不自动覆盖本轮界面实现。

## Product Principles

- 首页回答电脑当前状态以及用户现在能做什么。
- 技术详情随时可查，路径和诊断不占据默认首页。
- 有限的颜色表达状态，留白与字体建立层级。
- 顶部导航下以全宽散热面板呈现主要读数：宽屏采用左侧双温度、右侧趋势、下方风扇的顺序，控制项继续排在面板下方；窄窗口按内容顺序收拢为单列。
- 不可用功能给出简短解释并允许展开原因，避免用大片禁用控件占据主要空间。
- 交互反馈有生命力，未刷新时不制造运行活动。
