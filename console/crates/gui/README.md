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

界面采用石墨背景、暖白文字与有限的低饱和绿／琥珀曲线，替换旧 Cyber-HUD；不以蓝色作为视觉方向。
默认主窗为 1120×760 逻辑单位。宽窗使用固定宽度侧栏，小窗改用顶部导航，概览按可用宽度转为单列。
布局、颜色与交互约定见 [界面设计](../../DESIGN.md)，功能边界见[控制台说明](../../../docs/MECHREVO-CONSOLE.md)。

首页 CPU / GPU 分别显示同类当前最高读数，曲线显示当前选定实际来源的历史，不表示物理热管拓扑。
历史按机器身份和传感器来源隔离，每个来源最多 120 点；缺失或不可用读数形成断点，不补零或补齐历史。
产品长名省略显示，悬停与设备详情可查完整名称；传感器页可展开全部设备标识、单位、来源与不可用原因。
型号目录不会授予硬件控制权限。OEM 性能模式、EC、风扇曲线、MUX、RGB 与水冷控制尚未实现，其他控制以运行时能力为准。

“实时采样”默认开启，可关闭；距上次快照至少 3 秒且空闲时只读刷新，待确认的操作预览期间暂停。开启时空闲界面每 1 秒重绘，不表示每秒读取硬件。
选中导航与常规控件使用 180 ms 过渡，后台忙碌时刷新图标旋转；可在显示设置中关闭动画。
标准／紧凑、主题、文字缩放、实时采样和动画偏好会保存在 eframe 的系统用户配置目录。
操作计划与温度历史不持久化，切换目标或手动刷新会作废原计划。

Windows 目前只有交叉编译链接证据，未完成实机硬件写入验收；界面改版不代表新增控制兼容。

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
