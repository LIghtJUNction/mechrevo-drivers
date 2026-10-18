# Mechrevo Console

独立的 Rust 工作区，包含共用核心、命令行和 Windows / Linux 原生界面。使用系统已有接口；原厂 EC、风扇、MUX、RGB 和水冷协议的接入进度单独记录，型号目录不授予硬件写入权限。

```sh
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- status --json
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- export --output /tmp/mechrevo-status.json
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- models 无界
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- capabilities
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-cli -- profile balanced --dry-run
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-gui --features gui
```

构建和验收方式、平台边界见 [使用说明](../docs/MECHREVO-CONSOLE.md)。依赖锁定在 `Cargo.lock`。构建不安装或加载驱动，不运行 Windows 原厂程序，也不修改系统服务。

CLI 状态 JSON 与导出文件保留 `schema_version: 1`，新增采集起止时间、单调耗时、工具版本和运行内核信息。Linux hwmon 传感器 ID 改为解析后的输入路径，物理设备归属与原始来源路径仍分别保留；依赖旧 ID 的消费者需要迁移索引。字段范围、休眠处理及 ID 稳定性限制见 [CLI JSON 与采集时间](../docs/MECHREVO-CONSOLE.md#cli-json-与采集时间)。

官方营销目录、精确硬件身份依据和上游问题快照分别保存在 `data/mechrevo-catalog.json`、`data/mechrevo-platform-evidence.json` 和 `data/openrevo-issues.json`。别名、项目号前缀、`mine` 或自定义功耗配置不能替代硬件识别。
