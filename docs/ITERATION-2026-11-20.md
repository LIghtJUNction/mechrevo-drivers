# 2026-11-20 工程迭代记录

本轮完成控制中心界面改版、Project Z 库复用、WMI 请求路径优化和 PL4 编码边界修正，并校正机型参数资料的说明。交付包括源码、针对性回归、原生窗口截图及真实开发机只读运行结果。全型号适配按具体功能、主板和固件继续推进。

## 界面与日常操作

控制中心采用石墨黑／暖白主题，配以少量暖沙色强调。常规卡片和导航移除装饰描边，侧栏通过背景和留白区分选中页面；键盘焦点仍有明确标记。性能区采用开放布局，温度区按内容自然增长，减少固定高度和嵌套面板占用。

首页优先呈现 CPU／GPU 温度、对应风扇、当前性能档位、电池保养和屏幕亮度。五档选择在足够宽的桌面中保持同排，窄窗自动换行；1280×900 的完整模拟场景中，常用预览入口进入首屏。原生窗口已截图检查，布局回归同时覆盖深浅主题、缩放、键盘操作、长文本和滚动可达性。交互继续区分请求目标与设备当前值，修改经过预览、显式应用和回读。详见 [界面设计](../console/DESIGN.md) 与 [原生截图和验收记录](../console/VISUAL-REVIEW.md)。

首屏回归使用同时包含性能、充电、屏幕亮度、温度和双风扇的完整场景，检查实际控件位置，并点击“长期接电”验证请求目标更新、设备当前值保持。自动采样在待确认预览期间暂停；写入、回读或恢复仍在进行时，窗口继续显示进度并延后关闭。

## 共享代码与驱动改进

CLI 接入 `zclap = 0.1.4`，RGB 十六进制处理接入 `zhex = 0.1.3`。GUI 调用核心的 `color::format_rgb_hex`，共用颜色格式规则；窗口与渲染继续使用 egui / eframe。实现见 [Project Z 复用提交](https://github.com/LIghtJUNction/mechrevo-drivers/commit/90f848035775d21efef6518a50964403ce09cb0b)。

Uniwill WMI 的固定 8 字节输入改为栈缓冲区，移除请求输入侧的堆分配和释放。运行时回归中，2048 次请求的该输入缓冲区堆分配计数为零；验证同时检查参数和返回行为。该结果描述请求构造成本，实际整机延迟和散热收益仍需对应工作负载测量。实现见 [WMI 优化提交](https://github.com/LIghtJUNction/mechrevo-drivers/commit/21a7e23abdcab7c123b0a78e944cbea0accfeffc)。

PL4 路径按寄存器单字节宽度及当前能力的 2 W 缩放规则限制有效上限，超过上限的请求在写入前拒绝。旧实现会把未缩放的 400 W 截成字节 144，并返回成功；新测试已实证捕获该问题。普通编码最多表示 255，有 double PL4 能力时最多表示 510，实际允许值仍取编码上限与原有型号表上限的较小值。型号功耗表、既有缩放和奇数目标舍入行为均未改变。详见 [PL4 修复提交](https://github.com/LIghtJUNction/mechrevo-drivers/commit/4b9591499a8014e9b0d3f8a2be0aa7cb546b0ea7)。

功能绑定测试夹具补齐了 READ_ONCE、smp 等原子访问兼容辅助，使既有生产代码可以重新编译验证；该单线程夹具不证明并发内存序正确性。见 [夹具修正提交](https://github.com/LIghtJUNction/mechrevo-drivers/commit/cc31792dc58d5a1c140ffddc457511bb8f473a57)。

## 验证结果

| 检查范围 | 本轮结果 |
| --- | --- |
| GUI | 31 项单元测试、1 项生命周期测试、16 项布局测试通过，共 48 项；布局以修复后的完整重跑结果为准。 |
| Core 与 CLI | 136 项通过，覆盖共享核心及命令接口。 |
| WMI 请求 | 25 项通过；2048 次请求验证输入缓冲区零堆分配。 |
| Uniwill 回归 | 448 项通过。 |
| 机型与内核兼容 | 10 项机型测试覆盖 4.17、4.18、7.2.11 分支；7.3.4 内核模块构建完成。 |
| 原生运行 | 在 `Kuangshi16Pro Series GM6PX0X / N.1.09MRO07` 上只读运行 GUI，并检查原生截图。 |

Project Z 消费者登记已提交 [Draft PR #1](https://github.com/LIghtJUNction/project-z/pull/1)。其基线已有 4 个涉及已退役包的验证失败，新消费者登记未增加失败；该 PR 继续保留草稿状态。工程本身对 zclap、zhex 的依赖已经接入并测试，不依赖此登记 PR 合并才能运行。

## 机型资料与交付范围

[机型资料说明](models/README.md) 已改为按证据描述研究归档，保留原有 29 条参数记录。文档写明真实身份注册表和本机 profile 读取路径，修正自动加载出厂曲线与贝塞尔调节的实现描述，并区分参数来源、源码接入和实机记录。不同字段的功耗默认值和风扇数值尺度保留原始差异，等待对应来源核对。

本轮驱动已经构建，但尚未安装并加载到开发机；没有发布 AUR 包，也没有完成 Windows 或全型号硬件验证。真实运行结果覆盖上述开发机的 GUI 只读路径，驱动请求与编码行为由对应回归记录支撑。

后续全型号采证按系列收集厂商、完整产品名、主板和 BIOS，核对平台接口、传感器来源与各项控制能力。新增调节逐项记录请求值、回读、负载温度与风扇转速，以及恢复结果；先扩展有来源的能力映射，再验证对应硬件行为。

资料统计持续按功能维护：型号目录用于查找产品，身份规则用于定位平台，驱动能力用于决定可执行操作。每一项支持结论都应关联到对应固件和验证记录，后续性能比较使用相同负载、供电和温度条件，避免把不同设备或运行环境的结果合并。

## 提交与交付位置

| 交付 | 提交 |
| --- | --- |
| WMI 输入缓冲区优化 | [21a7e23](https://github.com/LIghtJUNction/mechrevo-drivers/commit/21a7e23abdcab7c123b0a78e944cbea0accfeffc) |
| 功能绑定测试夹具修正 | [cc31792](https://github.com/LIghtJUNction/mechrevo-drivers/commit/cc31792dc58d5a1c140ffddc457511bb8f473a57) |
| Project Z 库复用 | [90f8480](https://github.com/LIghtJUNction/mechrevo-drivers/commit/90f848035775d21efef6518a50964403ce09cb0b) |
| PL4 编码上限修复 | [4b95914](https://github.com/LIghtJUNction/mechrevo-drivers/commit/4b9591499a8014e9b0d3f8a2be0aa7cb546b0ea7) |
| 原生 UI、布局回归与 10 张截图 | [d6993de](https://github.com/LIghtJUNction/mechrevo-drivers/commit/d6993de4817eb9299ebb30e366495bb70e3275be) |
| 机型研究资料与适配边界说明 | [e169128](https://github.com/LIghtJUNction/mechrevo-drivers/commit/e169128651b1aa175069ed9be80dd4368e31cccc) |

源码与已完成提交位于 [mechrevo-drivers](https://github.com/LIghtJUNction/mechrevo-drivers)。本轮截图、报告与验收材料同步至 [Google 云盘目录](https://drive.google.com/drive/folders/1Jj6g6ob6RficWTa2Mbf0PfSTfDapp1zo)。
