# Rust 双端控制台

源码位于 [console](../console/README.md)，与内核驱动和 TCC 分开构建。CLI 和原生图形界面使用同一 Rust 核心。Windows 和 Linux 各有系统后端；实际功能以运行时能力结果为准。

## 型号与证据

官网目录快照收录 298 条公开记录，含 178 条笔记本、70 条台式机、28 条软件、16 条外设和 6 条待分类记录。它是营销目录，不能算成 298 台已适配电脑。同一个名称可能对应多块主板和多代固件。

```sh
python -B scripts/sync_mechrevo_catalog.py --dry-run
python -B scripts/sync_mechrevo_catalog.py --check --output data/mechrevo-catalog.json
```

同步器按页校验数量、重复 ID 和总数变化，保存每页 SHA-256，网络或校验失败时保留原文件。官网没有快照令牌，因此校验完成也不能证明抓取期间绝无等量替换。

Rust 身份识别使用独立的精确规则。品牌命中仅是身份线索；GM6PX0X 的产品名、主板和 BIOS 同时命中，仍只对应历史部分实机验证。来源匹配、固件匹配、硬件验证分别表示；冲突不选择首条规则。控制台不读取序列号、UUID 或 MAC。

## 系统功能

电源策略使用 Linux `platform_profile` 和 Windows `powercfg`。这里的系统策略不等于原厂办公、狂暴或指定瓦数模式。修改先生成计划，再检查当前值，最后写入并回读；失败报告与恢复结果分开。

屏幕亮度使用 Linux backlight 或 Windows `WmiMonitorBrightness`。控制值与实际亮度分开；多屏需要选择目标。缺驱动、权限不足或不支持的目标值会返回明确错误，不显示虚构的成功状态。

遥测保留设备 ID、单位和来源。EC 温度不是 CPU 核心温度，GPU 休眠也不证明功耗为零。缺失值显示不可用。启动和查询不自动调整 GPU、风扇、充电、ICC、MUX 或功耗限值。

Windows 标准接口依据：[电源策略](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/powercfg-command-line-options)、[亮度类](https://learn.microsoft.com/en-us/windows/win32/wmicoreprov/wmimonitorbrightness)、[亮度方法](https://learn.microsoft.com/en-us/windows/win32/wmicoreprov/wmisetbrightness-method-in-class-wmimonitorbrightnessmethods)。Linux 接口依据：[backlight](https://docs.kernel.org/gpu/backlight.html)。

## 本地验证

```sh
mkdir -p .local-tests
python -B scripts/verify.py --domain console
python -B scripts/verify_console.py --output .local-tests/console-result.json
```

第一条检查 Python 同步器与验收工具契约；第二条执行 Rust 格式、测试、Clippy 和 Windows 目标编译检查。返回码 `0` 表示检查通过，`1` 表示失败，`2` 表示验证不完整。缺工具、缺目标或来源变化不能记为通过。构建记录和日志留在 `.local-tests`，不上传机器日志。

本机 Linux 运行结果、Windows 编译结果和 Windows 实机验收分别记录。交叉编译不能证明 Windows 上亮度、电源策略或厂商驱动已经运行成功。

## OpenRevo 调查与设计边界

[0.8.5 静态分析](OPENREVO-085-ANALYSIS.md)只记录实际观察到的哈希、文件偏移和调用入口。[77 项问题索引](OPENREVO-ISSUES.md)保留每项来源及验收条件。新框架规避某类失败，不等于上游问题已修复；没有对应机型实测的项目保持待验。

前期研究/设计中提出的功耗降级原因、跨机型 EC 地址、ITE 与 EC 联动、泵最低 PWM 和唤醒固定延时等主张，尚不足以作为当前实现的硬件规范。29 组内嵌配置也不能证明每台机器的出厂校准或安全上限。当前控制台没有把这些推测写入硬件。

原生界面已替换旧 Cyber-HUD，采用石墨背景、暖白文字与有限的低饱和绿／琥珀曲线，不以蓝色作为视觉方向，也不复制 OpenRevo 内嵌资产。默认主窗为 1120×760 逻辑单位，宽窗使用固定宽度侧栏，较小窗口使用顶部导航，概览另按内容宽度转为单列。首页产品长名省略显示，完整名称可在悬停或设备详情中查看；传感器页可展开全部来源。视觉与交互约定见 [界面设计](../console/DESIGN.md)。

首页 CPU / GPU 分别显示同类当前最高读数，曲线只显示当前选定实际来源的真实温度历史，不推断物理热管拓扑。历史按机器与传感器身份隔离，每来源最多 120 点；未知读数保留断点，不补零、不补齐历史。实时采样默认开启、可关闭，距上次快照至少 3 秒且空闲时只读刷新，待确认的操作预览期间暂停；开启时空闲界面每 1 秒重绘。选中导航与常规控件的过渡为 180 ms，后台忙碌时刷新图标旋转，动画可关闭且偏好会保存。界面读取仍放在单个后台任务中，重复刷新不堆积，主线程不等待硬件调用。

电源与亮度仍保留预览、应用、回读、必要恢复及写事务结束后关窗的流程。OEM 性能模式、EC、风扇曲线、MUX、RGB 与水冷控制尚未实现；这次界面改版不增加硬件兼容或 Windows 实机写入验收结论。

## 石墨界面改版验收（2026-10-18）

改版后的全工作区 67 项 Rust 测试通过，80 组布局场景通过；格式、Clippy 和 Windows 目标检查通过。展开全部传感器来源也执行真实宽度与末尾可达性检查。Linux 原生截图复核了滑轨、双风扇、长名称和来源展示，Windows 仍未实机运行。

新版开发包已分别链接并校验归档成员，源码摘要和产物摘要见 [改版验收记录](verification/console-ui-redesign.json)。[声音诊断工具](FAN-AUDIO-DIAGNOSTICS.md)另外通过 25 项音频回归与 16 项验收 runner 回归；它只分析已有本地录音，不是 GUI 风扇控制功能。以下保留初版的历史验收范围。

## 0.1.0 初版开发候选验收（2026-10-18）

这是独立子项目的开发候选，未安装、未发布 AUR，也未完成全系列或双端实机验收。GUI 依赖 Rust 1.95 以上；本轮使用 Rust 1.97。

| 检查 | 结果与范围 |
| --- | --- |
| Rust 全工作区 | 65 项通过，零忽略、零筛除；格式与 Clippy `-D warnings` 通过 |
| 原生界面布局 | 64 组场景覆盖 360×280／800×600 逻辑窗口、100／125／150／200% DPI、1.6 倍文字及不同页面；另验证写入与恢复完成前推迟关窗 |
| Python 控制台工具 | 70 项通过，覆盖静态 PE 清单、目录同步及验收脚本 |
| TCC 部署 | 工作区 TCC 域 58 项通过；隔离掉既有 UI 改动的提交版本另外通过 17 项部署回归 |
| 验收负对照 | 实际使用 `/usr/bin/true` 代替 Cargo 测试 runner，Cargo 返回 0，但验收工具正确判定为零测试、不完整 |
| Linux 运行 | 原生界面在隔离 headless compositor 中渲染并检查中文；包内 CLI 的帮助与只读状态查询通过，读到 38 项传感器及 1 个亮度设备 |
| Windows 产物 | CLI 与 GUI 均实际交叉链接出 x86_64 PE；没有 Windows 实机运行、驱动或硬件控制验收 |

本机没有暴露 ACPI `platform_profile`，普通用户也没有 backlight 写权限。这两项实际返回不可用；没有自动提权、改服务或绕过驱动。两端亮度与电源策略的成功、失败、过期计划和恢复路径由离线回归覆盖，不能据此宣称本机写入已生效。

同一份源码构建出 Linux `tar.gz` 和 Windows `zip` 开发包，归档成员逐字节校验，附 SHA-256 与来源清单。源码摘要及检查摘要见 [验收记录](verification/console-0.1.0.json)；机器原始数据、分析载荷与构建中间文件保留在本地。
