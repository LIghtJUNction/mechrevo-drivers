# mechrevo-drivers — 机械革命 Linux 平台驱动维护分支

[![支持 mechrevo-drivers](https://donate.lmm.best/badge.svg?project=mechrevo-drivers&currency=CNY&lang=zh-CN&period=all&layout=compact&theme=dark&width=440&title=mechrevo-drivers)](https://donate.lmm.best/?project=mechrevo-drivers)

基于 [TUXEDO `tuxedo-drivers`](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers) 的非官方维护分支，保留原始历史、模块名称及许可证。

- **初始维护基线：上游 `v4.24.0`**，不是开发版 `v4.25.0_rc`。
- **当前适配：** `MECHREVO` 品牌检查、Uniwill 错误传播与 WMI 响应校验；旷世 16 Pro `GM6PX0X` 加载时保留已有电源配置；无界 14X 三档模式、蛟龙 16 Pro 2025 精确板号功耗接口、无界 15X Pro / 蛟龙 16K 接电开机与 USB 供电适配。新增机型依据公开源码完成，尚未在本项目实机验收，精确板号、固件限制见[其他机型说明](docs/hardware/OTHER-MODELS.md)。保留原有 TUXEDO 匹配和旧平台逻辑。
- **本机验证：** 旷世 16 Pro `GM6PX0X` 已安装、加载并完成部分接口验证，详见[验证记录](docs/hardware/GM6PX0X.md)；不代表完整功能认证。
- **不是性能优化包：** 尚无机械革命型号专属风扇曲线、功耗调优或实机兼容性认证。品牌匹配不能保证 EC 操作安全。
- **上游同步：** 每日镜像开发分支到 `upstream`，仅将正式版本合入自动更新 PR；`main` 不会被强推或自动合并。

阅读 [机械革命支持范围与验证](docs/MECHREVO.md)、[同步机制及维护流程](docs/MAINTENANCE.md) 和 [Arch / AUR 安装与打包](docs/AUR.md)。

GM6PX0X 的独立 CPU 长期功耗工具及其适用条件见 [功耗控制说明](docs/POWER-CONTROL.md)。
它单独安装，不包含在现有 DKMS / AUR 驱动包中；验收范围见本机验证记录。

> 本分支的 AUR 包名为 `mechrevo-drivers-dkms-git`，从本仓库 `main` 获取源码。它与其他维护者的 `mechrevo-drivers-dkms` 独立、互斥，不接管对方的包；也不要与原版 tuxedo-drivers 同时安装同名模块。AUR 发布的是构建配方，不是预编译 DKMS 二进制包。下方保留上游说明，列出的功能不代表所有机械革命机型都支持。

---

# Table of Content
- <a href="#description">Description</a>
- <a href="#building-and-install">Building and Install</a>
- <a href="#troubleshooting">Troubleshooting</a>
- <a href="#regarding-upstreaming-of-tuxedo-drivers">Regarding upstreaming of tuxedo-drivers</a>

# Description
Drivers for several platform devices for TUXEDO notebooks meant for DKMS.

## Features implemented by this driver package
- Fn-keys
- Keyboard backlight
- Fan control
- Power control
- Other sensors
- Hardware specific userspace quirks

## Modules included in this package
- clevo_acpi
- clevo_wmi
- tuxedo_keyboard
- uniwill_wmi
- ite_8291
- ite_8291_lb
- ite_8297
- ite_829x
- tuxedo_io
- tuxedo_compatibility_check
- tuxedo_nb05_keyboard
- tuxedo_nb05_power_profiles
- tuxedo_nb05_ec
- tuxedo_nb05_sensors
- tuxedo_nb04_keyboard
- tuxedo_nb04_wmi_ab
- tuxedo_nb04_wmi_bs
- tuxedo_nb04_sensors
- tuxedo_nb04_power_profiles
- tuxedo_nb04_kbd_backlight
- tuxedo_nb05_kbd_backlight
- tuxedo_nb02_nvidia_power_ctrl
- tuxedo_nb05_fan_control
- tuxi_acpi
- tuxedo_tuxi_fan_control
- stk8321
- gxtp7380

# Building and Install

## Dependencies:
All:
- make

`make package-*`:
- [simple-package-creator](https://gitlab.com/tuxedocomputers/development/packages/simple-package-creator)
- [simple-package-tools](https://gitlab.com/tuxedocomputers/development/packages/simple-package-tools)

# Troubleshooting

## The keyboard backlight control and/or touchpad toggle key combinations do not work
For all devices with a touchpad toggle key(-combo) and some devices with keyboard backlight control key-combos the driver does nothing more then to send the corresponding key event to userspace where it is the desktop environments duty to carry out the action. Some smaller desktop environments however don't bind an action to these keys by default so it seems that these keys don't work.

Please refer to your desktop environments documentation on how to set custom keybindings to fix this.

For keyboard brightness control you should use the D-Bus interface of UPower as actions for the key presses.

For touchpad toggle on X11 you can use `xinput` to enable/disable the touchpad, on Wayland the correct way is desktop environment specific.

# Regarding upstreaming of tuxedo-drivers
The code, while perfectly functional, is currently not in an upstreamable state. That being said we started an upstreaming effort and the first small part, the keyboard backlight control for the Sirius 16 Gen 1 & 2, already got accepted.

If you want to hack away at this matter yourself please follow the following precautions and guidelines to avoid breakages on both software and hardware level:
- Involve us in the whole process. Nothing is won if at some point tuxedo-control-center or the dkms variant of tuxedo-drivers stops working. Especially when you send something to the LKML, please set us in the cc.
- We mostly can't share documentation, but we can answer questions.
- Code interacting with the EC, which is most of tuxedo-drivers, can brick devices and therefore must be ensured to only run on compatible and tested devices.
- If you use tuxedo-drivers as a reference or code snippets from it, a "Codeveloped-by:\<name\> \<tuxedo_email\>" must be included in your upstream commit, with \<name\> and \<tuxedo_email\> depending on the actual part of tuxedo-drivers being used. Please talk to us regarding this.
