# Rust 双端控制台

源码位于 [console](../console/README.md)，与内核驱动和 TCC 分开构建。CLI 和原生图形界面使用同一 Rust 核心。Windows 和 Linux 各有系统后端；实际功能以运行时能力结果为准。

## r177 系统档位实写与 CPU 控制权竞争

安装版 CLI 已用普通用户完成两轮平衡→节能→平衡，四次 Set 均确认并独立读回；同档位操作及错误基线拒绝也已核对。同步观察捕获到 PPD 的实际 CPU 偏好写入，但 TCC 随后又写回节能偏好，约 2.878 秒即覆盖一次平衡设置。当前系统档位已恢复平衡，原 55W 保护、TCC 配置与服务保持不变；持续 CPU 控制权交接尚未实现，不能据此声称性能或风扇异响已经修复。[实测记录](verification/linux-profile-control-r177.json)、[交接要求](CPU-POLICY-OWNERSHIP.md)

后续 TCC pkgrel7 CPU 交接后端已落地源码并通过 78 项 TCC 离线回归及完整 TypeScript 检查；尚未生产打包、安装，也未接通控制台的提权 helper 和原生 CLI／GUI 操作。安装版的持续竞争限制仍保留。[后端验证](verification/cpu-ownership-backend-pkg7.json)

## 系统电源策略与风扇最低比例（r177 已安装，风扇实机待验）

Linux 源码新增 PPD 系统电源服务读取／设置及界面；只有确认两个服务名字都无 owner 才回退 ACPI，异常或身份变化不绕过服务。预览绑定真实公布档位、当前值、应用临时请求和服务身份；设置可能先释放应用请求再失败，故 PPD 不自动重试或恢复。CLI 新增 `profile … --expected-current RAW`，精确核对原始名称／GUID；刷新保留未知回执，新的显式意图和本地 worker 完成后关闭仍允许。[系统电源策略](SYSTEM-POWER-PROFILES.md)

完整用户风扇曲线新增可选 CPU／GPU 最低输出比例 `1..100%`，默认不设置且保留 0 值档位；只有显式选定通道并允许抬高 0 值才修改它们。GUI 点“应用到草稿”后仍需预览和显式应用，继续走原有租约及原表恢复。它是占空比下限，不是 RPM 或已验证的避振参数。[风扇合同](FAN-CURVE-CONTROL.md)

本轮签名源码 `d48692a` 已推送，95 份冻结输入通过 580 项 Rust／143 项 Python 控制台检查及格式、Clippy、Windows 编译；实际 Linux 发布特性另有 579 项测试通过、源码前后一致。Linux `0.1.0.r177.gd48692a-1` 已安装，11 项载荷和包完整性核对通过，原 55W／90°C／1 秒／0 秒策略、四个服务、EFI 与六个加载模块身份不变；没有 setter、重载或重启。安装版普通用户四条只读命令全部退出 0、stderr 为空；真实 PPD 当前 `balanced` 的带基线校验预览前后未变，两帧热采样正常。私有原生窗口的系统电源服务概览、未探测且无草稿的风扇页经两张 1280×720 截图核对，根代理也逐张查看；没有授权、应用、Set、探测或导入。首次 AT-SPI 桌面未就绪导致导航前失败，证据保留；仅测试脚本加有界等待和空值保护后重试通过，Rust 和安装 ELF 未改。应用／registry 以 SIGTERM 等待回收，Cage 退出 0，自有 PID 和进程组均空；不证明正常关窗、最低比例运行或页尾可达。独立 root 审计已双遍核对 28 份原文件及清单共 29 项，包载荷／策略／服务／EFI／模块身份再次通过。离线检查和安装都不能算 PPD 实写、真实风扇租约或声音验收。[r177 安装收据](verification/deployment-console-r177.json)、[历史候选收据](verification/os-profile-fan-control.json)

## r174 历史安装验收（2026-10-18）

Linux 控制台 `0.1.0.r174.g256fb05-1` 已真实安装，功能源码为本地签名提交 `256fb05aded292eca662bc0eaf1d876320596a24`，打包元数据另由签名提交 `c0b11920db7a03a5bc32fb033bd9a470bd1db12c` 固定。92 份输入匹配最终 519 项全特性 Rust／140 项 Python 控制台检查；实际 Linux 包只启用 GUI 特性，对应离线测试 518 项通过、0 失败。另一次 Windows all-targets 仅编译检查退出 0，保留两条 unused 警告，不能算 Windows 运行通过。[r174 安装收据](verification/deployment-console-r174.json)

11 项载荷的字节、模式和 root 归属均与包匹配；控制台／功耗工具／驱动包完整性为 23／26／126 项、零改动。旧 Cargo target 在原位复用，本轮日志确认三个工作区 crate 从新固定干净源码重新编译，没有宣称缓存为空。新备份以独立目录句柄、精确清单和两次摘要核验覆盖 28 份文件及清单自身，共 29 项；旧 r171 的空清单缺陷与补充证明仍按历史保留。安装版普通 UID 1000 的包版本／归属／完整性、38 项状态读数及两帧温度／转速采样均通过，五个命令退出 0、stderr 为空。CLI／GUI 摘要前后匹配。

已安装原生 GUI 在私有 Cage／XDG／D-Bus／AT-SPI 环境中完成概览、“控制”和未检查的 CPU 功耗页核对，三张 1280×720 截图经人工复核；根代理另看过概览。只点击本进程两个选择器，未检查 CPU、授权或应用，Linux 界面没有 Windows Fn 入口。应用和自有 registry 经 SIGTERM 回收，compositor 退出 0；无超时或 SIGKILL，原有用户 registry 不变，没有访问实际用户桌面。这是隔离启动／显示验收，不是正常关窗或硬件控制验收；独立清理检查确认自有应用与 registry 的精确 PID 已消失、私有进程组为空，wrapper 已等待结束且退出 0。r171 亮度写入与 r168 CPU 事务仍只属于各自历史版本。

本次仅升级控制台：功耗工具仍为 `0.1.0-18`，驱动仍为 `4.24.0.r162.ge4bfb3f-1`，未重装两者、重载、重启或执行硬件 setter。55W／90°C／1 秒／0 秒策略、四个服务的 PID／启动时间、四份 EFI 摘要和六个加载模块身份均不变，新风扇接口仍未加载。安装跟踪 complete、0 gap／0 unresolved；更早的 wrapper 因把文件作为 root 参数而拒绝，子进程未开始，这次失败另记，不覆盖。当次验收时源码尚未推送，后续已同步，未发布 AUR；真实风扇写入、噪声效果及 Windows 兼容均未增加验收结论。

## Windows Fn 锁源码候选（尚未安装）

本轮 Windows Fn 锁源码已接入真实 Win32 设备读写、完整 EC 字节的条件修改／独立回读／条件恢复、固件与官方驱动资格检查，以及受保护安装位置内的限权 helper。GUI／CLI 保持普通用户身份，只有显式检查或应用才请求 UAC；自动刷新和本地缓存预览不提权。CLI 候选入口为：

```powershell
mechrevo-console.exe oem windows-fn probe
mechrevo-console.exe oem windows-fn set on --dry-run
mechrevo-console.exe oem windows-fn set off
```

`windows-fn set --dry-run` 先授权只读检查，再做本地预览，不写 Fn 状态；它与不会提权的通用 `oem set --dry-run` 不同。应用不会因退出自动撤销，失败或未知结果不会自动重试；helper 尚未确认退出时保留待完成状态。GUI 已接检查、预览、显式应用和等待完成流程，但没有 Windows 原生 GUI 或实际 Fn 按键验收。

固定支持清单当前只有未验收 candidate；默认关闭 `windows-fnlock-candidate` 编译功能，没有正式 `runtime_accepted` 名单，也没有用户自填清单、地址或跳过资格的运行开关。MSI 配方及维护 gate 已落源码；维护 DLL 仅通过 MinGW 交叉编译／链接，MSVC／WiX／MSI 构建、签名发布、Windows 安装升级／Repair、UAC 及硬件读写均待验收。最终离线检查已通过 519 项 Rust／140 项 Python 控制台测试，格式、Clippy 和 Windows 目标编译通过；这不代替 Windows 实机验收。[候选收据](verification/windows-fnlock-control.json)、[历史设计约束](WINDOWS-FNLOCK-CONTROL-DESIGN.md)、[Windows 构建说明](../packaging/windows/README.md)。

本机已安装控制台现为上文 Linux `0.1.0.r177.gd48692a-1`；“候选尚未安装”指 Windows 产物。本轮没有安装 Windows 候选、重载驱动或重启，也不增加完整兼容或问题已解决的结论。

## r171 历史安装验收（2026-10-18）

控制台 `0.1.0.r171.g00e3998-1` 已真实安装，功能源码为本地签名提交 `00e3998a5bc35acde240c08b91b2693ac444e5c8`。59 份输入与最终 395 项 Rust／135 项 Python 控制台检查一致；格式、Clippy 和 Windows 目标编译通过，Windows 仍未实机验收。11 项载荷的字节、模式和 root 归属与包一致；控制台、原功耗工具、原驱动包完整性分别为 23／26／126 项，均为零改动。[r171 安装收据](verification/deployment-console-r171.json)

本包包含 Linux 活动本地会话的 logind 屏幕亮度调节、共享条件恢复、CLI 原始值基线校验与 GUI 未知状态写入阻断。已安装 CLI 由 UID／EUID 1000 经活动会话 logind 完成 70%→69%→70% 两次显式请求；`intel_backlight` 上限 96000，请求值和驱动实际值均为 67200→66240→67200，原值已恢复。两次原始值基线分别为 67200／66240，事务均 confirmed、无需故障恢复，无观察到的外来改值或错误，24 次独立观察及前后运行保护通过。此处是驱动回报，不是光学亮度测量；没有内核级比较交换或排除同值 ABA 竞争的保证。GUI 应用、真实故障恢复及实际发光效果不增加通过结论。

已安装原生 GUI 在独立私有 D-Bus／Cage／registry 环境运行约 10 秒，通过本进程的精确“预览亮度”按钮执行一次本地预览。根代理目视确认目标、请求和驱动报告均为 70%，会话调节路径及待应用 67200→67200 可见；未拖动滑块、点击应用、请求授权或操作其他硬件。四次端点请求值／驱动实际值均为 67200，这不是连续监控或原子快照。应用与 registry 经 SIGTERM 回收，compositor 正常退出且未超时或 SIGKILL；不能算原生正常关窗验收。独立只读清理核验确认应用、registry、compositor 三个自有 PID 已消失，自有进程组及该次私有运行目录下的挂载均为空。该 GUI 运行命令成功，跟踪仍为 0 gap／4 unresolved，不用后续完整总结覆盖这段部分历史。[亮度候选收据](verification/display-brightness-control.json)保留安装前的离线范围，不能把候选的“未安装”理解为当前软件包状态。

本次只升级控制台，功耗工具仍为 `0.1.0-18`，驱动仍为 `4.24.0.r162.ge4bfb3f-1`；没有重装这两个包、重载驱动、重启、写入 EFI 或在安装阶段执行硬件 setter。运行保护前后通过，四个服务 PID／启动时间、四份 EFI 摘要及旧加载模块身份不变，新风扇接口仍待加载。根安装命令成功，但文件跟踪有 0 gap／4 unresolved，和包载荷核对结果分别记录；恢复与归档材料位于统一备份目录；原归档 manifest 被发现为空清单，不能证明完整归档，原件保留；新增的独立清单已逐项核对原 26 份文件的摘要、字节数、类型、权限、root 归属及单链接，并重读匹配，18 项冻结输入也与收据一致，未重装或改写旧清单。这不改变已通过的安装载荷／包完整性／运行保护结论。全型号兼容、完整功能族和上游问题均不增加完成结论。

## r168 与功耗工具 18 历史安装（2026-10-18）

控制台 `0.1.0.r168.gd2d50f7-1` 和 `mechrevo-power-control 0.1.0-18` 已安装，固定功能源码为 `d2d50f701a5d44498069fdaa1a053159775c4fb1`。56 份输入匹配通过验证的源码；342 项 Rust、219 项功耗后端、135 项 Python 控制台检查及格式、Clippy、Windows 编译检查通过。这些是完整套件计数，不是新增功能各自的测试数。两个包共 23 项载荷核对一致，包完整性分别为 23／26 项、零改动。[本次安装收据](verification/deployment-cpu-r168.json)

Linux 新增 CPU PL1 的完整策略检查、预览和授权执行链。已安装 Rust CLI 的授权检查与本地预览通过；固定已安装 helper 经 root `pkexec` 完成 55W→54W→55W 两次事务，MSR／MMIO 两路分别回读 54W、55W，原有 90°C／1 秒／0 秒配置完整恢复。PL2、平均窗口和硬件 TCC 参考不变；没有进行压力负载或风扇写入。执行路径是固定 helper，不把它写成 Rust CLI execute 或普通用户认证弹窗的实机验收。

包安装前后原有配置、服务与六个加载模块身份相同。随后功耗事务按既有协调器重启 thermald；策略文件恢复，状态中的事务回执按设计更新，其他受保护配置、服务、加载模块和启动身份仍通过核对。控制台普通用户只读状态、OEM／能力查询及三帧温度／转速采样通过，三帧各 33 项。原生隔离窗口启动、概览及 CPU 未检查页已目视核对。前两次私有无障碍尝试的问题与证据保留；第三次显式启动私有 registry 并验证进程身份后，仅切换“控制”和“CPU 功耗上限”两个选择器，确认未检查状态、MSR／MMIO 破折号及未提供数字输入。未触发授权或应用，自有进程均已回收；GUI 写入仍待验收。该次运行跟踪有 2 gaps／8 unresolved，和界面验收成功分别记录。

r162 驱动与签名启动镜像保持原安装版本，新风扇内核接口仍未加载；未重载或重启。真实风扇曲线、异响改善、普通用户认证、GUI 应用及 Windows 实机仍待验收。旧版两个包、新包、完整源码和回执已保留至统一备份目录。源码已在本地提交，密钥环仍锁定，尚未推送或发布 AUR；完整功能族和上游问题均未因此关闭。

## r166 历史安装（2026-10-18）

控制台 `0.1.0.r166.ge3b7852-1` 已通过 pacman 安装，固定源码为 `e3b785274eb4923eff311bb9c11f6074601411d6`。44 份构建输入与 273 项 Rust／131 项 Python 验证一致；11 项载荷的字节、权限、root 归属逐项核对，包完整性 23 项、0 改动。安装版 CLI 的状态／OEM／能力查询及三帧温度／转速采样通过；专用采样每帧 33 项，追踪中无 ioctl 或可写打开。安装版原生窗口的概览和未探测风扇页在隔离环境目视核对，随后以 SIGTERM 回收；这不证明正常关窗恢复、管理员弹窗或 GUI 应用成功。

受保护配置、服务、六个加载模块身份与启动 ID 保持不变。r162 驱动和签名镜像仍待重启加载，真实风扇写入及噪声未验收。旧安装包、新包、源码及证明材料已校验归档；源码尚未推送，密钥环仍锁定，未发布 AUR。[r166 安装收据](verification/deployment-console-r166.json)分别记录实际检查和文件跟踪缺口；[采样收据](verification/thermal-sampling.json)保留早期候选本身的未安装范围。

## r162／r163 历史部署基线（2026-10-18）

该次部署收据记录控制台 `0.1.0.r163.g58e8d9c-1` 和驱动 `4.24.0.r162.ge4bfb3f-1` 已安装，签名启动镜像已更新，旧版恢复入口保留。r163 包含原生风扇曲线编辑器和受限会话 helper；该次验收时新驱动尚未加载，真实曲线与噪声效果未验收。[部署收据](verification/deployment-fan-r162-r163.json)、[风扇接口](FAN-CURVE-CONTROL.md)。后续源码进展与该安装基线分开记录；新的采样候选尚未由本收据证明已安装，后续实际部署另有收据。

## r160 历史安装验收（2026-10-18）

`mechrevo-console 0.1.0.r160.g5542b9d-1` 已从固定提交 `5542b9d66eb3bb95b2edd07e4f2fe94afc36ea60` 构建并通过 pacman 安装。CLI、GUI、受限 OEM helper 与授权策略已落盘；21 个包文件零改动，9 个正规载荷的字节、权限及 root 归属逐项一致。release 的 31 份输入与通过 146 项 Rust、95 项 Python 检查的快照相同；release 与早期 debug 候选的二进制摘要分别保留。

安装版普通用户 CLI 的版本、状态和 OEM 枚举通过，策略注册可见。另用普通用户已安装 CLI 生成新预览，再由 root 经 `pkexec` 调用固定已安装 helper，完成两次 Fn 切换／恢复；四项控制独立回读恢复，运行保护状态保持不变。这证明已安装路径可执行，不证明普通用户管理员认证弹窗或 GUI 应用流程已验收；其余三项控制没有在这次 release 验收中写入。

安装版 GUI 在独立 headless compositor 中持续运行约 25.58 秒，1280×720 的概览、四个 OEM 选择项及 Fn 当前值面板已人工复核。没有访问其他三个面板、预览或应用，也没有通过 GUI 写硬件；自有进程均已结束。来源、包摘要、恢复材料与跟踪缺口见 [r160 部署收据](verification/deployment-console-r160.json)。

本次只更新控制台，没有更改 EFI、TCC、风扇或功耗配置，没有重载驱动或重启。磁盘驱动仍为 r158，运行中仍为 r106。Windows OEM 与 Windows 实机验收未完成；没有发布 AUR。本轮源码已在本地提交，远端推送因系统密钥环锁定待完成。

## r158 历史安装基线（2026-10-18）

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

## Linux OEM 控制

本轮源码已通过 146 项 Rust 测试、95 项 Python 检查、格式、Clippy 和 Windows 编译检查。本机四项控制完成 11 次可逆写入及 7 次写前拒绝，独立回读确认全部恢复原值，配置、服务与加载模块保持不变；候选原生窗口的概览和控制页已人工复核。[OEM 验收收据](verification/oem-controls.json)记录源码指纹、二进制摘要及具体边界。这些是 r160 构建前 debug 候选验收，18 次事务与已安装 release 的两次 Fn 验收分别记录；r158 历史基线不含这些新控制，该次 r160 的安装验收另有收据。完整 Windows／Linux 目标继续以 [28 个功能族及 77 项问题的对齐清单](OPENREVO-PARITY.md)为准。

核心、CLI 和 GUI 共用 OEM 后端。它只接入已加载 `tuxedo_keyboard` 驱动实际暴露并通过绑定校验的固定 sysfs 接口，不从相似型号推测寄存器。状态文档新增 `oem_controls`，逐项保留 ID、类型、当前值、允许范围、来源、驱动绑定、可写状态及 `readback_scope`。没有接口就不生成控制项，接口无效或权限不足也不伪造可用状态。

| 控制项 | CLI 值与当前范围 | 回读边界 |
| --- | --- | --- |
| Fn 锁 | 枚举 ID `fn-lock`；`on`／`off` | EC 经加载驱动报告的状态，实体 Fn 行为与重启持久化需另验。 |
| 充电档位 | `charging-profile`；仅接受驱动列出的原始选项 | 档位不等于固定 60%／80% 阈值，回读不证明电池已实际停止充电。 |
| 键盘亮度 | 使用列出的完整 `keyboard-brightness:…` ID；整数 `0..max`，本机当前最大级别为 4 | 使用驱动给出的原始级别，不当作百分比；LED 请求状态缓存不测量实际发光。 |
| 单区键盘颜色 | 使用完整 `keyboard-color:…` ID；`'#RRGGBB'` | 仅针对识别到的单区 RGB 接口；回读请求缓存，不承诺实际颜色、逐键或多区灯效。 |
| 接电开机／关机 USB 供电 | `ac-auto-boot`／`usb-power-share`；`on`／`off` | 只有实际暴露才列出，本机未暴露；冷接电、关机供电需对应设备另验。 |

```sh
# 先查看本机真实 ID、选项和回读范围
mechrevo-console oem list
# 仅在上一步确实列出 fn-lock 时预览；不会写入或提权
mechrevo-console oem set fn-lock on --dry-run
```

CLI `oem set ID VALUE` 在没有 `--dry-run` 时生成计划并显式应用。GUI 顶部新增“控制”，按选择功能、调整目标、预览更改、应用并回读的顺序执行，继续使用单个后台任务。计划记录原值、驱动与设备绑定、范围及伴随亮度；应用前重新校验，拒绝过期、绑定已变或已失效的计划。写事务、回读及恢复期间延后关窗；不自动重放旧计划。

Linux 普通用户对真实 `/sys` 的 OEM 写入由 `pkexec` 调用固定 `/usr/lib/mechrevo-console/oem-helper`。helper 重新使用 `Service::new()` 访问真实 `/sys`，校验类型化计划，并以 `flock` 互斥；不接受任意路径、替代 sysfs 根、shell 或插件。无需将整个应用作为 root 运行，也不将 sysfs 改为全局可写。授权只发生在显式应用，枚举和 `--dry-run` 不提权；随包策略对活动会话使用 `auth_admin_keep`，管理员授权可短时缓存，非活动会话要求 `auth_admin`，`allow_any` 为 `no`。helper 与策略已随 r160 安装并核对，旧 r158 不具备这条通路；当前实测范围仍按前述 root pkexec 验收区分，不扩写为普通用户认证或 GUI 应用通过。

特权 helper 必须等待真实退出，不以超时杀进程宣称写入已取消。输出失败但进程已退出时报告 `outcome_unknown`；若进程存活状态也未知，继续阻止后续写入和关窗。写入原始错误、恢复写入、恢复回读分别保留；检测到第三方修改或状态未知时不覆盖他人的值。RGB 恢复写入失败，即使 LED 缓存仍显示旧值，也按状态未知处理。这些流程不等于已完成实体恢复验收。

Windows OEM 后端尚未实现，已有 Windows 系统亮度与电源策略后端继续保留。上述 OEM 控件不包含功耗模式；风扇曲线使用独立的会话接口，见[风扇控制](FAN-CURVE-CONTROL.md)。充电、按键、灯光、风扇与持久化均需分别验证，接口实现不能证明异响已解决。

## 型号与证据

官网目录快照收录 298 条公开记录，含 178 条笔记本、70 条台式机、28 条软件、16 条外设和 6 条待分类记录。它是营销目录，不能算成 298 台已适配电脑。同一个名称可能对应多块主板和多代固件。

```sh
python -B scripts/sync_mechrevo_catalog.py --dry-run
python -B scripts/sync_mechrevo_catalog.py --check --output data/mechrevo-catalog.json
```

同步器按页校验数量、重复 ID 和总数变化，保存每页 SHA-256，网络或校验失败时保留原文件。官网没有快照令牌，因此校验完成也不能证明抓取期间绝无等量替换。

Rust 身份识别使用独立的精确规则。品牌命中仅是身份线索；GM6PX0X 的产品名、主板和 BIOS 同时命中，仍只对应历史部分实机验证。来源匹配、固件匹配、硬件验证分别表示；冲突不选择首条规则。控制台不读取序列号、UUID 或 MAC。

## 系统功能

r174 及更早安装的电源策略使用 Linux `platform_profile` 和 Windows `powercfg`。新源码另接入优先的 Linux PPD 系统服务，具体资格、holds 副作用与失败合同见[系统电源策略](SYSTEM-POWER-PROFILES.md)；它已随 r177 安装，真实服务 Set 尚未验收。这里的系统策略不等于原厂办公、狂暴或指定瓦数模式，PPD 的服务副作用也不能按直接 sysfs 写入处理。

屏幕亮度使用 Linux backlight 或 Windows `WmiMonitorBrightness`。控制值与实际亮度分开；多屏需要选择目标。缺驱动、权限不足或不支持的目标值会返回明确错误，不显示虚构的成功状态。

遥测保留设备 ID、单位和来源。EC 温度不是 CPU 核心温度，GPU 休眠也不证明功耗为零。缺失值显示不可用。启动和查询不自动调整 GPU、风扇、充电、ICC、MUX 或功耗限值。

Linux hwmon 的传感器 `id` 使用解析符号链接后的完整输入路径，例如 `/sys/devices/…/hwmon/hwmon7/temp1_input`。旧版使用物理设备路径加通道名；同一设备的多个 hwmon 接口都含 `temp1_input` 时会发生 ID 冲突。新版把这些接口分别保留，即使读数相同也不会合并。多个 class 路径若指向同一输入，只保留排序后的第一个来源，并记录 `sensor_source_alias` 诊断。`device_id` 仍表示物理设备归属，`source.path` 仍保留实际枚举到的原始路径。

开发源码还会在设备报告 `suspending`、`resuming` 或 `error` 时跳过输入和标签；`active`、运行时电源管理未启用的 `unsupported` 和没有此属性的设备继续按原方式读取。状态含义以[内核 ABI](https://www.kernel.org/doc/Documentation/ABI/testing/sysfs-devices-power)为依据。读取前检查不能锁住后续电源转换，不代表消除了竞态，也不把休眠／错误读数当作零值。

依赖旧 `id` 的消费者需要迁移已有索引；此变化仅涉及 Linux hwmon，电池等其他 ID 不变。新 ID 不保证跨重启或 hwmon 重新编号稳定。输入在枚举后消失时，优先使用解析后的父目录加文件名，仍无法解析才保留原路径。设备被报告为 runtime-suspended 时，控制台解析路径但不打开传感器输入，保留 ID 与来源，将读数标为不可用；它不会以旧读数或零值代替本次采样。

Windows 标准接口依据：[电源策略](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/powercfg-command-line-options)、[亮度类](https://learn.microsoft.com/en-us/windows/win32/wmicoreprov/wmimonitorbrightness)、[亮度方法](https://learn.microsoft.com/en-us/windows/win32/wmicoreprov/wmisetbrightness-method-in-class-wmimonitorbrightnessmethods)。Linux 接口依据：[backlight](https://docs.kernel.org/gpu/backlight.html)。

## CLI JSON 与采集时间

开发源码另提供仅查询 Linux hwmon 温度和转速的命令：

```sh
mechrevo-console thermal
mechrevo-console thermal --samples 30 --interval-ms 1000
```

输出为每次一行的 JSONL。默认采一次；次数为 `1..3600`，间隔为 `250..60000` 毫秒，默认 1000 毫秒。首帧立即采集，后续在上一帧采集和输出完成后等待，不追赶错过的间隔。每帧有序号、UTC 起止时间、单调读区间耗时、传感器来源与状态，可用于关联录音；读区间不是固件新鲜度或同时采样的保证。采集失败会输出不含旧读数的错误帧并退出；断管或输出失败立即停止，若同时发生采集错误则保留两者。命令不枚举 OEM 控件、亮度、电源方案或型号目录，不提权、不接管风扇。Windows 目前明确返回此专用采样不可用，原有完整状态命令继续保留。此命令不属于 r163 安装包。 新增采样候选的 273 项工作区 Rust 测试、三项隔离负例及三帧真实 CLI 只读追踪见[采样收据](verification/thermal-sampling.json)；该收据另含隔离 Linux 1280×720 启动及未探测风扇页只读导航、131 项 Python 检查；不包含实际桌面、正常关窗、GUI 写入或新版本安装。

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

原生界面已替换旧 Cyber-HUD，采用石墨背景、暖白文字与有限的低饱和绿／琥珀曲线，不以蓝色作为视觉方向，也不复制 OpenRevo 内嵌资产。默认主窗为 1120×760 逻辑单位，使用可换行的顶部导航，概览按内容宽度转为单列。首页产品长名省略显示，完整名称可在悬停或设备详情中查看；传感器页可展开全部来源。视觉与交互约定见 [界面设计](../console/DESIGN.md)。

首页 CPU / GPU 分别显示同类当前最高读数，曲线只显示当前选定实际来源的真实温度历史，不推断物理热管拓扑。历史按机器与传感器身份隔离，每来源最多 120 点；未知读数保留断点，不补零、不补齐历史。实时采样默认开启、可关闭，干净状态距上次完整快照至少 3 秒且空闲时只读刷新电池／AC 等信息；Linux 有待确认预览或风扇会话时改为仅采温度／RPM，保留预览确认，Windows 预览期间暂停自动采集；开启时空闲界面每 1 秒重绘。选中导航与常规控件的过渡为 180 ms，后台忙碌时刷新图标旋转，动画可关闭且偏好会保存。界面读取仍放在单个后台任务中，重复刷新不堆积，主线程不等待硬件调用。

电源与亮度保留预览、应用、回读、必要恢复及写事务结束后关窗的流程。r160 OEM 实现在相同事务框架内接入有限驱动控制，范围与验证状态见前文；OEM 性能模式、风扇曲线、MUX、水冷及 Windows OEM 控制仍待完成。已有界面验收记录不覆盖这些新硬件控制，也不增加 Windows 实机写入结论。

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
