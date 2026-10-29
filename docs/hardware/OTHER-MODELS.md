# 其他机械革命机型

2026-10-29：以下是源码适配范围。开发机器仍是旷世 16 Pro GM6PX0X；没有这些新增型号可供实测，不能把本地测试或编译当成它们已通过硬件验收。

| 型号 | 必须同时匹配的 DMI 字段 | 本次行为 | 证据与验收边界 |
| --- | --- | --- | --- |
| 无界 14X | 厂商 `MECHREVO`、板号 `GX4HRXL` | 通过既有 ioctl 提供节能、均衡、性能三档；只改模式位，保留强冷等其他位 | [mech-forza-kmod 固定版本](https://github.com/minortex/mech-forza-kmod/blob/00769343a82f66bcbf44f58924d88b7035066b71/mechrevo-ec.c) 确认 `0x0751` 的 `0xa0/0x00/0x10`；不启用 legacy 风扇表初始化、custom-profile 位或 CPU 功耗表；BIOS 范围待实测 |
| 蛟龙 16 Pro 2025 | 厂商 `MECHREVO`、板号 `JIAOLONG Series-X6xR55xK` | 接入已有 `x6fr5xx` CPU 功耗表及 custom-profile；成功和失败沿用原来的完整事务 | [社区实测补丁固定版本](https://github.com/sklynic/pkgbuild-mechrevo-driver-dkms-16-pro/blob/cf71e4cc1b2d8d4ebdf68a98ca00910087f1678d/patch.diff) 和 [作者说明](https://github.com/sklynic/pkgbuild-mechrevo-driver-dkms-16-pro/blob/cf71e4cc1b2d8d4ebdf68a98ca00910087f1678d/README.md)；作者实测 9955HX，未据营销名称扩展到苍龙、XPro 或其他板号；本项目 BIOS 范围待实测 |
| 蛟龙 16K | 厂商 `MECHREVO`、产品名 `Jiaolong16K Series GM6BG0Q`、板号 `GM6BG0Q`；新增控制另要求 BIOS `N.1.19MRO15` | 接电开机、USB 供电开关及控制台精确识别；不新增风扇传感器驱动 | [主线驱动投稿](https://lists.openwall.net/linux-kernel/2026/10/23/301) 报告该 BIOS、EC 1.24 的实测；[Uniwill 原始实现](https://github.com/Wer-Wolf/uniwill-laptop/blob/a9e5ac3b10ed09c6d9aedd23288d3c7887afb450/uniwill-acpi.c) 确认与本分支相同的两个寄存器和位；EC 版本不是本分支识别条件，其他 EC 版本仍待验证 |
| 无界 15X Pro | 厂商 `MECHREVO`、板号 `WUJIE Series-X5SP4NAG` | 接电开机、USB 供电开关及控制台精确识别 | 投稿者的[实测说明](https://lists.openwall.net/linux-kernel/2026/09/28/2519)、[v4 补丁](https://lists.openwall.net/linux-kernel/2026/10/16/1522) 与上述 Uniwill 原始实现；投稿未限定 BIOS，不能据此认证所有固件；不启用其他机型功耗表、风扇曲线或 NVIDIA 控制 |

厂商、板号使用完整字段相等比较。类似前缀、后缀、缺失字段和其他厂商不会获得新增能力。控制台的匹配仍是 `upstream-source`，不会显示为本项目实机通过；GM6BG0Q 也不会继承蛟龙 16 Pro 的 CPU 功耗接口。

接电开机复用 `0x0726.bit3`，USB 供电复用 `0x0767.bit4`，通过已有共享读改写事务保留其他位。新型号检测只增加功能标记，不在检测时更改开关状态；加载后的 sysfs 文件是否成功注册仍需单独检查。控制台沿用实际 sysfs 属性发现和回读，不会仅因机型命中就显示功能可用。投稿中的 RGB、充电模式、Fn、风扇等其他能力没有被整体照搬成本分支支持声明。

初始化另修正一处源码确认的失败路径：以前先创建控制接口，再忽略 EC 读取失败并缓存未初始化的值。现在先成功读取和初始化状态，之后才创建接口；读取或注册失败均记录错误且不标记已加载。这是本地模拟与编译器确认的问题，未声称在新增型号上复现过。

蛟龙 16 Pro 表的校验范围为 PL1/PL2 25–162 W、PL4 25–195 W，复用既有值；这些是接口允许范围，**不是建议功耗或散热能力承诺**。PL4 是否双倍编码仍读取设备自身能力位。识别与读取不会自动写入功耗目标；手动功耗设置依旧需要已绑定的控制接口、有效索引和值，且失败会向调用者报错。custom-profile 标记也参与加载、接电与睡眠恢复，因此这些生命周期需要新增机型实测。

无界 14X 的三档能力与旧式风扇曲线初始化分开。不能因为模式寄存器相同，就复制未知风扇表。原有通用驱动加载仍有其他 EC 初始化动作；本次修改并非为未验证固件提供安全认证。

## 本地回归

`tests/test_uniwill_mechrevo_models.py` 编译实际 C 函数及功耗表，替换 DMI 和硬件传输；覆盖新增板号、相似名称拒绝、现有 TUXEDO/GM6PX0X 行为、全部模式字节、功耗边界、PL4 编码、读写失败和失败后重试。模拟版本覆盖 4.17、4.18 和 7.2.11 的预处理分支；不代表在这些真实内核或硬件上运行过。

```sh
python -B scripts/verify.py --domain driver
cargo test --manifest-path console/Cargo.toml -p mechrevo-console-core --locked
```

旧源码不能通过新增功能用例；独立替换旧 ioctl 源码也应无法通过蛟龙功耗范围用例。新增设置项的回归通过实际 feature 检测、开关读写辅助函数和初始化函数验证：严格身份和 BIOS、两个寄存器全部 256 种原值的位保留、原错误码传播、缓存不误更新、读取或注册失败不发布已加载状态、成功重试，以及识别过程零写入。控制台回归检查四个其他型号的精确身份和未实机验收状态。

本轮本地结果：驱动 506 项、控制台核心 90 项全部通过，无跳过；核心 clippy、文档生成、工作区格式检查通过；`7.2.11-arch1-1` headers 下 28 个模块编译成功。按单线程、低 CPU/I/O 优先级依次构建，没有触发远程 CI，也没有安装或加载新模块。

## 新机验收仍需完成

先记录上述非敏感 DMI、CPU/GPU、BIOS/EC 和内核版本，再逐项验证安装、实际加载模块身份、模式回读及物理表现。蛟龙功耗操作另需验证约束范围、散热、失败恢复和睡眠/接电恢复；不能在不同型号上套用 GM6PX0X 的 `mechrevo-power` 55 W 配置。

已有 `uniwill-laptop`、`mech-forza-kmod` 或另一套直接控制 EC 的驱动时，应先确认接口归属，避免多套控制器同时写硬件；本分支不会替用户卸载它们。新增机型尚未验证安装、加载、风扇、充电、键盘、睡眠和长期稳定性，也未发布新的 AUR 配方或安装到本机。
