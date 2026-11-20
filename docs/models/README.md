# 机型参数研究归档

[mechrevo_factory_models.json](mechrevo_factory_models.json) 保存 29 条机型或平台参数记录，用于整理型号线索、对照功耗字段与研究风扇表。记录中的 `factory_specs`、`presets` 等名称沿用归档格式；每条记录的原始安装包、固件版本、提取位置和实机验证范围仍需分别补齐。当前文件包含 17 条带 `factory_specs`／`presets` 的记录，其余记录保留各自已有字段。

实际支持范围按设备功能和固件分别记录。产品资料见 [全机型资料矩阵](../MECHREVO-ALL-MODELS-MATRIX.md)，身份规则见 [精确身份依据](../../data/mechrevo-platform-evidence.json)，已实现接口见 [OEM 功能覆盖](../OEM-FEATURE-COVERAGE.md)，实机记录见 [hardware](../hardware)。

## 字段如何理解

归档保留原始数值和标记，使用前需结合对应型号的来源与接口定义解释。项目号、占空比编码和功能标记分别承担不同用途，不能用一个字段替代完整设备身份或控制能力。

| 字段 | 归档用途与需要核对的内容 |
| --- | --- |
| `model_id`、`display_name`、`alias` | 型号、平台或名称线索；部分 ID 含 `x`，需要进一步确认覆盖的实际主板与固件。 |
| `project_id`、`bios_project_id` | 来源中的项目标记。多个条目共享同一值，例如 `TRX1` 对应多条记录；运行时身份匹配需要独立规则。 |
| `factory_specs`、`presets` | 部分条目的三档参数对象，保留 `pl1`、`pl2`、`pl4`、`temp_offset` 字段。实际单位、控制语义、允许范围和固件行为需要来源及对应后端说明。 |
| `pl1_default`、`pl2_default`、`pl4_default` | 顶层默认值字段。它们与档位对象可能不同，应分别保留并核对，不能自动选一组作为设备启动参数。 |
| `base_tgp`、`max_db` | GPU 功率相关的来源值；实际允许值与持续功耗由具体硬件、固件和驱动决定。 |
| `fan_tables` | 各档位保存 CPU／GPU 的 `upt`、`downt`、`duty` 数组及 `max_duty`。现有数组每组 16 项，包含 `255` 等原始标记；有效节点、温度语义与占空比换算需按来源解析。 |
| `max_fan_duty`、`temp_levels` | 顶层风扇相关字段。顶层值与表内值可能使用不同语义，不能直接按百分比混用。 |
| `has_water_cooler`、`keyboard_type`、`fan_control_respective` | 归档中的能力线索，实际控制还需设备枚举、协议实现与功能验证。`TypeA`／`TypeB` 的解释需要对应来源。 |
| `mine` | 归档标记；设备支持判定使用独立身份与能力规则。 |

例如 `GM6IX9B` 的顶层 `pl1_default/pl2_default/pl4_default` 为 `75/125/175`，而其 `balanced` 对象为 `75/75/125`；同一记录的 `max_fan_duty` 为 `100`，`turbo.max_duty` 为 `180`。这些差异保留为研究依据，换算与优先级需要原始来源确认。`temp_offset` 的温控结果也应结合具体处理器与固件定义，不能统一套用一个 TjMax。

## 当前程序如何读取配置

Rust 的 [registry.rs](../../console/crates/core/src/registry.rs) 读取 [mechrevo-platform-evidence.json](../../data/mechrevo-platform-evidence.json)，输出身份匹配和固件证据状态。它检查厂商及产品名或主板等多个字段，保留未知、冲突、源码识别和部分实机验证状态；本目录的参数 JSON 保持为研究资料，不参与该匹配路径。

Linux 图形界面的五档性能入口由 [oem/performance.rs](../../console/crates/core/src/oem/performance.rs) 提供。当前入口限定 `MECHREVO / GM6PX0X`，读取 tccd 已保存的配置与活动档位；应用经过预览、固定控制程序和回读。它读取的是本机配置，程序启动不会从本归档生成风扇或功耗写入。

[mechrevo_control.py](../../scripts/mechrevo_control.py) 单独维护已保存档位的同步和模式切换，并提供独立的 EC-only 模式路径。这些路径使用自己的实现、配置与驱动检查，未接入本目录参数表。当前原生控制中心的功能范围以 [控制台说明](../MECHREVO-CONSOLE.md) 为准；归档中没有可直接执行的贝塞尔风扇调节流程。

## 适配记录与后续补充

现有独立只读 [EC 传感器驱动](../../src/mechrevo_ec_sensors.c) 对以下两组身份作完整产品名、主板和 BIOS 匹配：

| 设备与主板 | BIOS | 实机资料 |
| --- | --- | --- |
| `Kuangshi16Pro Series GM6PX0X` / `GM6PX0X` | `N.1.09MRO07` | [GM6PX0X](../hardware/GM6PX0X.md) |
| `Jiaolong16K Series GM6BG0Q` / `GM6BG0Q` | `N.1.19MRO15` | [GM6BG0Q](../hardware/GM6BG0Q.md) |

这张表只描述该传感器模块。键盘、充电、系统电源和其他平台功能分别依赖各自的驱动接口与身份规则，覆盖范围在 [机械革命支持说明](../MECHREVO.md) 和 [原生内核控制](../hardware/NATIVE-KERNEL-CONTROLS.md) 中维护。

新增系列适配时，先为条目补充可复核的来源、完整设备身份及固件版本，再把已确认功能接入对应后端。功耗与散热调节应记录原配置、请求值、回读结果、负载下的实际温度和转速，以及恢复结果；资料提取、源码实现和实机结果分别更新。这样可以持续扩大机型覆盖，同时让每项调节都有明确的设备依据。
