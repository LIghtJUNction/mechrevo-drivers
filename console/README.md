# Mechrevo Console

独立的 Rust 工作区，包含共用核心、命令行和 Windows / Linux 原生界面。系统电源策略与屏幕亮度已有双端后端；Linux 已接入驱动提供的 Fn 锁、充电档位、键盘背光及独立风扇曲线会话，新增 CPU PL1 控制复用独立功耗工具。Windows CPU PL1／OEM 后端、MUX、水冷和完整实机验收继续按[完整功能清单](../docs/OPENREVO-PARITY.md)推进，型号目录不授予硬件写入权限。

此前四项 OEM 候选通过 146 项 Rust 测试、95 项 Python 检查、Clippy 与 Windows 编译检查；本机完成 11 次可逆写入及 7 次写前拒绝验收，全部恢复原值。范围见 [OEM 验收收据](../docs/verification/oem-controls.json)，不将这些历史结果计为新增 CPU PL1 接口的验收；安装版本另以[部署记录](../docs/MECHREVO-CONSOLE.md)为准。驱动回读不等于实体按键、停充或灯光测量。

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

OEM 示例仅适用于 `oem list` 实际列出的 ID 和范围；预览不写入、不提权。Linux 普通用户显式应用时，经安装的受限 helper 请求管理员授权，无需把整个图形界面作为 root 运行。充电档位不等于固定百分比限充，键盘灯回读也不等于物理发光效果。完整流程见[Linux OEM 控制](../docs/MECHREVO-CONSOLE.md#linux-oem-控制)。

## Linux CPU 长期功耗上限

新增核心、CLI 和“控制”页的 CPU 区域共用独立 PL1 接口，与 `profile` 系统电源方案分开。它依赖 `mechrevo-power-control` 包的固定授权 helper 和策略；仅允许现有功耗工具核验的本机组合，缺少工具或资格不符会拒绝，不回退为裸 RAPL 或 EC 写入。协议见 [CPU PL1 契约](CPU-POWER-PROTOCOL.md)，支持条件与完整用法见 [功耗说明](../docs/POWER-CONTROL.md#控制台-cpu-pl1-接口)。

```sh
mechrevo-console cpu-power probe
mechrevo-console cpu-power set 55 --dry-run
mechrevo-console cpu-power set 55
mechrevo-console cpu-power clear --dry-run
mechrevo-console cpu-power clear
mechrevo-console cpu-power recover --dry-run
mechrevo-console cpu-power reapply --dry-run
```

这些 CPU 命令都会显式授权只读检查；`--dry-run` 在检查后仅本地预览，不应用策略。GUI 先点“检查 CPU 功耗策略（需授权）”，再编辑完整策略、预览和授权应用；启动、完整状态查询与自动采样不会调用此检查。GUI 显示收到观察的时间，温度自动更新不代表 CPU 策略已重新检查。

`set` 仅替换瓦数及明确指定的选项，省略项保留已观察的完整策略，包括 90°C／1 秒轮询／0 秒 RAPL 递进或原生配置。显式选项为 `--target-celsius native|90`、`--polling-seconds native|1`、`--rapl-debounce-seconds native|0`，依赖不满足时拒绝，不默默清除。35 W 到实际初始运行上限只是允许区间；示例 55 W 和界面起始值均非推荐值。只管理 PL1，不改变 PL2、Tau 或硬件 TCC。

CPU 策略在 CLI 退出或 GUI 关闭后保留；`clear`／`recover` 返回原生 thermald 策略，不固定写回某个瓦数，`reapply` 重新保持已检查的完整托管策略。状态未知时需重新检查，真实授权进程未确认退出时阻止后续操作及关窗。Windows 未实现此 CPU 后端。上述为源码接口说明，不宣称已经安装或完成新的实机验收。

CLI 状态 JSON 与导出文件保留 `schema_version: 1`，新增采集起止时间、单调耗时、工具版本和运行内核信息。Linux hwmon 传感器 ID 改为解析后的输入路径，物理设备归属与原始来源路径仍分别保留；依赖旧 ID 的消费者需要迁移索引。字段范围、休眠处理及 ID 稳定性限制见 [CLI JSON 与采集时间](../docs/MECHREVO-CONSOLE.md#cli-json-与采集时间)。

官方营销目录、精确硬件身份依据和上游问题快照分别保存在 `data/mechrevo-catalog.json`、`data/mechrevo-platform-evidence.json` 和 `data/openrevo-issues.json`。别名、项目号前缀、`mine` 或自定义功耗配置不能替代硬件识别。
