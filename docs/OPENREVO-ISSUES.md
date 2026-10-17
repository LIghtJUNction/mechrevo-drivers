# OpenRevo 问题需求与验收清单

本页记录上游公开问题，并把它们转成 CLI 与 Rust 跨平台控制台的验收条件。它不是本项目的修复完成报告。

- 快照时间：2026-10-17T21:31:49Z
- 来源：[GitHub REST open issues](https://api.github.com/repos/faintonce/open-revo/issues?state=open&per_page=100)
- 共 77 个 open issue，排除 PR；已检查分页、重复编号及全量编号集合。
- 机器可读快照：[data/openrevo-issues.json](../data/openrevo-issues.json)
- 证据级别：全部初始为 upstream-report-unverified，resolved_in_this_project 全部为 false。上游用户的复现与根因描述不等于本项目已复现。
- 范围：仅保存标题、来源、分类与简短验收条件，不保存附件、设备原始日志或完整用户正文。

## 优先做的六类要求

1. **准确识别机型和可用能力。** 对应 [#86](https://github.com/faintonce/open-revo/issues/86)、[#104](https://github.com/faintonce/open-revo/issues/104)、[#34](https://github.com/faintonce/open-revo/issues/34)、[#39](https://github.com/faintonce/open-revo/issues/39)、[#36](https://github.com/faintonce/open-revo/issues/36)。型号、主板与后端探测各有依据；相似型号不能串配，未知或冲突型号拒绝写入。缺驱动、无通路、权限不足应显示实际原因，不得静默用模拟值冒充硬件。测试覆盖准确匹配、歧义、未知型号和缺失后端。

2. **先列出修改计划，关闭的功能完全不碰。** 对应 [#60](https://github.com/faintonce/open-revo/issues/60)、[#30](https://github.com/faintonce/open-revo/issues/30)、[#79](https://github.com/faintonce/open-revo/issues/79)、[#92](https://github.com/faintonce/open-revo/issues/92)。CLI和图形控制台共用计划、校验、执行流程；dry-run不得写入，未启用GPU调节时启动与模式切换均不能写GPU参数。不支持的操作在触碰后端前失败；测试单项设置不改变无关功能。

3. **回读真实状态，保留恢复依据。** 对应 [#64](https://github.com/faintonce/open-revo/issues/64)、[#35](https://github.com/faintonce/open-revo/issues/35)、[#80](https://github.com/faintonce/open-revo/issues/80)、[#62](https://github.com/faintonce/open-revo/issues/62)、[#84](https://github.com/faintonce/open-revo/issues/84)、[#93](https://github.com/faintonce/open-revo/issues/93)。写前保留能恢复的原值，写后回读；明确区分用户意图、请求成功、已验证生效、未知与恢复部分失败。测试在中途写失败、读回不一致和恢复失败时准确记录结果；没有原值或真实回读不能宣称完全恢复。

4. **遥测区分设备、来源、单位和不可用状态。** 对应 [#112](https://github.com/faintonce/open-revo/issues/112)、[#52](https://github.com/faintonce/open-revo/issues/52)、[#8](https://github.com/faintonce/open-revo/issues/8)、[#96](https://github.com/faintonce/open-revo/issues/96)、[#67](https://github.com/faintonce/open-revo/issues/67)。每张GPU绑定稳定设备ID，CPU/GPU指标附带来源、单位、采样时间与可用状态；缺失不是0，陈旧值不是新采样，异常值不能直接当真。外部电源在线与电池充放电状态分开表达。测试双GPU枚举变化、单位换算、计数器重置、失联、陈旧与供电证据冲突。

5. **后台查询有边界，硬件故障不冻结界面。** 对应 [#12](https://github.com/faintonce/open-revo/issues/12)、[#77](https://github.com/faintonce/open-revo/issues/77)、[#98](https://github.com/faintonce/open-revo/issues/98)、[#68](https://github.com/faintonce/open-revo/issues/68)、[#71](https://github.com/faintonce/open-revo/issues/71)。硬件探测不在UI线程同步重试；每个提供者至多一个查询，失败退避，可停止。后台降低不必要采样，避免频繁启动系统查询程序及无谓唤醒独显。测试缺驱动快速返回、重复刷新不堆积、退出取消任务和释放单实例资源；CPU、内存和独显活动还需实际测量。

6. **跨平台诊断导出可用，构建说明可复现。** 对应 [#18](https://github.com/faintonce/open-revo/issues/18)、[#27](https://github.com/faintonce/open-revo/issues/27)。CLI支持明确输出路径；默认桌面通过Windows Known Folders或Linux XDG获取，不能硬拼用户目录。报告使用UTF-8并脱敏，成功后返回真实路径；测试重定向桌面、中文空格、目录不存在及无权限。Windows/Linux构建记录要使用真实仓库和现存目录。

## 软件测试与硬件验收边界

模拟后端能证明校验、计划、错误处理、回读比较和恢复顺序正确，不能证明某机型的EC、功耗、风扇、背光、MUX或校色协议可用。编译成功也不能代替安装、加载和实机功能验证。

- 每个真实硬件能力应记录精确机型、BIOS/EC、操作系统、接口、证据与验证级别。不能根据同厂或相似模具猜寄存器、功耗上限、风扇参数或显卡模式。
- [#30](https://github.com/faintonce/open-revo/issues/30) 报告切换iGPU后持续黑屏并无法显示BIOS，具体写入位置和根因未确认。未验证型号的MUX操作应拒绝；不能以模拟测试宣称已修复这台机器。
- [#104](https://github.com/faintonce/open-revo/issues/104) 的WMI和反汇编说明是上游报告。宽匹配GK7前缀、直接EC I/O及免驱描述不能当作已核验写协议；先做精确诊断和独立接口验证。
- [#19](https://github.com/faintonce/open-revo/issues/19) 的420W及其他报告数值是具体用户环境描述，不能成为跨型号默认上限。
- 恢复记录必须保留可核验的原值与失败结果。设备无法回读或无法恢复时，要明确停在未验证或部分恢复状态。

## 历史反馈不能当作当前必现问题

- [#102 最新反馈](https://github.com/faintonce/open-revo/issues/102#issuecomment-6230788005) 表示Fn映射在上游v0.9.1正常。清单保留回归需求，不把它写成当前版本仍必现。
- [#62 后续反馈](https://github.com/faintonce/open-revo/issues/62#issuecomment-5964945722) 表示部分限充已改善，乱码已解决，但重启后限充仍可能失效并需手动开关恢复。乱码、配置迁移和真实限充应分开验收。
- [#62 关于文件名的澄清](https://github.com/faintonce/open-revo/issues/62#issuecomment-5881482030)：open-revo-v0.8.5.7z是v0.8.5的7z压缩包，不能当成v0.8.5.7版本。
- [#12 维护者评论](https://github.com/faintonce/open-revo/issues/12#issuecomment-5827581187) 确认其与#6有关，但指出#7是宝龙达模具识别问题。不能把三个报告归为同一已确认根因。
- [#64 维护者评论](https://github.com/faintonce/open-revo/issues/64#issuecomment-5882950484) 说明校色文件本体未删，丢失的是注册表关联；不能将报告简化成删除ICC文件。

## 状态更新规则

status保留来源及本项目核验状态的区别。初始全部是上游报告，resolved_in_this_project为false。只有在本项目有可定位的实现、成功与失败路径测试、边界验证及适用硬件验收后，才可以按具体范围更新；若仅完成防护或离线测试，应另外注明范围，不把整条硬件问题标为完全解决。

本清单不表示已经提交、推送、发布AUR、安装或在运行中生效。每个后续结论应链接相应提交、测试记录或硬件验收证据。

## 分类统计

| 分类 | 数量 |
| --- | ---: |
| 识别、能力与适配边界 | 7 |
| 功耗、风扇、电池与电源策略 | 24 |
| RGB与背光 | 9 |
| 校色与屏幕 | 6 |
| 恢复与写入副作用 | 5 |
| 遥测、后台性能与生命周期 | 9 |
| 界面、快捷操作、启动、更新、导出与文档 | 17 |

## 全量问题索引

所有条目当前均未在本项目确认为已解决。多主题问题按主要问题分类。

| 上游问题 | 分类 | 本项目验收条件 |
| --- | --- | --- |
| [#115 [Bug]: 机械革命17kscpu功耗锁不住80w只有65w](https://github.com/faintonce/open-revo/issues/115) | 功耗、风扇、电池与电源策略 | 报告目标功耗、实际回读和限制原因；在17KS对应硬件验证80W请求的支持边界，不伪报已锁定。 |
| [#114 [Feature]: 建议把水冷的智能控制调整为不关闭水泵](https://github.com/faintonce/open-revo/issues/114) | 功耗、风扇、电池与电源策略 | 提供水泵固定、风扇独立调节策略；温度在阈值附近波动时不反复启停，实机核对水泵状态。 |
| [#113 [Feature]: 屏幕亮度自定义预设](https://github.com/faintonce/open-revo/issues/113) | 界面、快捷操作、启动、更新、导出与文档 | 支持保存和重新应用自定义亮度；校验范围，保存失败不得显示成功。 |
| [#112 [Bug]: 翼龙15pro 8845hs+4070版本状态总览显示的gpu状态功耗不准](https://github.com/faintonce/open-revo/issues/112) | 遥测、后台性能与生命周期 | 核显与独显以稳定设备ID分别展示功耗等指标；枚举顺序变化不串卡，不混合来源。 |
| [#111 [Bug]: 机械革命钛钽plus 11800h 3060 所有版本充电上限锁不住](https://github.com/faintonce/open-revo/issues/111) | 功耗、风扇、电池与电源策略 | 钛钽PLUS对应型号明确充电阈值支持状态，写后回读并验证真实充电停止；不承诺修复非控制台掉电。 |
| [#110 [Bug]: 无法熄灯](https://github.com/faintonce/open-revo/issues/110) | RGB与背光 | 闲置超时与合盖事件正确关闭背光，活动恢复遵循原设定；在耀世16Ultra实机验收。 |
| [#109 [Feature]: UI字体有点显小了](https://github.com/faintonce/open-revo/issues/109) | 界面、快捷操作、启动、更新、导出与文档 | 支持可读字体缩放；在27寸2K与不同DPI下核对文字、按钮和布局。 |
| [#108 [Bug]: 24蛟龙16pro风扇控制异常](https://github.com/faintonce/open-revo/issues/108) | 功耗、风扇、电池与电源策略 | 风扇曲线及手动升降均有回读与状态依据；验证CPU/GPU双通道，明确固件保护下限。 |
| [#107 [Bug]: 迷你窗口布局有问题 下面的两个按钮显示不出来](https://github.com/faintonce/open-revo/issues/107) | 界面、快捷操作、启动、更新、导出与文档 | 小屏、DPI变化和屏幕底边场景下内容可滚动，底部按钮可见可操作；溢出依据实际视口。 |
| [#106 [Bug]: 耀世15pro i9-13900hx4060睡眠唤醒后cpu/gpu功率异常](https://github.com/faintonce/open-revo/issues/106) | 功耗、风扇、电池与电源策略 | 睡眠唤醒后重新探测能力和真实功耗状态；恢复失败可见，在对应耀世15Pro重复睡眠负载验收。 |
| [#105 [Feature]:解除离电限制开启下拔电后可自定义模式切换的建议](https://github.com/faintonce/open-revo/issues/105) | 功耗、风扇、电池与电源策略 | 允许显式配置离电后的性能模式；AC断开和恢复按策略切换，未知供电状态不盲目提高功耗。 |
| [#104 [Bug]: MECHREVO 深海泰坦 X3-S (GK7MR0R, 20系/RTX2060) 无法匹配任何机型清单，永久落入 MockHAL (SAFE MOCK)](https://github.com/faintonce/open-revo/issues/104) | 识别、能力与适配边界 | 明确GK7MR0R的匹配和后端探测原因；没有通路时显示不可用而非真实模拟值，不按前缀推断写协议。 |
| [#103 [Bug]: 耀世16pro没有A面灯效设置](https://github.com/faintonce/open-revo/issues/103) | RGB与背光 | 耀世16Pro2024的A面灯能力来自独立证据；有支持则可设置并回读，无支持明确说明。 |
| [#102 [Bug]: Fn 功能键错位（v0.9.1 功能已正常）](https://github.com/faintonce/open-revo/issues/102) | 界面、快捷操作、启动、更新、导出与文档 | 保留无界14XA逐项Fn键映射回归；区分0.8.8/0.8.9历史报告与上游0.9.1已恢复反馈。 |
| [#100 [Bug]: 校色文件异常](https://github.com/faintonce/open-revo/issues/100) | 校色与屏幕 | 校色变更可恢复，视频及桌面显示分别核对；没有对应屏幕证据不提供假定校色文件。 |
| [#99 键盘灯光控制问题](https://github.com/faintonce/open-revo/issues/99) | RGB与背光 | 灯效设置立即生效且亮度调整不是必要条件；拔插电源不意外重置用户灯效，逐效果实机验收。 |
| [#98 [Bug]: cpu占用率异常](https://github.com/faintonce/open-revo/issues/98) | 遥测、后台性能与生命周期 | 后台降低采样，避免无必要唤醒独显；记录CPU/内存和GPU活动并验证界面切换响应。 |
| [#97 [Feature]: 希望添加快捷键](https://github.com/faintonce/open-revo/issues/97) | 界面、快捷操作、启动、更新、导出与文档 | 可绑定模式及面板快捷键，冲突和注册失败明确显示；重启后配置保留。 |
| [#96 [Bug]: 用100W PD充电时电源管理界面显示正在使用电池供电](https://github.com/faintonce/open-revo/issues/96) | 功耗、风扇、电池与电源策略 | 外部电源在线与电池充放电分开报告；100W PD证据冲突显示未知，不直接误报仅电池供电。 |
| [#94 [Bug]: 睡眠守护中忽略系统所需策略的开关貌似无效](https://github.com/faintonce/open-revo/issues/94) | 功耗、风扇、电池与电源策略 | 对当前电源计划的AC/DC目标项进行读改回读，开关语义一致，失败不显示已生效。 |
| [#93 [Bug]: 24款翼龙15pro性能模式切换无效](https://github.com/faintonce/open-revo/issues/93) | 功耗、风扇、电池与电源策略 | 模式切换后读取实际硬件状态；对应翼龙15Pro验证各档负载表现，不以UI标签代替生效。 |
| [#92 [Bug]:蛟龙16pro 锁定充电上限导致性能模式灯效错乱](https://github.com/faintonce/open-revo/issues/92) | 功耗、风扇、电池与电源策略 | 充电阈值操作不生成性能指示灯写入；对应蛟龙16Pro核对模式与灯色映射。 |
| [#91 [Bug]: 键盘灯光不熄灭](https://github.com/faintonce/open-revo/issues/91) | RGB与背光 | 闲置超时后背光真实熄灭且活动后恢复；在耀世16Pro与Windows10实机验证。 |
| [#88 [Feature]: 添加风扇转速同步功能](https://github.com/faintonce/open-revo/issues/88) | 功耗、风扇、电池与电源策略 | 风扇同步模式有明确曲线选择及双通道输出；独立与同步切换不遗留旧目标。 |
| [#86 [Bug]: 无界14Pro会被辨识成蛟龙16Pro](https://github.com/faintonce/open-revo/issues/86) | 识别、能力与适配边界 | 无界14Pro7840HS不匹配蛟龙16Pro；歧义匹配拒绝写入，展示识别依据与能力。 |
| [#85 [Bug]: 来电自动开机失效](https://github.com/faintonce/open-revo/issues/85) | 功耗、风扇、电池与电源策略 | AC自动开机写前保留可恢复原值并回读；恢复官方控制后状态可核验，不猜测EC位。 |
| [#84 [Bug]:重启或者休眠回来后屏幕色彩校正就会失效，但是app里开关还是处于打开状态](https://github.com/faintonce/open-revo/issues/84) | 校色与屏幕 | 重启和睡眠后区分用户意图与实际校色状态；恢复失败明确显示而非只保留打开开关。 |
| [#82 [Bug]: 键盘关闭背光闪烁情况](https://github.com/faintonce/open-revo/issues/82) | RGB与背光 | 关闭背光后不被后台刷新意外重开；记录命令序列并在耀世16Pro反复观察。 |
| [#81 [Bug]: 状态灯不对，Fn+F5无法切换触控板](https://github.com/faintonce/open-revo/issues/81) | 功耗、风扇、电池与电源策略 | 26款蛟龙16Pro的性能灯及Fn触摸板映射按型号验证，操作失败有可见状态。 |
| [#80 [Feature]: 使用软件后出现异常问题，如何退回？](https://github.com/faintonce/open-revo/issues/80) | 恢复与写入副作用 | 提供变更记录与可恢复原值；恢复结果逐项回读，未知或恢复失败不得声称完全退回。 |
| [#79 [Bug]: 切换屏幕刷新率会自动打开屏幕色彩校正](https://github.com/faintonce/open-revo/issues/79) | 校色与屏幕 | 刷新率修改不触发未启用的校色操作；校色开关与实际状态一致，单项变更可回归。 |
| [#78 [Bug]: 高负载时键盘灯光闪烁](https://github.com/faintonce/open-revo/issues/78) | RGB与背光 | 高负载下灯光更新不竞争硬件访问且不意外熄灭；以压力下命令时序与实机录像核验。 |
| [#77 [Feature]: 重构后台查询机制并尽快开放源码](https://github.com/faintonce/open-revo/issues/77) | 遥测、后台性能与生命周期 | 后台查询使用适合常驻的API，单提供者最多一个查询，失败退避并可取消，避免高频外部进程。 |
| [#76 [Bug]: RGB灯光欢迎模式设置不了](https://github.com/faintonce/open-revo/issues/76) | RGB与背光 | 欢迎灯效保存、预览与重启后的实际效果一致；保存失败不得保留成功状态。 |
| [#75 [Bug]: Intel 平台 PPM 开启时，混合显卡模式最高睿频被限制为小核频率](https://github.com/faintonce/open-revo/issues/75) | 功耗、风扇、电池与电源策略 | Intel大小核睿频上限分别读取并明确单位；PPM在混合和独显模式下不误用小核上限。 |
| [#73 [Bug]: 调整为均衡/狂暴/自定义后，CPU异常高温高频](https://github.com/faintonce/open-revo/issues/73) | 功耗、风扇、电池与电源策略 | 空闲与负载分别验证各性能模式的系统电源策略及频率；不将报告中未复现的根因当结论。 |
| [#72 [Bug]: 无法开机自启](https://github.com/faintonce/open-revo/issues/72) | 界面、快捷操作、启动、更新、导出与文档 | 自启动设置后读取真实注册状态，重启登录核验；权限或任务创建失败明确显示。 |
| [#71 [Bug]: 托盘退出后无可见主进程，但替换新版文件后双击无法直接启动](https://github.com/faintonce/open-revo/issues/71) | 遥测、后台性能与生命周期 | 退出取消后台任务并释放句柄及单实例锁；替换文件后可直接启动，不残留阻塞子进程。 |
| [#70 [Feature]: 增加更新功能](https://github.com/faintonce/open-revo/issues/70) | 界面、快捷操作、启动、更新、导出与文档 | 更新检查区分网络错误、无更新和新版本；下载校验与原子替换可恢复，版本比较不混淆压缩包扩展名。 |
| [#69 [BUG]：灯光问题](https://github.com/faintonce/open-revo/issues/69) | RGB与背光 | 欢迎模式、Logo颜色及海岸灯带效果分通道验证，保存与实际回读一致，不假定灯区通用。 |
| [#68 [Feature]: 后台webview占用资源太多了，希望后台时能自动杀掉webview进程](https://github.com/faintonce/open-revo/issues/68) | 遥测、后台性能与生命周期 | 隐藏界面降低渲染和采样开销；恢复窗口仍能获取最新状态，测量后台内存与CPU。 |
| [#67 [Bug]: 锁充电上限、屏幕色彩切换、A面logo灯关闭均失效](https://github.com/faintonce/open-revo/issues/67) | 功耗、风扇、电池与电源策略 | 分别核验PD在线、充电阈值、校色及Logo能力；逐项回读，不用一个成功标志覆盖多项失败。 |
| [#66 [Bug]: 高性能电源模式切换异常](https://github.com/faintonce/open-revo/issues/66) | 功耗、风扇、电池与电源策略 | 自定义模式的Windows电源策略与用户选择和提示一致；逐模式测试，关闭功能时不改电源方案。 |
| [#64 [Bug]: 24款翼龙15pro官方控制台无法重新接管屏幕校色和电池充电上限](https://github.com/faintonce/open-revo/issues/64) | 恢复与写入副作用 | 接管前保留颜色关联和充电设置；恢复后由官方控制台接管，并验证关联及充电状态。 |
| [#63 [Feature]: 电池循环次数.电源推荐方案及非透明UI](https://github.com/faintonce/open-revo/issues/63) | 界面、快捷操作、启动、更新、导出与文档 | 循环次数缺失显示不可用，支持可读非透明外观；离电策略与Windows电源模式关系明确。 |
| [#62 [Bug]: 更新open-revo-v0.8.6.2后充电上限功能失效与存在文字显示bug](https://github.com/faintonce/open-revo/issues/62) | 功耗、风扇、电池与电源策略 | 重启后重新验证实际充电阈值，配置迁移与失败恢复可测试；乱码和充电问题分别跟踪历史反馈。 |
| [#61 [Feature]: 屏幕色彩增强的管理功能](https://github.com/faintonce/open-revo/issues/61) | 校色与屏幕 | 色彩方案支持导入导出、删除和重命名，非法文件和同名冲突安全失败且保留现有方案。 |
| [#60 [feature]: 显卡设置默认覆盖msi afterbunner。能否默认不开启显卡设置，尤其是在应用未开启任何显卡设置的条件下](https://github.com/faintonce/open-revo/issues/60) | 恢复与写入副作用 | 关闭GPU调节时启动及模式切换均不写GPU参数；第三方调节保持不变，以后端命令记录核验。 |
| [#58 [bug]: 狂暴模式与官方狂暴模式性能释放差异](https://github.com/faintonce/open-revo/issues/58) | 功耗、风扇、电池与电源策略 | 对应极光Pro在相同供电、散热与负载下比较官方和本项目模式；记录实际限制与回读，不猜功耗值。 |
| [#57 [Feature]: 监控悬浮窗 和 快捷键](https://github.com/faintonce/open-revo/issues/57) | 界面、快捷操作、启动、更新、导出与文档 | 面板显示带单位的CPU/GPU即时功耗，快捷键可配置；不可用读数不显示零。 |
| [#56 [Bug]: 翼龙15Pro2024是双显三模被识别为双显双模](https://github.com/faintonce/open-revo/issues/56) | 识别、能力与适配边界 | 显卡模式列表来自精确型号的已验证能力；未验证iGPU模式不开放危险切换。 |
| [#52 [Bug]: 显卡功耗读取异常](https://github.com/faintonce/open-revo/issues/52) | 遥测、后台性能与生命周期 | 按设备与来源验证GPU功耗单位、可用性和有效边界；异常值不冒充真实功耗，NVAPI/NVML不串来源。 |
| [#51 [Feature]: 性能监控 悬浮面板](https://github.com/faintonce/open-revo/issues/51) | 界面、快捷操作、启动、更新、导出与文档 | 性能面板快捷键可配置并处理冲突；指标绑定对应设备且采样不可用时明确显示。 |
| [#49 [Feature]: 锁定充电上限数额回退](https://github.com/faintonce/open-revo/issues/49) | 功耗、风扇、电池与电源策略 | 充电上限与主动放电能力分开；不支持主动放电时明确说明，不将阈值设置伪装成放电功能。 |
| [#48 [Feature]: 全屏模式CUP信息显示不完整+缺少快捷开关+海岸灯条不工作](https://github.com/faintonce/open-revo/issues/48) | 界面、快捷操作、启动、更新、导出与文档 | 长CPU名称不截断；触摸板切换键与触摸板状态独立，海岸灯条先确认硬件连接与能力。 |
| [#46 [Bug] 校色状态“未校准”的判断与 Windows 系统色彩管理不明确](https://github.com/faintonce/open-revo/issues/46) | 校色与屏幕 | 分别显示配置文件安装、显示器关联、曲线加载和本项目开关状态，提供由系统管理及恢复路径。 |
| [#45 [Feature]: 选择性禁用 自定义 调度模式](https://github.com/faintonce/open-revo/issues/45) | 功耗、风扇、电池与电源策略 | 可从模式循环排除不支持或未启用的自定义方案；按键循环不重新启用已排除方案。 |
| [#44 [Bug]: miniled控光算法导致的大面积光晕](https://github.com/faintonce/open-revo/issues/44) | 校色与屏幕 | MiniLED算法与面板能力分开验证，以相同测试图对比实际光晕；未验证面板不写控制参数。 |
| [#39 [Bug]: 无界14X 8845HS 功能异常](https://github.com/faintonce/open-revo/issues/39) | 识别、能力与适配边界 | 无界14XA按准确型号提供能力；CPU/GPU读数、风扇通道、功耗支持与界面型号分别验证。 |
| [#36 [Docs]: 文档中明确可支持的型号和功能边界](https://github.com/faintonce/open-revo/issues/36) | 识别、能力与适配边界 | 支持清单按型号、功能、操作系统及验证级别记录，未验证或不支持功能可见且不可误操作。 |
| [#35 [Bug]: 无法恢复官方控制台](https://github.com/faintonce/open-revo/issues/35) | 恢复与写入副作用 | 保存接管前服务及配置状态；恢复官方控制台逐步验证，失败保留恢复材料和准确原因。 |
| [#34 [Bug]: 无界15X 8845HS 只能切换办公/均衡模式，Fn等功能映射异常](https://github.com/faintonce/open-revo/issues/34) | 识别、能力与适配边界 | 无界15XA的背光档位、灯区、Fn及温度字段按独立证据建模；PD模式限制明确来源，不猜字段单位。 |
| [#30 [Bug]: 30系机械革命切换核显模式黑屏，无法进入Windows/bios](https://github.com/faintonce/open-revo/issues/30) | 恢复与写入副作用 | 未验证的GPU/MUX切换在精确型号能力校验前拒绝；保留恢复依据，不能以模拟测试证明黑屏根因已修复。 |
| [#29 [Bug]: 电源重新断开后重新插入键盘灯光自动切换](https://github.com/faintonce/open-revo/issues/29) | RGB与背光 | 电源状态变化保留明确选择的灯效；只在策略显式要求时切换，并回读实际效果。 |
| [#28 [Feature]: 为40系机型增加狂暴模式显卡OC开关](https://github.com/faintonce/open-revo/issues/28) | 功耗、风扇、电池与电源策略 | GPU超频能力及范围按具体型号验证；未启用时零写入，退出模式按已记录原值恢复。 |
| [#27 [Bug]: README.md 开发版本部署 ISSUE](https://github.com/faintonce/open-revo/issues/27) | 界面、快捷操作、启动、更新、导出与文档 | 文档使用真实仓库和存在的目录；Windows/Linux从干净检出按记录命令构建并验证。 |
| [#24 [Feature]: 性能模式的更多快捷按钮](https://github.com/faintonce/open-revo/issues/24) | 界面、快捷操作、启动、更新、导出与文档 | 用户自定义方案可直接选择并保持顺序；无效或不支持方案显示原因而非假成功。 |
| [#22 [Feature]: 换个更好看的图标](https://github.com/faintonce/open-revo/issues/22) | 界面、快捷操作、启动、更新、导出与文档 | 图标在托盘、高DPI及深浅背景下可辨认，透明背景与尺寸资源正确。 |
| [#20 [Feature]: 记住窗口显示模式](https://github.com/faintonce/open-revo/issues/20) | 界面、快捷操作、启动、更新、导出与文档 | 显示模式持久化并在重新启动后恢复；损坏配置有明确回退且不影响硬件设置。 |
| [#19 [Bug]:数据采集问题PL4功耗低很多导致CPU性能释放低](https://github.com/faintonce/open-revo/issues/19) | 功耗、风扇、电池与电源策略 | PL1/PL2/PL4的单位及范围各有型号证据，回读不混淆字段；不直接采纳报告中的420W为通用值。 |
| [#18 [Bug]: 使用机型数据收集专用版一键导出至桌面失败](https://github.com/faintonce/open-revo/issues/18) | 界面、快捷操作、启动、更新、导出与文档 | 报告导出支持显式路径及系统桌面位置，覆盖重定向目录、中文空格和权限失败，不伪报成功。 |
| [#12 [Bug/RFC]: 卸载官方控制台后 \.\ACPIDriver 缺失导致 UI 主线程死锁（托盘无响应根因分析与纯净自愈方案）](https://github.com/faintonce/open-revo/issues/12) | 遥测、后台性能与生命周期 | 缺失ACPI驱动快速报告不可用，探测不阻塞UI，无无限同步重试；安装驱动不是普通探测的隐式副作用。 |
| [#8 [Bug]: 控制台CPU负载和频率与实际数据差异巨大](https://github.com/faintonce/open-revo/issues/8) | 遥测、后台性能与生命周期 | CPU负载与频率明确采样来源、时间和单位；计数器重置或缺失不产生伪数据。 |
| [#7 [Bug]: 24年蛟龙16pro 7745 4070 无法使用](https://github.com/faintonce/open-revo/issues/7) | 识别、能力与适配边界 | 正确辨别宝龙达蛟龙16Pro及后端能力；不把其无法使用直接归为与缺驱动死锁相同根因。 |
| [#6 [Bug]: 前端页面渲染丢失](https://github.com/faintonce/open-revo/issues/6) | 遥测、后台性能与生命周期 | 快速重复开关托盘窗口不丢失渲染；硬件探测失败时窗口仍可操作并显示错误。 |
| [#5 [Bug]: 自定义方案CPU始终跑满限制的功耗](https://github.com/faintonce/open-revo/issues/5) | 功耗、风扇、电池与电源策略 | 自定义模式的功耗上限不变成强制最低负载；空闲和负载分别核验电源策略，不凭报告猜根因。 |
| [#3 [Feature]: 多个自定义方案，可以在托盘右键切换](https://github.com/faintonce/open-revo/issues/3) | 界面、快捷操作、启动、更新、导出与文档 | 自定义方案可在快捷入口直接切换并隐藏不用的默认方案；持久化且验证切换后的实际状态。 |
