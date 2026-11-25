# mechrevo-drivers

机械革命笔记本的社区 Linux 平台驱动，基于 TUXEDO drivers 维护。

[![Upstream baseline](https://img.shields.io/badge/upstream-v4.24.0-111111?style=flat-square)](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers)
[![AUR](https://img.shields.io/badge/AUR-mechrevo--drivers--dkms--git-111111?style=flat-square&logo=archlinux)](https://aur.archlinux.org/packages/mechrevo-drivers-dkms-git)
[![License](https://img.shields.io/github/license/LIghtJUNction/mechrevo-drivers?style=flat-square)](LICENSE)
[![Last commit](https://img.shields.io/github/last-commit/LIghtJUNction/mechrevo-drivers?style=flat-square)](https://github.com/LIghtJUNction/mechrevo-drivers/commits/main)

> [!CAUTION]
> 本项目与 MECHREVO、TUXEDO 均无官方支持关系。错误的 EC、风扇或功耗写入可能损坏硬件。安装前请核对具体型号、固件和功能的支持范围。

## 支持范围

适配依据 DMI、板号与 BIOS 精确匹配，复用已有平台接口，并修复错误传播、参数边界和并发问题。品牌识别本身不能证明某项硬件控制可用；下面的实机记录也只覆盖明确列出的功能。

| 范围 | 状态 | 说明 |
| --- | --- | --- |
| 旷世 16 Pro `GM6PX0X` | **部分实机验证** | 已验证部分背光、Fn 锁、传感器、功耗拒绝路径与状态保持，以及触摸板命令和注入热键链路；物理操作验收范围见下文 |
| 蛟龙 16K `GM6BG0Q` | **部分实机验证** | 本机核对 EC 温度、双风扇 RPM 与手动占空比、三档模式、PL1 写入效果；TDP 范围取自 OEM 控制台 profile 点，PL2/PL4 效果未测 |
| 无界 14X `GX4HRXL` | 源码适配，待本项目实机验收 | 三档模式，未复制旧式风扇表 |
| 蛟龙 16 Pro 2025 `JIAOLONG Series-X6xR55xK` | 精确身份接入，待实机验收 | 复用已有 TDP 范围和 custom-profile 流程 |
| 无界 15X Pro、翼龙 15 Pro | 精确型号/固件接入，待实机验收 | 接电开机、USB 供电等已有接口 |
| 蛟龙 16 Pro 2023 `MRID6-23` | 原生 Bitland/Uniwill 接口整合 | 不移植其他机型 EC 参数 |
| 无界 15 Pro HPT、星耀 P916F HPT-R | 精确识别与控制台适配 | 固件差异和未确认 OEM 控制保持封闭 |
| 其他 MECHREVO 机型 | **未知** | 只命中品牌字符串不能推导功能安全性 |

完整身份与功能边界见 [机械革命适配说明](docs/MECHREVO.md)。实机记录：[GM6PX0X](docs/hardware/GM6PX0X.md)、[GM6BG0Q](docs/hardware/GM6BG0Q.md)；其他范围：[机型证据](docs/hardware/OTHER-MODELS.md)、[原生内核控制](docs/hardware/NATIVE-KERNEL-CONTROLS.md)、[HPT / P916F](docs/hardware/WUJIE-HPT-P916F.md)。

## 安装（Arch Linux / AUR）

安装与当前内核匹配的 headers，再安装 DKMS 包。标准 Arch 内核示例：

```bash
sudo pacman -S --needed base-devel linux-headers
yay -S mechrevo-drivers-dkms-git
# 或：paru -S mechrevo-drivers-dkms-git
```

LTS、Zen、Hardened 或自定义内核必须安装对应 headers。安装后检查：

```bash
dkms status
```

AUR 提供构建配方，DKMS 在本机为装有 headers 的内核编译模块。更新安装包不会替换内存中已加载的模块；保留安装前版本和恢复方法，再通过重启加载新模块，避免直接热卸载相互依赖的平台驱动。

> `mechrevo-drivers-dkms-git` 与另一维护者的 `mechrevo-drivers-dkms`、原版 `tuxedo-drivers`、旧 `tuxedo-keyboard` DKMS 包互斥。它不使用 `replaces`，不会接管其他维护者的包。

安装、卸载、恢复和发布步骤见 [Arch / AUR 文档](docs/AUR.md)。

### 原生控制台

独立的 `mechrevo-console` 包提供设备状态、温度趋势和受限 OEM 控制，支持已验证 `GM6PX0X` 上的五档性能选择。它不依赖或自动升级 DKMS 包。五档控制需要有效的 tccd 配置，并保留用户修改过的参数。

```bash
yay -S mechrevo-console
# 或：paru -S mechrevo-console
```

- [AUR 触摸板修复版本：0.1.0.r1553.g97e6656-1](https://aur.archlinux.org/packages/mechrevo-console)：已包含下述触摸板实现及专用 helper。
- [安装与授权说明](packaging/arch/mechrevo-console/README.md)、[控制台功能边界](docs/MECHREVO-CONSOLE.md)。

### 触摸板开关

触摸板热键与命令行共用 Linux input 的 `inhibited` 接口，通过 udev 的 `ID_INPUT_TOUCHPAD=1` 识别设备，写入后回读状态。使用条件是内核导出该属性、udev 正确标识触摸板；输入开关不依赖具体桌面的私有接口。机制见 [Linux input 文档](https://www.kernel.org/doc/html/latest/input/input-programming.html#inhibiting-input-devices)。

控制台包内置独立 touchpad helper 和 polkit 策略。普通用户切换需要活动本地会话，由专用 polkit 动作授权，并由 helper 通过 logind 核对用户及所属 seat。helper 只接受固定操作，不接受任意文件路径；查询状态不提权，其他 OEM 控制的授权保持原有边界。

```bash
mechrevo-control touchpad status
mechrevo-control touchpad off
mechrevo-control touchpad on
mechrevo-control touchpad toggle
```

同一热键只保留一个处理入口。使用用户热键守护进程时，应移除桌面对同一键的重复切换绑定，避免一次按键切换两次。

成功结果表示内核开关已回读一致。桌面自己的“禁用触摸板”设置仍独立生效；若内核已启用但光标不移动，还需检查桌面设置。设备缺失或接口不可用时返回错误，不显示为“已禁用”。恢复入口为 `mechrevo-control touchpad on`。

**已验证范围：** GM6PX0X / Hyprland 安装 `0.1.0.r1553.g97e6656-1` 后，普通用户（UID 1000）的 `off/on` 已通过；向 evdev 注入两次 F21，用户热键守护进程回读到 `enabled: true → false → true`。物理手指操作与实际按键尚未经用户验收，其他桌面环境也未逐一实测。详细结果见[本轮验收记录](docs/verification/touchpad-driver-quality-2026-11-20.md)。源码见[触摸板实现](https://github.com/LIghtJUNction/mechrevo-drivers/commit/97e6656)与[打包提交](https://github.com/LIghtJUNction/mechrevo-drivers/commit/4194801)。

## 只读识别本机

下列命令不加载模块，也不写 EC：

```bash
uname -r
for field in \
  sys_vendor product_name product_version \
  board_vendor board_name chassis_vendor bios_version
do
  printf '%s: ' "$field"
  cat "/sys/class/dmi/id/$field" 2>/dev/null || true
done
```

反馈兼容性时请附型号、CPU/GPU、BIOS、发行版、内核、驱动版本，以及每项功能的实际结果。不要上传序列号、UUID、MAC 地址或未脱敏的完整系统报告。

## 驱动维护内容

相对上游正式版 `v4.24.0`，主要维护内容包括：

- 在原有兼容检查下增加已知机械革命板号、产品名与 BIOS 匹配。
- 修复 Uniwill TDP、WMI 和输入输出复制中的边界、错误传播与回读问题。
- 修复 WMI 设备移除、后台任务停止、LED 阻塞回调及风扇读数处理；仅在写入成功后更新配置状态。
- 统一风扇、模式和睡眠恢复中的同步与状态更新。
- 避免 `GM6PX0X` 探测时覆盖已有显卡电源配置。

这些改动不引入未知机型的 EC 地址、风扇曲线或功耗策略。`GM6PX0X` 的 CPU 长期功耗工具单独维护，未包含在 DKMS 包中，见 [功耗控制说明](docs/POWER-CONTROL.md)。

## 构建与验证

需要编译器、`make` 和当前内核 headers：

```bash
make -j2 KDIR=/usr/lib/modules/$(uname -r)/build
mkdir -p .local-tests
python3 -B scripts/verify.py --domain driver
python3 -B scripts/verify.py --domain console
# 完整范围：python3 -B scripts/verify.py --domain all
```

验证入口会把缺失依赖、跳过用例和未运行用例报告为 `incomplete`，不能只依据 unittest 的 `OK` 判断完整通过。离线测试、模块编译、磁盘安装与实际加载分别记录。

本轮触摸板、驱动质量、软件包安装与系统更新的版本和证据见 [2026-11-20 验收记录](docs/verification/touchpad-driver-quality-2026-11-20.md)。

安装和恢复请使用 DKMS 包；`make install` 会直接写入系统模块和规则，绕过包管理器，不适合作为未知机型的试用步骤。

## 上游同步

每日任务镜像开发分支到 `upstream`，仅为正式上游版本创建更新 PR；不会强推 `main` 或自动合并。合并仍需审阅和本机验证，安装后的加载与硬件结果另行核对。同步和冲突处理见 [维护文档](docs/MAINTENANCE.md)。

## 上游模块

保留上游 Clevo、Uniwill、ITE、NB04/NB05 等模块、原有匹配和用户态接口。Fn 键、背光、传感器、风扇和功耗功能是否可用，取决于各模块的 DMI、控制器与固件条件。

上游正在逐步把适合主线的部分提交到 Linux 内核。引用或移植 TUXEDO 代码时，应保留作者与许可证信息，并在提交主线前与上游协调相应 `Codeveloped-by` 归属。

## 许可证

本项目保留 TUXEDO 上游历史、版权与 GPL 许可证。具体条款见 [LICENSE](LICENSE)。

Python 控制台现有 `mechrevo-control daemon`（含 `--ec-only`）在受支持机型上
默认按本机时间控制键盘背光：07:00–19:00 熄灭、夜间亮度 1/4，合盖优先。
夜间保留 Fn 亮度键的临时调整，合盖或离开该夜间时段后清除。
支持自定义时间/亮度及 `--no-keyboard-schedule` 关闭。保留 Hyprland 旧合盖
背光脚本时必须关闭此功能，避免两个自动控制器争写。
详见 [配置、机型范围及验证限制](console/README.md#linux-键盘背光时段控制)。
