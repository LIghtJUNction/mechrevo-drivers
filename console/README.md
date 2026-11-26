# Mechrevo Console

独立的 Rust 工作区，包含共用核心、命令行和 Windows / Linux 原生界面。系统电源策略与屏幕亮度已有双端后端；Linux 接入驱动提供的 Fn 锁、充电档位与键盘背光控制。2026-11-02 原生工作台新增 GM6PX0X 的五档性能选择、充电快捷控制与设备入口，复用本机已有 `mechrevo-control` 和 tccd 配置，不另写寄存器或任意功率参数。机械革命控制桥接工具还负责 profile 初始化、实体性能档位灯同步和已有 TCC 的连接。Windows OEM 后端、自定义风扇曲线、MUX 与水冷等继续按[完整功能清单](../docs/OEM-FEATURE-COVERAGE.md)推进，型号目录不授予硬件写入权限。

r160 历史基线通过 146 项 Rust 测试、95 项 Python 检查、Clippy 与 Windows 编译检查；当时本机四项 OEM 控制完成 11 次可逆写入及 7 次写前拒绝验收，全部恢复原值。范围见 [OEM 验收收据](../docs/verification/oem-controls.json)，安装版本另以[部署记录](../docs/MECHREVO-CONSOLE.md)为准。2026-10-29 新增原生 Uniwill / Bitland 有限接口，范围及本轮验证见[原生驱动说明](../docs/hardware/NATIVE-KERNEL-CONTROLS.md)，未安装或实机验收。驱动回读不等于实体按键、停充或灯光测量。

后续源码补上新版内核单处理器 `platform-profile` 接口，以及无界 HPT / 星耀 P916F HPT-R 的[精确范围与固件差异](../docs/hardware/WUJIE-HPT-P916F.md)。旧聚合接口继续优先，多处理器不自动选第一个。

```sh
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- status --json
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- export --output /tmp/mechrevo-status.json
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- models 无界
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- capabilities
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- oem list
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- oem set fn-lock on --dry-run
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- profile balanced --dry-run
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-gui --features gui
```

构建和验收方式、平台边界见 [使用说明](../docs/MECHREVO-CONSOLE.md)。依赖锁定在 `Cargo.lock`。构建不安装或加载驱动，不运行 Windows 原厂程序，也不修改系统服务。

OEM 示例仅适用于 `oem list` 实际列出的 ID 和范围；预览不写入、不提权。Linux 普通用户显式应用时，经安装的受限 helper 请求管理员授权，无需把整个图形界面作为 root 运行。充电档位不等于固定百分比限充，键盘灯回读也不等于物理发光效果。完整流程见 [OEM 控制](../docs/MECHREVO-CONSOLE.md#linux-oem-控制)，五档性能桥接和本轮验证边界见该文档开头。

CLI 状态 JSON 与导出文件保留 `schema_version: 1`，新增采集起止时间、单调耗时、工具版本和运行内核信息。Linux hwmon 传感器 ID 改为解析后的输入路径，物理设备归属与原始来源路径仍分别保留；依赖旧 ID 的消费者需要迁移索引。字段范围、休眠处理及 ID 稳定性限制见 [CLI JSON 与采集时间](../docs/MECHREVO-CONSOLE.md#cli-json-与采集时间)。

官方营销目录和精确硬件身份依据分别保存在 `data/mechrevo-catalog.json` 与 `data/mechrevo-platform-evidence.json`。别名、项目号前缀、`mine` 或自定义功耗配置不能替代硬件识别。

## Linux 键盘背光时段控制

Python 控制台的现有 `mechrevo-control daemon` 和 `daemon --ec-only`
入口默认启用时段控制；Rust GUI/CLI 不会另起自动控制器。当前仅精确匹配
MECHREVO 的 GM6PX0X / `Kuangshi16Pro Series GM6PX0X` 和 GM6BG0Q /
`Jiaolong16K Series GM6BG0Q`，并要求现有 `rgb:kbd_backlight` LED 的
`max_brightness` 为 4（与现有驱动定义一致）。其他设备不写入。无需新增权限、内核逻辑或寄存器访问。

默认按本机时区：07:00（含）到 19:00（不含）熄灭，其余时间亮度 1/4。
自定义示例（时间必须为两位 `HH:MM`，两个分界不能相同）：

```sh
mechrevo-control daemon --day-start 08:00 --night-start 20:30 --night-brightness 2
# 保留外部自动控制器时，在每个运行中的 daemon 上关闭本功能：
mechrevo-control daemon --no-keyboard-schedule
```

白天区间可跨午夜，例如 `--day-start 20:00 --night-start 06:00`。
合盖优先，开盖始终按当前时段计算，不恢复缓存亮度；启动、恢复运行和时区/
时钟调整后也重新计算。守护进程每 250 ms 读取 ACPI lid 状态并核对亮度，
仅在需要调整时写入现有 LED `brightness`，不修改 RGB。
这是用户态轮询，合盖关灯存在调度/轮询延迟；进程未运行或系统挂起时不能执行。
不设置 logind inhibitor，不修改 Hyprland、logind 或任何休眠策略。
合盖状态缺失/无效、接口不支持或权限不足时记录错误并跳过，不猜测开盖状态。
该情况不能保证纠正设备原有亮度；错误不会终止原来的热键功能。

**不要让两个自动控制器同时写同一个设备。** 项目不会修改用户现有 Hyprland
合盖背光脚本。若保留它，请给所有启动的 `mechrevo-control daemon` 加上
`--no-keyboard-schedule`；若采用本功能，需自行停用旧背光控制器后再运行。
本项目的两个 daemon 之间通过机器级抽象 socket 互斥自动背光，不能阻止
Hyprland、其他程序或手动亮度写入。争取控制权前，先读取 lid、核对亮度范围，
并以读写方式打开同一个 `brightness` 端点、验证当前值；即使亮度已等于目标，
也检查实际打开权限，但不做探测写入。打开、读取、写入、flush 或回读失败会
释放控制权，另一个具有可用访问权限的现有 daemon 可在下次轮询接管。
权限检查不增加授权；只有真正写入时才能发现内核在 store 阶段拒绝请求的情况。

开盖夜间保留现有 Fn 背光键（循环/减亮/增亮）。收到物理亮度键事件后，控制器
最多等待 2 秒，在 LED 回读中确认亮度发生变化，才把新值作为当晚临时覆盖，
包括 Fn 熄灭的 0。只有按键而没有回读变化不会产生覆盖；其他途径的任意写入
不自动成为 Fn 覆盖。两个 daemon 都观察物理事件，由自动背光的实际持有者
确认变化；`--ec-only` 仍不会调用用户亮度工具。临时覆盖在合盖、进入白天、
进入另一夜间区间、控制失败交接或进程重启后清除；开盖按当前时段恢复计划值。
白天和合盖期间仍强制为 0，Fn 不建立覆盖。由于确认依赖物理事件与随后回读，
并发写入或超过 2 秒才生效的工具不保证归因为 Fn 调整；避免其他自动写灯程序。
使用 `--no-keyboard-schedule` 时保留原有 Fn 手动行为，不强制时段或合盖亮度。

服务的参数通过 systemd override 修改（本仓库不会自动启用/安装服务）：
`systemctl --user edit mechrevo-gaming-hotkey.service`，加入：

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/mechrevo-control daemon --day-start 08:00 --night-start 20:30 --night-brightness 2
```

使用系统服务时对应为 `systemctl edit mechrevo-ec-hotkey.service`，并在
`ExecStart` 保留 `daemon --ec-only`。保留旧脚本时将上述自定义参数改为
`--no-keyboard-schedule`；若两种服务都运行，两个服务都应设置关闭参数。
修改后需重新加载对应 systemd manager 并重启服务。仅文件变更不会改变已运行进程。

云端验证：`python -m unittest discover -s tests -p 'test_keyboard_schedule.py'`，
以及 `test_mechrevo_hotkey.py` 的现有入口回归。使用临时 LED/lid 文件和真实
输入 pipe，经 `main → daemon → 策略 → brightness` 验证；未在对应笔记本上
安装或验收灯光、ACPI 合盖上报、实际时区切换与睡眠恢复。源码中的既有型号
部分验收记录不代表本功能已经实机验收。
