# 原生界面验收 · 2026-11-20

本轮重做了控制中心的视觉层级：石墨黑与暖白主题、无描边的浅层面板、克制的暖色强调、更紧凑的侧栏和性能档位。实时读数先于日常操作，性能档位直接排列在页面上，减少重复标题与嵌套卡片。布局沿用原生 egui / eframe；后台控制流程继续经过预览、显式应用和回读。

## 原生截图

下列截图由开发机上的实际 eframe / OpenGL 窗口生成。界面预览直接调用生产 `DesktopApp`，使用明确标注的模拟数据，后台拒绝硬件写入。模拟数值和目录条目仅用于检查布局。

| 视图 | 尺寸 | 截图 |
| --- | --- | --- |
| 深色概览 | 1280×900 | [查看](screenshots/overview-dark.png) |
| 浅色概览 | 1280×900 | [查看](screenshots/overview-light.png) |
| 设备控制 | 1280×900 | [查看](screenshots/controls-dark.png) |
| 传感器 | 1280×900 | [查看](screenshots/sensors-dark.png) |
| 深色型号目录 | 1280×900 | [查看](screenshots/catalog-dark.png) |
| 浅色型号目录 | 1280×900 | [查看](screenshots/catalog-light.png) |
| 窄窗概览 | 400×780 | [查看](screenshots/overview-narrow.png) |
| 160% 文字概览 | 1120×760 | [查看](screenshots/overview-large-text.png) |
| 最小窗口，160% 文字 | 360×280 | [查看](screenshots/minimum-160-percent.png) |
| 真实设备，只读运行 | 1280×900 | [查看](screenshots/live-device-dark.png) |

已检查深浅主题、控制与传感器页面、目录、窄窗和大字体的中文排版、对比度、内容溢出、选中状态。小窗口和大字体通过滚动保留内容；不要求在最小窗口内展示全部控件。

真实设备截图来自 `Kuangshi16Pro Series GM6PX0X`、BIOS `N.1.09MRO07`。生产程序在独立 Xvfb 会话中读取了 CPU / GPU 温度、风扇转速和当前状态，没有点击或执行硬件写入。图中的权限提示如实保留。温度是当时读数，不构成散热或性能基准；当前内核仍运行既有模块，本轮修改后的驱动尚未安装或加载。

`native-system.png` 与 `before-native-system.png` 是 2026-11-09 的历史证据，不属于本轮重新生成的截图。

## 自动验证

GUI 合计 48 项测试通过：

- 31 项库测试覆盖状态、数据和主题，包括文字对比度。
- 1 项关闭时的写事务生命周期测试。
- 16 项布局与交互测试覆盖 100% / 125% / 150% / 200% DPI、160% 文字、360×280 最小窗口、长标识、错误状态、两套主题、导航交互和 OEM 编辑控件。

新增的首屏回归在两套主题的 1280×900 视口检查五个性能档位处于同一行，当前状态、充电和亮度操作处于可见区域。它还验证“长期接电”只改变待提交目标，未改变硬件回读值，也未启动后台操作。

首轮布局测试暴露了两个问题：亮度预览操作落到首屏下方，以及导航悬停边框造成 2 px 几何变化。缩短趋势区域并统一导航边框后，重新运行完整布局测试目标，16 项全部通过。31 项库测试和生命周期测试在此前运行中通过。原生 GUI 与截图示例均编译成功。

本轮实际运行：

```sh
cargo test --manifest-path console/Cargo.toml -p mechrevo-console-gui \
  --features screenshot --locked --offline
cargo test --manifest-path console/Cargo.toml -p mechrevo-console-gui \
  --features screenshot --locked --offline --test headless_layout
cargo build --manifest-path console/Cargo.toml -p mechrevo-console-gui \
  --features screenshot --bins --example ui_preview --locked --offline
cargo fmt --manifest-path console/Cargo.toml -p mechrevo-console-gui --check
```

第一条测试命令的布局部分在修复前有上述两项失败，第二条是修复后的完整布局复验，不将第一次运行记作全绿。

## 重现截图

在图形桌面可使用截图特性；无桌面环境可通过 Xvfb 启动相同原生渲染程序：

```sh
env -u WAYLAND_DISPLAY EFRAME_SCREENSHOT_TO=/tmp/mechrevo-overview.png \
  xvfb-run -a -s '-screen 0 1280x900x24' \
  console/target/debug/examples/ui_preview --width 1280 --height 900
```

`ui_preview` 支持 `--light`、`--section controls|sensors|catalog`、`--width`、`--height` 和 `--scale`。生产程序不加载预览数据。

验证环境为 Linux、Rust 1.100、内核 7.3.4-arch1-1；当前 Rust 工作区最低版本为 1.99.1。本轮没有 Windows 实机验收、硬件写入验收或全型号回归，也未发布新 AUR 包。UI 改版不扩大驱动的硬件能力声明；型号资料与适配边界见 [型号说明](../docs/models/README.md)。
