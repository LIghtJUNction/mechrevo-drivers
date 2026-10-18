# Rust 双端控制台

源码位于 [console](../console/README.md)，与内核驱动和 TCC 分开构建。CLI 和原生图形界面使用同一 Rust 核心。Windows 和 Linux 各有系统后端；实际功能以运行时能力结果为准。

## 本机安装（2026-10-18）

`mechrevo-console 0.1.0.r158.ga190455-1` 已通过 pacman 安装，固定源码为
`a1904557bbea55f39aadab03dbc31813e64c3f9b`。
包内文件的字节、权限、root 归属和包完整性检查通过。
安装版 CLI 的状态查询与导出读到 38 个唯一读数、33 个 hwmon 输入，时间元数据及拒绝覆盖检查通过。
安装版 GUI 在独立 compositor 中持续运行 16 秒，1280×720 截图人工复核通过；仅验证概览与只读遥测，没有执行硬件控制。
应用菜单入口为“机械革命控制台”，也可直接运行：

```sh
mechrevo-console-gui
mechrevo-console status --json
```

本机安装配方见 [Arch 打包说明](../packaging/arch/mechrevo-console/README.md)，
来源、安装及验证范围见[部署收据](verification/deployment-r158.json)。
这次安装未新增常驻服务或替换 stock TCC，没有改变现有风扇／功耗配置。
同期安装的 r158 内核驱动仍待重启激活；控制台已安装不等于新版驱动已经运行。
未发布 AUR，Windows 仍只有编译检查和开发产物，没有 Windows 实机验收。
后文各次候选验收保留当时的范围，不能将其中“未安装”理解为当前安装状态。

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

Linux hwmon 的传感器 `id` 使用解析符号链接后的完整输入路径，例如 `/sys/devices/…/hwmon/hwmon7/temp1_input`。旧版使用物理设备路径加通道名；同一设备的多个 hwmon 接口都含 `temp1_input` 时会发生 ID 冲突。新版把这些接口分别保留，即使读数相同也不会合并。多个 class 路径若指向同一输入，只保留排序后的第一个来源，并记录 `sensor_source_alias` 诊断。`device_id` 仍表示物理设备归属，`source.path` 仍保留实际枚举到的原始路径。

依赖旧 `id` 的消费者需要迁移已有索引；此变化仅涉及 Linux hwmon，电池等其他 ID 不变。新 ID 不保证跨重启或 hwmon 重新编号稳定。输入在枚举后消失时，优先使用解析后的父目录加文件名，仍无法解析才保留原路径。设备被报告为 runtime-suspended 时，控制台解析路径但不打开传感器输入，保留 ID 与来源，将读数标为不可用；它不会以旧读数或零值代替本次采样。

Windows 标准接口依据：[电源策略](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/powercfg-command-line-options)、[亮度类](https://learn.microsoft.com/en-us/windows/win32/wmicoreprov/wmimonitorbrightness)、[亮度方法](https://learn.microsoft.com/en-us/windows/win32/wmicoreprov/wmisetbrightness-method-in-class-wmimonitorbrightnessmethods)。Linux 接口依据：[backlight](https://docs.kernel.org/gpu/backlight.html)。

## CLI JSON 与采集时间

`status --json` 和 `export --output PATH` 使用相同的状态文档结构，保留 `schema_version: 1`、原有快照字段及 `identity_evidence`，新增 `collection` 对象。导出命令另外返回导出路径等确认信息；文件内保存完整状态文档。导出使用显式路径，不覆盖已有文件或符号链接，也不自动创建父目录。

| `collection` 字段 | 含义与边界 |
| --- | --- |
| `started_at`、`finished_at` | 一次完整 `Service::snapshot()` 调用前后的系统时间，包含亮度枚举。各由 UTC Unix 纪元整数秒 `unix_seconds` 与不足一秒的整数纳秒 `nanoseconds` 组成；早于纪元的时间为 `null`。 |
| `wall_clock_status` | `ordered` 表示结束不早于开始，`moved_backwards` 表示系统时间回退，任一时间早于纪元则为 `before_unix_epoch`。有序不证明时钟已同步，也不证明绝对时间准确。 |
| `monotonic_elapsed`、`monotonic_clock_status` | 使用单调时钟计算耗时，包含快照与两次系统时间读取，以整数 `seconds`、`nanoseconds` 表示；不受通常的系统校时影响。若时钟顺序检查失败，耗时为 `null`、状态为 `moved_backwards`，正常为 `ordered`。 |
| `consistency`、`scope`、`time_basis` | `consistency` 固定为 `non_atomic_sequential_reads`；各项按顺序读取，可能对应不同瞬间或系统缓存。时间范围不包含运行环境收集、型号匹配与 JSON 序列化，不能把它当作所有传感器的同时硬件采样或新鲜度保证。其余两字段在文档内说明范围和时间基准。 |
| `tool`、`build_target` | 工具名称、Cargo 包名、包版本，以及构建目标操作系统和架构。构建目标不等于驱动版本或实机验收结果。 |
| `runtime.kernel_release` | Linux 从 `/proc/sys/kernel/osrelease` 读取运行内核版本，含 `value`、`source`、`status`，失败时附 `reason`。最多接受 256 字节，拒绝无效 UTF-8、空值和非可打印版本内容；状态为 `available`、`read_failed` 或 `invalid_data`。其他平台为 `not_collected`，值与来源均为 `null`，不推测 Windows 版本。 |

整数秒与纳秒分开保存，避免用浮点时间戳丢失精度；消费者仍须使用可保留整数精度的 JSON 解析方式。采集时间可用于关联录音和系统日志，但本身不判断风扇异响原因，也不改变风扇控制。

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

以下为初版开发候选当时的记录：该阶段未安装、未发布 AUR，也未完成全系列或双端实机验收。GUI 依赖 Rust 1.95 以上；该轮使用 Rust 1.97。

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
