# OpenRevo 0.8.5 静态分析

本轮确认发布包内含原生 Windows 程序、嵌入驱动与辅助程序、压缩前端，以及一段包含 29 条机型记录的 JSON。可以据此研究界面与通信分层；不能据此宣布本仓库、本机或机械革命全系列已经兼容。

**没有运行 Windows 程序、辅助程序或驱动，没有安装、加载驱动，也没有执行硬件写入。** 本文的 `seen` 表示看到文件、结构、字符串或静态代码；`inferred` 表示由这些证据作出的推断；`unknown` 表示尚未证实。静态代码不是运行结果。

## 来源与复核方式

- [0.8.5 发布页](https://github.com/faintonce/open-revo/releases/tag/0.8.5)，tag 指向 `bdd22b508efba36368ea0e483608580ad4a90883`。
- 归档：`open-revo-v0.8.5.7z`，GitHub 资产摘要 `04ddf1c8634c077d2b0960b383f5fa742f7564c08a8f38e90e9ba893a51a0093`。
- 外层 `open-revo.exe`：8,098,816 字节，SHA-256 `3e3b9d2297e36a0361d56c0f6357afdd9ebc8b201b943546c5cf8002cfabda09`。
- 本轮使用用户指定的 [aicodeking/leanrea](https://github.com/aicodeking/leanrea) 工作流：先 `file`、魔数和 PE 清单，再定向读 imports/resources/strings，最后解码必要的内嵌前端。工具为 Python 3.14.7、pefile 2024.8.26、GNU objdump 2.47、系统 Brotli 解码库。没有安装新的分析工具，没有反汇编整份程序。
- 公共机器可读证据见 [data/openrevo-085-analysis.json](../data/openrevo-085-analysis.json)。公开文件只记录摘要、偏移、工具和观察状态，不包含厂商代码、内嵌界面、寄存器表或机型功耗/风扇表。

下文 `exe+0x...` 是固定摘要外层文件的绝对字节偏移。`js+0x...` 是解码后前端的 **UTF-8 字节偏移**，不是 PE 偏移，也不是 JavaScript 字符下标。解码 JS 的 SHA-256 为 `7e8cfb80ab466678a1b698fcad4556a9c00e0d71bd0cca1f56891cd2e020ae74`，长度 555,042 字节。

## 可重复的本地盘点

[`scripts/analyze_openrevo.py`](../scripts/analyze_openrevo.py) 只使用 Python 标准库。它只接受上述固定体积和摘要的 0.8.5 外层 exe，不会执行、安装或导出任何载荷；输出为 PE/import、资产路径标记和已知通信/门控标记的偏移及摘要。已有输出文件、目录、有效或悬空链接均不覆盖。输出先在同一文件系统完成，再原子地独占发布；不支持硬链接的文件系统会明确失败，不退回覆盖写入。

```sh
python scripts/analyze_openrevo.py /path/to/open-revo.exe --output /path/to/new-inventory.json
PYTHONDONTWRITEBYTECODE=1 python -m unittest discover -s tests -p 'test_analyze_openrevo.py' -v
```

本轮 23 个合成 PE/输出测试通过，覆盖截断、错误 magic、节/RVA 重叠与越界、virtual-only 数据、导入目录和字符串/表终止边界、按名与 ordinal 导入、IAT fallback、资产后缀误报、摘要拒绝、独占输出及并发目标保留。合成测试不依赖私有发布包。

另对固定摘要的真实 exe 单独运行接受检查，得到 6 个节、28 个导入 DLL、420 个导入符号、3 个资产路径标记及 10 类已知标记；与独立 pefile 解析的 DLL、符号、ordinal 和名称偏移逐项相同。公共证据清单的 24 段偏移摘要也已对原始/解码字节复核。以上属于本地离线检查，不是运行或硬件验收。

## 文件结构：seen

外层文件为 PE32+ x86-64 GUI 程序，包含 6 个节；CLR header 为空，因此外层并非普通托管 .NET 程序。`exe+0x7b2310` 的 manifest 请求 `requireAdministrator`。外层 PE 的 Security Directory 为空，不能把它当作已通过签名验证的程序。

PE 资源有 9 个叶节点，包含图标、版本和 manifest。前端资产不是独立的 PE 图标资源，而是位于数据区中的 Brotli 数据流：

| 资产 | 压缩流 exe 偏移 | 压缩字节 | 解码字节 |
| --- | --- | ---: | ---: |
| PNG 标识图 | `0x65f491` | 34,043 | 34,039 |
| 主 JavaScript | `0x667b13` | 119,855 | 555,042 |
| 主 CSS | `0x684f5c` | 4,962 | 30,319 |

解码产物仅保留在被 Git 忽略的本地研究目录，不随本仓库分发。

还发现两个内嵌 PE：

| exe 偏移 | 已见结构 | 边界 |
| --- | --- | --- |
| `0x6bb753` | x86 PE，导入 `mscoree.dll!_CorExeMain`；按节计算为 4,608 字节 | 说明存在托管辅助载荷，不代表执行过它或确认了用途 |
| `0x6bf417` | x64 Native Subsystem PE，导入内核、WDF 与 WPP 接口，调试元数据命名 UWACPIDriver | 按节及 Security Directory 计算为 46,352 字节；有证书表不等于已验证签名可信或目标 Windows 会接受加载 |

第二个载荷包含证书目录；本文保留包含该目录的摘要 `6e360ee4a0c42b2a4eaf46208c1592c87f8cd71aca2a6c52831a2ad356e61235`。只计算节数据得到的摘要不同，不能混用。

## 机型库与公开配置：seen / unknown

`exe+0x6ce173` 开始的一段 178,713 字节 JSON 可由有边界的 JSON 解码器解析为 **29 条记录**；这与 README 中概述的“24 种机型”不是同一项可直接互换的数字。JSON 块 SHA-256 为 `049507c0854c0cc8e95bad205c1262904eead031fb0548855dfe46d917d57bfa`。

本轮看到了身份、BIOS/project、能力、预设和风扇表等字段；这只能证明打包了数据，不能证明 29 条都经过实机验证，不能把该段直接导入本仓库作为支持白名单。没有在此次 ASCII 字节搜索中命中 `GM6PX0X`，也不能据此推断该软件绝对不支持本机。

公开 [models 配置指南](https://github.com/faintonce/open-revo/blob/bdd22b508efba36368ea0e483608580ad4a90883/docs/models_customization_guide.md) 定义了 `model_id`、`display_name`、`alias`、`mine`，以及 `factory_specs` 与 `presets` 的分工。独立实现可以参考这种数据分层，但应把可编辑文件当用户输入：昵称、显示名称、`mine` 和用户改写的“出厂值”都不能授予 EC 写权限。公开指南没有给出完整寄存器协议，也不能由例子的瓦数推导其他主板的限制。

## 通信路径：seen / inferred

以下是文件证据，不是设备操作记录：

| 路径 | 已见证据 | 结论状态 |
| --- | --- | --- |
| Windows 设备 IOCTL | `DeviceIoControl` import，`exe+0x773bdc`；EC 读写诊断，`exe+0x6bcb39`、`0x6bcecd` | seen：API 与诊断存在；inferred：存在独立设备桥接通道 |
| ACPI 设备驱动 | 内嵌 INF 的 ACPI 设备标记，`exe+0x6beefe`；内嵌 Native PE，`exe+0x6bf417` | seen：设备与驱动载荷存在；unknown：目标机绑定和兼容性 |
| HID 灯控 | `HidD_SetFeature` import，`exe+0x773222`，以及其他 HID/SetupAPI imports | seen：具备 HID 访问依赖；unknown：每款键盘的报告布局与实机效果 |
| WMI/CIM | `AcpiTest_MULong` 与 `GetSetULong` 标记，`exe+0x65d153`、`0x65d1c2` | seen：命令文本存在；unknown：实际选择条件及错误处理覆盖 |
| UEFI 变量 | `SetFirmwareEnvironmentVariableW` import，`exe+0x7733d0`，以及权限与 GPU 模式诊断 | seen：固件变量接口依赖；unknown：所有固件的字段布局和可恢复性 |

解码前端在 `js+0x23087` 使用 Tauri invoke 桥接；针对字面量调用的有界扫描得到 100 个不同的应用命令名（不含 Tauri plugin 命令）。这不是完整后端接口总数，也不证明每个请求都成功注册或生效。所见请求覆盖状态、功耗模式、风扇、灯光、电池、显卡、BIOS 菜单、官方服务与外置水冷。

## 门控与显示状态：seen / unknown

门控指“检查当前硬件是否允许这项操作”。以下位置可用于独立实现的需求分析，不能当作整套安全机制已经通过验证：

- **宝龙达隔离，seen：** `exe+0x6b9f00` 的诊断描述拒绝初始化物理 EC 并返回无动作桥接；`exe+0x6ba676` 描述检测到该平台后的终止；`exe+0x65cf00` 附近有软件残留、ACPI 节点、BIOS 镜像与 SMBIOS 检查的诊断。**unknown：** 未执行，未证实这些分支覆盖所有入口，也未证实识别结果。
- **显卡切换，seen：** 前端 `js+0x57cfc` 有拒绝不支持的纯核显切换的路径，原生数据 `exe+0x6bae1a` 也有拦截写入的诊断。`js+0x57b5b` 在能力对象缺失时默认允许该能力。**unknown：** 不能据前端推断后端同样缺少拦截，也不能反过来据诊断宣称后端覆盖完整。
- **BIOS 菜单，seen：** `js+0x72983` 只在“已读到硬件状态且声明不支持”时阻止按钮操作；`js+0x72a50` 随后有修改请求。读取失败不是已支持。独立实现应在能力未知时拒绝写入。
- **电池强效限制，seen：** `js+0x5ed50` 请求单独的硬件限制开关；`js+0x3bb4a` 的另一电池控件仅在支持标志明确为 false 时隐藏。**unknown：** 未证实平台门控、充电回差、EC 版本与掉电恢复行为，不启用为通用能力。
- **遥测显示，seen：** `js+0x839cf` 请求硬件状态后合并尚未过期的乐观更新；相关前端逻辑会临时用固定参考 RPM 覆盖风扇显示。界面上刚更新的数字不能代替驱动回读或传感器观测。独立实现应区分“目标”“已请求”“已回读”。

这些位置用于记录行为依据，不构成对 OpenRevo 的完整缺陷审计；只观察到前端分支时，不把它升级成后端漏洞结论。

## 独立 Rust 控制台可采用的边界

借鉴能力描述、状态读取、用户策略、操作计划和实际设备后端的分层。Linux 可以优先使用已经存在且已核验的内核接口；Windows 保持单独后端，只有在明确识别平台并验证通信契约后才启用写入。模拟模式可覆盖界面和失败路径，但不能作为硬件验收。

未知机型与未知能力默认只读；导入配置不能扩充型号授权或功耗上限。对多步硬件修改先保存可恢复状态、验证范围，再串行执行并回读，准确返回部分失败。显卡拓扑、UEFI、高级 BIOS、电池强效限制和水冷不因界面中出现按钮就自动启用。

## 公开源码与许可

固定 tag 的公开树只有 README、许可、配置指南、图片与 issue 模板，没有可构建的 Rust/前端/HAL/驱动源码。tag 与下载文件的关联也不等于已获得可复现构建证明。README 同时明确有未公开的平台资料。公开 [LICENSE](https://github.com/faintonce/open-revo/blob/bdd22b508efba36368ea0e483608580ad4a90883/LICENSE) 为 MIT；README 另有禁止倒卖表述，不能据此把发布包中的全部组件、嵌入驱动或未公开资产当作已取得相同许可的源码。

本仓库保留独立写作的分析事实与来源，不提交或改写分发提取出的程序、内嵌 UI、机型功耗表、风扇表或驱动代码。

## 未完成的验证

Windows 运行、驱动安装与签名验证、目标设备绑定、IOCTL 数据契约、EC 事务与读回、HID 报告、BIOS/EC 版本差异、挂起恢复和每款型号的实机验收均未完成。尤其没有证明 GM6PX0X 的协议兼容；本报告不改变当前驱动的硬件支持范围。
