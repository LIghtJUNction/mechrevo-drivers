# 原生控制台

Windows / Linux 共用 Rust 核心；界面使用 eframe / egui 0.37。
GUI 的 Rust 最低版本为 1.95，默认 workspace 测试不会构建 GUI 依赖。

```sh
cargo run --manifest-path console/Cargo.toml -p mechrevo-console-gui --features gui
```

启动仅请求读取身份、系统传感器和能力，不申请管理员权限，不安装驱动。
电源策略和屏幕亮度均须先预览、再点击应用，后端重新校验并回读。
Linux 显示亮度分别显示请求值与驱动报告值；后者可能延迟，不能当作实测亮度。
写入、回读或恢复尚未结束时，关闭窗口会延后到事务返回，界面继续响应。

主区采用独立的 Cyber-HUD 方向，当前范围见[控制台说明](../../../docs/MECHREVO-CONSOLE.md)：
紫黑底色、青色通道、CPU / 各 GPU / 风扇独立节点和可滚动的操作区。
拓扑连线表示采集通道，不推断物理热管结构；没有读数就显示未采集，保持静止。
传感器页保留设备标识、单位、来源与不可用原因，目录条目不会授予硬件控制权限。
未验证的 OEM 性能模式、风扇曲线、GPU、MUX、充电上限与水冷控制保持禁用。

完整 / 紧凑、主题、字体缩放和自动刷新偏好会保存在 eframe 的系统用户配置目录。
操作计划不持久化，切换目标或刷新会作废原计划。自动刷新会暂停等待中的操作预览。

```sh
# 无显示器的状态与后台线程回归
cargo test --manifest-path console/Cargo.toml -p mechrevo-console-gui --lib

# 中文缺字检查、真实内容几何、DPI 与关闭写事务的回归
cargo test --manifest-path console/Cargo.toml -p mechrevo-console-gui --features gui

# 隔离的截图验收构建；常规发布无需此 feature
cargo build --manifest-path console/Cargo.toml -p mechrevo-console-gui --features screenshot
```

布局测试使用明确标注的测试后端，不读取真实硬件。测试可在
`MECHREVO_GUI_CAPTURE_DIR` 指定目录输出 egui 内容的软件渲染图；这类图不能代替
原生窗口、平台字体、辅助功能、真实显示设备与 Windows 实机验收。
