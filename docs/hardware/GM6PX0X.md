# 旷世 16 Pro GM6PX0X 本机验证

日期：2026-10-14；2026-10-15 续验（Asia/Taipei）。这是单台机器的部分功能验证，不是完整兼容认证。

## 2026-10-15 功耗工具状态检查加固（用户态已生效）

独立工具已从 `mechrevo-power-control 0.1.0-1` 更新为 `0.1.0-2`，
包内源码冻结于 `8578d5f9d28a27af2ee5354670cf9616a2c65e78`。
四个已安装文件的内容、root 所有者和权限均已核对；包完整性检查为 16 个条目、0 个改动，
原 `thermald` 包为 22 个条目、0 个改动。没有更新 DKMS 包或发布 AUR。

已复现损坏应用收据中的字符串 `"false"` 被当作成功、空数组事务被当作未配置事务，
以及缺少嵌套快照字段或错误收据类型导致未处理异常的问题。
现在加载状态时检查事务所有者、功耗快照、哈希及应用收据的类型和一致性；
损坏状态阻止修改命令，保留原文件和运行中的温控，不借修复之名删除状态。
`status` 发现错误仍输出 JSON，但退出码为 1；正常的未配置状态保持退出码 0。

最终 **465 项本地测试通过**。新增 5 项回归，其中损坏状态测试覆盖 36 组变体及
6 组无效事务所有者，并检查无功耗写入、无服务重启、无文件覆盖；
另覆盖有效状态、失败收据、暂未应用的状态及 CLI 退出码。
新增回归在旧源码上曾产生 26 次失败和 2 次未处理异常，最终源码及配方通过完整回归。
真实本地归档构建、源码冻结核对和配方 ShellCheck 均通过。

安装前后真实 `status` 均无错误，MSR 与 MMIO 的 PL1 当次回读均为 55 W。
受管状态/XML/drop-in、thermald 偏好和 feature 文件、原服务单元、TCC settings 的内容及权限保持；
四个驱动的运行 build ID 在本轮安装前后保持，Fn 锁、充电档位、背光/RGB/cTGP 读值保持。
前置快照误写 GPU 模块名，未捕获该项；随后用真实模块名 `tuxedo_nb02_nvidia_power_ctrl`
补查，其 build ID 与此前已核验记录相同，没有把缺失值当作前后验证。
`thermald`、`tccd`、`nvidia-powerd`、`power-profiles-daemon` 的 PID、状态和启动命令保持。
安装没有重启这些服务、重载模块、睡眠或重启机器，也没有再次进行真实 set/apply/clear。
本轮验证的是已有配置兼容与状态读取；不能据此扩大持续负载、睡眠恢复及其他型号的验收范围。

旧工具包与当前已安装旧文件逐字节核对后，恢复材料保存并校验于
`~/Backups/projects/mechrevo-drivers/light/20261015-power-state-validation/`，包含旧包、
带内容/权限/所有者的安装前快照、校验清单及 `RESTORE.txt`。
包回滚仅重新安装该旧工具包，不默认重启 thermald 或覆盖当前状态。
证据和新包保留在 `.local-tests/state-validation-20261015/`，不上传原始机器记录。
本轮该目录约 4.3 MiB，其中构建工作区约 1.4 MiB 为临时产物，未擅自清理旧证据。

文件跟踪保留早期包装器超时、数据库锁冲突，以及测试短命文件/打包权限造成的缺口；
后续使用独立状态目录并确认基线就绪。安装跟踪覆盖工具、包数据库、日志及功耗状态目录；
安装钩子的 `/usr` 目录时间戳及 PackageKit IPC 所涉范围外状态不在该跟踪内。
未把最终扫描或成功安装写成完整的文件写入历史。

## 2026-10-15 RAPL 用户上限与原生温控协调

独立用户态工具 `mechrevo-power-control 0.1.0-1` 已在本机安装，源码冻结于
`124a440bbad69643fd6752b90db6e2832ac61884`；实际包内四个文件与该提交逐字节一致，
包完整性检查为 16 个条目、0 个改动。它没有加入现有 DKMS 包或发布到 AUR。
本轮 460 项完整本地测试通过，其中新增 62 项 CLI 行为回归、11 项实际 thermald C++
控制链回归和 6 项打包回归。冻结源码在测试前后哈希一致，未用较早的变化中测试代替最终验收。

本机原有 `thermald 2:2.5.13-1.1` 二进制保持，22 个包文件完整性核对通过。
工具严格核对本机 DMI、BIOS `N.1.09MRO07`、i9-12900HX、发行版二进制哈希、
有效且已启用的两套 RAPL 长期约束，并拒绝 PPCC、自适应平台、外来配置及会触发
额外 TCC offset workaround 的情况。当前没有这些固件接口，原服务原本走 default 回退路径；
启用工具时明确使用该原生 default engine 与独立 XML，保留默认温控区及原温度阈值。

真实 `set 55` 将 MSR 和 MMIO 两套 PL1 下调到 55 W。
随后 90.44 秒、180 次样本中，MSR 为 34、41、48、55 W，MMIO 保持 55 W，
没有任何样本超过请求上限。独立内核跟踪捕获同一 thermald 进程的 44 次长期限值写入，
请求值同为 34、41、48、55 W，44 次返回全部成功。
这同时验证了本次观察内的上限保持、温控继续下调，以及降温约束放松后回到 55 W；
不是靠后台循环与温控反复抢写。临时探针和 trace instance 已撤除。

在较低温度窗口还执行了真实 `clear`：受管 XML、drop-in 和状态撤销，原始
`--adaptive` 服务命令恢复且服务 active，两套限值回读均为 140 W。
这是交回原生策略后的当次读值，不是工具主动写高功耗或保证该值以后固定。
随后重新应用 55 W 成功。受管配置下另一次直接重启 thermald，启动回调生成新的成功收据，
两套实际 PL1 再次不超过 55 W；服务退出到启动回调完成期间不承诺严格保持上限。
这次服务重启后的 60.35 秒、120 次样本也通过：MSR 仍为 34、41、48、55 W，MMIO 为 55 W。

五个运行模块 build ID、TCC settings 字节、cTGP、背光/RGB/Fn 锁、原内核温控 trip 值，
以及 tccd、power-profiles-daemon、nvidia-powerd 进程均保持。
PL2、peak 约束及 enabled 的采样值保持；长期时间窗口仍由 thermald 原生状态机调整。
XML 匹配使 thermald 创建偏好文件和 `.save`，偏好值保持原来的 ENERGY_CONSERVE，
没有宣称这些运行文件的存在性或字节完全不变。
没有重载驱动、重启 TCC、睡眠、重启机器或增加压测工作进程。

重要限制：现有任务下 90 秒 CPU package 能量差分均值约 39.31 W，低于 55 W，
因此没有证明持续满载下的平均功耗执行效果。温度采样为 86–100°C，曾达到本机
100°C 临界参考值；没有据此宣称温度下降、散热改善或长期稳定性已验收。
55 W 以外的设置、整机重启、睡眠恢复、不同内核、BIOS 和其他型号仍未实机验收。
CPU RAPL 不代替尚未开放的 EC TDP，也不解决 GPU `0xC5` 问题。

用法及撤销、中断恢复见 [功耗控制说明](../POWER-CONTROL.md)。
证据保留在 `.local-tests/rapl-fix-20261015/` 的完整测试、实际包、`held55_first.json`、
`first-hardware-summary.json`、`thermal-writer-identities.json`、`clear-reapply.json`
及 `restart55.json`、`held55_after_restart.json`、`restart-hardware-summary.json`。
每次配置事务的恢复材料统一保留在
`~/Backups/projects/mechrevo-drivers/light/rapl-power-control/`，原始机器日志不上传。

## 2026-10-15 Intel RAPL 临时降限试验

本轮已安装包为 `4.24.0.r30.g85bf136-1`，五个运行模块仍保持 IO r23、其余四个 r10。
本机 CPU 是 Intel Core i9-12900HX，Linux 同时暴露 MSR 和 MMIO 的 package-0 RAPL 接口。
这条标准 CPU 功耗控制路径与尚未适配的 MECHREVO EC TDP 接口不同。

第一轮预检观察到长期限值在 119、126、133、140 W 之间动态变化，因此没有执行写入。
第二轮在短暂稳定窗口，只向 MSR 接口的 `constraint_0_power_limit_uw` 写入一次 55,000,000，
从 140 W 下调为 55 W，并立即读回相同值。
约 0.255 秒后，该限值变为 133 W，时间窗口从约 55.97 秒变为约 1 秒，脚本停止，没有反复抢写。
外部控制接管后没有强行恢复更高限值；结束时回读的两套控制值均与 140 W 基线一致。
这不是本脚本主动恢复成功或长期固定限值的证明，之后仍可能被温控服务动态调整。

随后独立的六秒内核跟踪捕获 `thermald` 成功写入长期限值 126 W 和 133 W，返回值为 0。
这与前述覆盖模式吻合；跟踪发生在试验之后，没有直接捕获那一次覆盖事件。
临时探针和 trace instance 均已撤除，没有停止或重启温控服务。

55 W 来自本机长期约束的热设计参考值；[内核实现](https://github.com/torvalds/linux/blob/master/drivers/powercap/intel_rapl_common.c)
将该项映射到 `THERMAL_SPEC_POWER`，不能将它当作已核实的整机最大安全功耗。
试验前现有任务的 CPU package 能量差分采样均值约 40.34 W，没有另加压测；
低限值保持时间又很短，因此不能宣称已经验证满载限功耗、温度改善或稳定的手动调节。
sysfs 没有原子比较后写入的所有权机制，不能完全排除其他控制器写入相同值的竞争。

没有请求修改 PL2、时间窗口、enabled、MMIO、EC、GPU 或风扇。
五个运行模块 build ID、TCC 配置字节、cTGP、背光/RGB/Fn 锁及四个服务进程均保持。
没有睡眠、重启或模块重载；内核写 RAPL 时可能同时调整功耗通知处理，未声称全部硬件寄存器逐字节恢复。
接下来需要协调 RAPL 用户设置与 `thermald`，保留温控保护，再验证限值保持、恢复和负载效果。
EC 地址、单位和整机范围仍缺少本型号依据，没有据此开放 EC TDP 白名单。

原始记录保留在 `.local-tests/rapl-20261015/summary.json`、两轮 `probe-*/receipt.json`
和 `writer-trace.json`，不上传原始机器日志。
写入前控制快照已核对并保留于统一 Backups 目录的 `light/20261015-rapl-intel-rapl-0/`。

## 2026-10-15 probe、PM、Clevo 与 LED 修正（r23，部分生效）

已安装并发布 AUR `4.24.0.r23.gf1aa652-1`，包源码 `f1aa652`；功能提交最终为 `2c749ac`。
281 项本地测试通过，两个内核各重新编译全部 27 模块；包内隔离 DKMS 另通过 54 模块构建。
系统 DKMS 为两套内核编译、签名并安装 54 模块；包完整性、UKI 与引导器签名核对通过，未重启。
安装 hook 保留此前已有的 `xhci_pci_renesas` 缺少固件警告，没有隐藏日志或额外改动该驱动。

只精确重载 `tuxedo_io`，其运行 build ID 与新磁盘模块一致。
keyboard、WMI、兼容检查和 GPU 电源模块的运行 build ID 保持 r10；
新 probe 错误回退、适配器/PM 门禁、背光阻塞回调、睡眠队列及重绑修复仍未运行。
Arch 未启用 `CONFIG_MODVERSIONS`；重载前另核对共享头文件、十个项目接口的签名和导出、
新 IO 源码、导入符号和依赖均与已审版本一致，不能用未启用的 CRC 检查代替这项核对。

本机 profile 数量仍为 0，合法但不支持的模式返回 `ENODEV`，非法枚举返回 `EINVAL`，空指针返回 `EFAULT`。
无效风扇参数、九项不支持 TDP 读取及三项写入拒绝通过；失败读取的缓冲区保持不变。
Clevo 识别为不存在；空接口字符串、空指针和六项失败读取检查通过，未执行有效 Clevo 固件写入。
三次样本均为 CPU 95°C、GPU 55°C、双风扇原始值 170，不是 RPM 或散热改善证据。

cTGP 前后均为 25，原背光/RGB/Fn 锁读值保持；`tccd`、`thermald`、`nvidia-powerd` 均 active。
安装前后 `/etc/tcc/settings` 的 SHA-256 完全一致。
这些旧 keyboard 控件仅读取用于检查状态保持，不能当作新 LED 修复的硬件验收。
没有触发闪烁、物理按键、手动风扇、睡眠或重启，长期负载、电池及其他型号也未验收。
NVIDIA `0xC5` 仍未修复，GM6PX0X TDP 仍缺少规范；没有据此启用猜测性写入。

证据在 `.local-tests/next-20261015/` 的 `main-validation/receipt-final.json`、
`package-validation/receipt.json`、`system-install/receipt.json`、`live-runtime/output/receipt.json`
及 `postinstall/receipt.json`。r16 包、54 模块和原 TCC 配置已保存并校验于
`~/Backups/projects/mechrevo-drivers/light/20261015-next-r16/`。
跟踪保留事件队列溢出、临时 FIFO 等缺口；安装阶段使用轮询，Limine 的短命 `/tmp` 目录不在选定根目录内。
不把最终扫描当作完整历史，未额外清理。

## 2026-10-15 能力、序列及 EC 错误修正（r16，部分生效）

已发布 AUR 并安装 `4.24.0.r16.gb549e06-1`，包源码 `b549e06`，三项功能修复为
`8ab40fb`、`6a924ac`、`165434a`。包内源文件与该提交逐字节一致，包完整性检查通过。
两套内核的 27 个模块均经系统 DKMS 编译、签名和安装；UKI 与引导器签名检查通过。
这不代表已完成重启或启动验收。

本轮仅用 `rmmod tuxedo_io` 精确卸载 IO 后重新加载，新 IO 的 GNU build ID 与磁盘一致。
keyboard、WMI、兼容检查和 GPU 电源模块的运行 build ID 均保持 r10；
绑定、按键任务、序列和 custom-profile 修复尚未在运行中生效。
不通过旧 keyboard 的已知卸载问题路径强行重载，按用户安排留待以后重启。

本机公开 profile 数量为 0；向三个合法枚举写入均返回 `ENODEV`，
0、4、`UINT32_MAX` 返回 `EINVAL`，空指针返回 `EFAULT`。
拒绝前不访问 EC 的依据是实际 C 回归和已匹配的源码，没有声称做过实时 EC 探针。
两只风扇的超范围输入拒绝、九项不支持 TDP 读取及三项写入拒绝通过；
三次样本为 CPU 95°C、GPU 54°C、风扇原始值 170，不是 RPM 或散热改善证据。
cTGP 前后均为 25，背光/RGB/Fn 锁值保持，`tccd`、`thermald`、`nvidia-powerd` 均 active。
仅读取这些旧 keyboard 控件用于核对保持状态，不作为新修复的硬件验收。

新增修正按机型声明的能力限制公开性能模式，避免接受不存在的档位；
合法 custom TDP 的内部 OVERBOOST 和 raw MODE ABI 保持。
过滤器移除后及重新安装前清零半截触控板按键序列，避免跨重绑漏报或误排任务。
custom-profile 的读取/reset/enable 任一步失败后停止依赖操作，保留错误并继续其他初始化；
GM6PX0X 仍未启用该能力，没有据此开放本机 TDP 或写入新寄存器。

完整 **201 项本地测试通过**。新增能力测试检查全部 16 组标志组合；
8 项完整实际 probe/filter/remove 测试各编译四组 API 宏，初始化服务由模拟实现；
16 项 custom-profile 回归覆盖逐项 EC 失败和全部初始寄存器字节。
同一测试运行旧源码能分别捕获原错误；这些不是实机 EC 故障注入或物理按键验收。
源代码与日志保存在 `.local-tests/remaining-20261015/`。

安装前已保存并独立解包校验 r10 包和两套内核的 54 个项目模块，路径为
`~/Backups/projects/mechrevo-drivers/light/20261015-remaining-r10/`。
本轮按用户安排不睡眠、不重启；运行中的模块与磁盘包需分别核对。

## 2026-10-15 绑定失败修正（源码验证，未安装）

本节此前验证时安装包为 `4.24.0.r10.gc79a268-1`。当时修正尚未安装、加载或进行实机故障注入，
不代表运行中的模块已经包含这些改动。

模拟 EC 错误可使风扇启动和键盘探测失败；旧外层绑定仍报告成功，随后解绑可能访问空驱动。
修正将失败传回 WMI probe，回滚 transport 和事件回调，并处理重复解绑、错误接口解绑、
其他驱动占用和输入设备初始化失败后的残留指针。事件回调在成功初始化后才发布，解绑前清除。

另外修正触摸板辅助按键任务的生命周期：卸载先停止本驱动安装的 i8042 过滤器并同步排空任务，
再清空、释放输入设备；直接平台解绑也经过同一入口。输入设备注册失败时，任务不再访问空指针。
这解决已排队或正在运行的任务在卸载后继续访问输入设备的问题，不能只靠空指针检查替代排空。

新增回归提取真实绑定函数和风扇启动路径，模拟平台、输入设备及 EC。
本维护分支 **170 项本地测试通过**，其中 13 项绑定回归、11 项按键任务生命周期回归；
后者使用 pthread 条件变量固定排队、运行和卸载的交错，不是内核调度压力测试。
绑定修正前的维护源码在 13 项中失败 11 项；已修绑定但尚未修按键任务的源码在 11 项中
失败 9 项、通过 1 项、跳过尚不存在的并行准备入口用例 1 项。
最终维护源码独立通过 `7.2.9-arch1-1` headers 的全部 27 个模块编译，未安装或加载。
维护分支继续基于 `4.24.0`，独立运行全量测试并编译；上游贡献分支基于 `4.25.0`，
其构建结果不能代替本维护分支的验证。日志保存在
`.local-tests/upstream-20261015/local-binding-validation/`。
这类模拟失败不是本机真实 EC 故障，也不覆盖 LED、电池等完整探测流程。
按键任务用例模拟 filter 的安装状态，没有执行完整 probe 的 filter 安装尾部。

## 2026-10-15 并发修正（r10）

本节当时安装并加载 `4.24.0.r10.gc79a268-1`，功能提交 `c79a268`。
从已推送的 GitHub `main` 构建，经 `pacman -U` 升级；两个内核各完成 27 个模块编译、签名和安装。
五个运行模块的 GNU build ID 与磁盘一致，三个改动的驱动源文件与 `/usr/src` 安装内容逐字节一致。
包完整性检查仍为 100 个文件、0 个改动，UKI/引导器签名检查通过；未重启，Secure Boot 仍关闭。

本轮对风扇手动/自动模式、初始化、MODE/profile、睡眠恢复和卸载操作进行同步，
避免后台任务与请求交错时丢失目标或覆盖模式位。取消任务时释放其需要的状态锁；
卸载先排空任务再拆除资源，重新探测明确重置生命周期。
MODE 的低字节格式、profile 位映射、原有寄存器和曲线及正常 50 分钟周期保持。
DEBUG 任意地址 EC 写入不属于这套同步保障。

- **146 项本地测试通过**：46 项 ioctl、37 项风扇顺序/故障、12 项风扇并发、
  18 项 WMI、9 项电源、24 项打包/同步。
- 并发用例提取实际 C 函数，使用 pthread 条件变量固定执行交错，检查最终寄存器、缓存和任务状态。
  修改前源码有 9 项失败、2 项原有保障通过，1 项新增生命周期用例跳过；
  profile 基线提取真实旧函数，使用正确的 powersave 位映射。
  这些仍是模拟 EC/工作队列测试，不是实机故障注入或内核 lockdep 检验。
- 实机顺序两次自动模式及两线程合计四次自动模式请求均返回 0；两只风扇的 `256`、
  `UINT32_MAX` 参数仍各返回 `EINVAL`。没有请求任意手动转速或更改曲线。
- 背光 `1 → 0 → 1`、Fn 锁 `0 → 1 → 0` 写入/回读及恢复通过；RGB 恢复 `4 4 4`。
  九项不支持 TDP 读取、三项不支持写入和无效指针检查通过。
  三次温度样本均为 CPU 95°C、GPU 53°C，两个风扇原始值均为 170；不是 RPM 或散热改善证据。
- cTGP 在重载前、重载后 TCC 启动前及 TCC 启动后均为 25。
  `tccd`、`thermald`、`nvidia-powerd` 均恢复运行。
- 本轮采集窗口没有出现 NVIDIA `osapi.c:2198` 日志，但没有修改 NVIDIA 处理器，
  不能据此认定 `0xC5` 兼容问题已解决。官方后续源码核对见 [NVIDIA 记录](../NVIDIA-0xC5.md)。

真实重启、睡眠/唤醒、手动风扇、实体热键、外接显示器、充电行为及长期负载仍未验收。
TDP 仍没有本机安全边界依据，保持不支持。

本轮日志在 `.local-tests/concurrency-20261015/`，包含全量/双内核构建、并发基线/修复/独立复核、
安装和实机快照；`cpyref-receipt.json` 保存固定源码及测试引用的完整空间信息。
升级前 r8 包和两套内核的 54 个项目模块保存在
`~/Backups/projects/mechrevo-drivers/light/20261015-concurrency/`，独立解包校验及 `SHA256SUMS` 复核通过。
早期故意失败测试留下约 31 MiB core 转储，已观察并标为临时；创建时不在原 watcher 根目录内，
不能补作完整历史。测试现已禁止生成 core，详见本地 `coredump-observations.json`；未额外清理。

以下 r8、r6 内容保留为历史验证记录。

## 2026-10-15 续验

当前安装并加载 `4.24.0.r8.g90fae0c-1`，功能修复提交 `90fae0c`。
经 GitHub `main` 构建及 `pacman -U` 升级，两套内核各完成 27 个 DKMS 模块的编译、签名和安装。
安全重载项目模块后，五个运行模块的 GNU build ID 与磁盘一致；本轮改动的
`uniwill_keyboard.h`、`tuxedo_io/tuxedo_io.c` 与安装源码逐字节一致。
`pacman -Qkk` 报告 100 个文件、0 个改动，UKI/引导器的 `sbctl verify` 通过。
仍未重启，Secure Boot 仍关闭。

本轮修正风扇参数截断、EC 失败吞错、重试回读不匹配、全速模式幂等性和恢复缓存。
睡眠恢复先准备双风扇目标；部分失败不再让已排队任务使用另一风扇被清零的缓存。
挂起失败会在本回调尽力恢复，不依赖系统再调用恢复回调；后续手动/自动命令更新或取消旧目标。
后台失败后，下一次手动请求会取消旧定时任务并立即排队恢复。
原有寄存器、曲线、速度映射和正常成功路径的 50 分钟定时策略保持。

- 本地 **125 项测试通过**：42 项 ioctl、32 项风扇/睡眠恢复、18 项 WMI、9 项电源、
  24 项打包/同步。测试提取真实 C 函数，但 EC/ACPI 由模拟实现，不是实机故障注入。
  17 组风扇及 PM 回归也在修改前源码上失败，确认能捕获原错误。
- 实机向两只风扇接口分别传入 `256`、`UINT32_MAX`，四次均返回 `EINVAL`。
  连续两次标准自动模式命令均返回 0；没有请求任意手动转速或新曲线。
- 背光 `1 → 0 → 1`、Fn 锁 `0 → 1 → 0` 写入/回读及恢复通过，RGB 恢复 `4 4 4`。
  这些仍是接口验证，没有人工确认发光效果或逐键验证。
- 九项 TDP 读取仍返回 `ENODEV` 且输出未变，三项不支持写入及无效指针拒绝检查通过。
  三次传感器样本均为 CPU 93°C、GPU 53°C、双风扇原始值 170；不能当作 RPM 或散热改善。
- cTGP 在重载前、重载后 TCC 启动前及 TCC 启动后均为 25；原配置恢复，
  `tccd`、`thermald`、`nvidia-powerd` 均运行。
- NVIDIA `osapi.c:2198` 仍出现两次。已捕获 NPCF 通知和动态参数读取成功的独立链路，
  但不能据此认为 GPU `0xC5` 可忽略，见 [NVIDIA 诊断记录](../NVIDIA-0xC5.md)。

**真实睡眠/唤醒、实机 EC 故障、手动风扇控制和长期负载仍未验证。**
本轮修复不构成完整睡眠兼容或散热认证，也未解决既有整体风扇并发串行化问题。

证据保存在 `.local-tests/iteration-20261015/`：`all-tests.log`、两个内核构建日志、
`fan-runtime-tests.log`、`fan-regression-sensitivity.log`、`makepkg.log`、`install.log`、
`installed-validation.json`、`runtime-checks.json`、`after-install-checks.log`、`after-install-kernel.log`。
升级前的 r6 包和两个内核的 54 个项目模块保存在
`~/Backups/projects/mechrevo-drivers/light/20261015-iteration/`，
已独立解包校验内容、权限和所有者，`SHA256SUMS` 复核通过。

以下保留 2026-10-14 的 r6 历史验证，不代表当前安装版本。

## 2026-10-14 环境与安装状态

- DMI：`MECHREVO / Kuangshi16Pro Series GM6PX0X`，主板 `GM6PX0X`。
- BIOS：`N.1.09MRO07`。
- 系统：Omarchy 4.0.4，运行内核 `7.2.9-arch1-1`；另保留 `7.2.7-arch1-1` 的 headers。
- 安装包：`mechrevo-drivers-dkms-git 4.24.0.r6.g4bbbdc9-1`，驱动源码提交 `4bbbdc9`。
- 按仓库 AUR 配方从 GitHub `main` 构建，经 `pacman -U` 升级；不是手工覆盖系统模块。
- 两个内核的 DKMS 均完成全部 27 个模块的编译、签名和安装。
- 在当前会话安全卸载、重新加载相关模块，未强制卸载，未重启系统。
  五个已加载项目模块的 GNU build ID 全部与新安装模块一致；三个改动的驱动源文件
  与 `/usr/src` 中的安装内容逐字节一致。
- 安装 hook 重新生成并签名 UKI；`sbctl verify` 通过。当前 Secure Boot 关闭，
  不能据此认定 DKMS 签名密钥已在启用 Secure Boot 时受信任。

## 修复与结果

| 项目 | 实测结果与边界 |
| --- | --- |
| Uniwill 识别 | 使用 `uniwill_wmi`，硬件 model ID 为 16；没有强制启用 Clevo 路径 |
| TDP 读取 | 原包将不支持返回值 `-ENODEV` 显示成 `4294967277`；修复后全部九个当前值/最小值/最大值命令返回 `ENODEV`，用户缓冲区不被改写 |
| 错误处理 | 三项不支持的 TDP 写入返回 `ENODEV`，其空指针输入及七项空指针输出返回 `EFAULT`；未执行有效功耗调节 |
| Mini LED | 本机没有局部调光控制节点；回归测试验证不支持及 sysfs 注册失败时不发送初始化命令；未做 WMI 总线跟踪 |
| RGB 键盘亮度 | 标准 sysfs 亮度 `1 → 0 → 1` 写入与回读通过；恢复原 RGB 强度 `4 4 4`，未由人工确认实际发光效果 |
| Fn 锁 | 标准 sysfs `0 → 1 → 0` 切换与回读通过；未人工逐键验证 Fn 组合键 |
| 温度与风扇 | 三次样本均可读取 CPU/GPU 温度和两个风扇原始值；本轮为 CPU 94–95°C、GPU 53°C，风扇原始值 170；这是共享机器工作期间的样本，不能当作 RPM、散热改善或稳定性结论 |
| 充电 | 可读取 `high_capacity / balanced / stationary`，原配置 `high_capacity` 保持；未切换充电模式，未测实际充电行为；无标准 `charge_control_end_threshold` 节点 |
| TCC | 重载前短暂停止，验证后恢复运行；原键盘设置与 `ctgp_offset=25` 在守护进程启动后恢复，未主动调节 GPU 功耗 |
| 电源初始化 | 加载前和加载后、TCC 启动前的 cTGP 均为 25；本机跳过原有四项电源默认写入，没有套用其他型号的功耗表 |
| 同值设置 | 三次写入现值 25 均成功；旧版探针记录到三次 EC 写入，新版记录为零次；配置保持不变 |

`GM6PX0X` 未匹配现有 TUXEDO TDP 白名单。这里修复的是正确报告“不支持”，
没有借用相近型号的功率上限来启用调节。

92 项本地测试全部通过，包括 41 项 Uniwill 运行用例、18 项 WMI 用例、9 项电源用例和
24 项打包/同步检查。Uniwill 用例编译普通及 DEBUG 两个版本；WMI 用例启用 ASan/UBSan。
测试使用提取的实际 C 函数及 ioctl 定义，模拟 EC/ACPI 而不访问硬件；故意恢复历史错误的
回归有效性实验能检出错误。不能把 mock 的风扇 helper 或 Clevo 接口当作其内部错误路径已验证。
`namcap` 无错误，保留源文件架构及 DKMS hook 依赖的两个预期警告。

## 未解决与未验证

- 已捕获 NVIDIA `osapi.c:2198` 的直接条件：GPU 收到固件通知 `0xC5`，但
  [NVIDIA 615.78.08 的通知处理器](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/615.78.08/src/nvidia/arch/nvalloc/unix/src/osapi.c#L2163)
  没有对应分支。当前 DSDT 与既有反编译材料哈希一致，电源相关 `_Q83/_Q84` 同时向
  NPCF 发送 `0xC0`、向 GPU 发送 `0xC5`。不能未经依据把后者映射为 GPS 操作。
- 修复前模块加载及 TCC 启动各捕获一次 `0xC5`/断言；本轮修复后模块加载阶段没有出现，
  但 TCC 启动时真正改变功耗仍触发两次。修复消除了强制初始化及重复写入，
  **没有修复 NVIDIA 对 `0xC5` 的兼容性**，也没有屏蔽断言。
- 旧 NVIDIA PR #1402 处理的是另一种启动计数器问题，不能修复这里的 `0xC5`；本次没有更换 NVIDIA 模块。
- 未验证重启、睡眠/唤醒、外接显示器切换、实体热键、充电阈值或长期高负载稳定性。
- 没有主动修改风扇曲线、TDP 白名单、GPU 功率或充电策略；模块加载本身包含上游的
  EC/WMI 初始化，已有 TCC 服务也会应用原配置，因此不能称整个过程“没有 EC 写入”。
- 风扇/温度样本只证明正常读取路径可用，未模拟实机 EC 故障；TDP 仅验证拒绝不支持及无效输入，未验证任何成功功耗写入。

## 本地证据与恢复

本轮日志和快照位于 `.local-tests/continuation-20261014/`：`all-tests.log`、
`makepkg.log`、`install.log`、`runtime-checks.json`、`module-build-ids.json`、`after-tccd.json`、
`power-preserved.json`、`notify-reload-*`、`notify-corrected-reload-*`、`same-power-before.txt`、
`same-power-after.txt` 及两个内核的构建日志。初次验证仍保存在
`.local-tests/local-adaptation-20261014-2253/`。不将机器日志提交到源码仓库。

本轮升级前的 r3 安装包和两个内核的 54 个项目模块已归档到
`~/Backups/projects/mechrevo-drivers/light/20261014-continued-correction/`，
独立解包校验内容、权限和所有者，并保存 `SHA256SUMS` 与恢复说明。
恢复优先使用保存的包及 DKMS，不要手工删除 `/usr/src` 或 `/var/lib/dkms`。
