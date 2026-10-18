# Windows Fn 锁：客户端、受限 helper 与实机验收契约

2026-10-18。本文保留 Windows Fn 锁接入前的设计约束及当时缺口，**不是安装或 Windows 实机通过回执**。下文“需要”“首版”等表述属于设计阶段；本轮源码进展以 [Windows Fn 锁候选收据](verification/windows-fnlock-control.json) 为准，已安装的 Linux r171 与本轮 Windows 候选分开记录。

首版只接 Fn 锁，完整交付授权检查、缓存预览、真实 EC 读改写、独立回读、条件恢复和安装包。不得以空实现、固定 `unsupported` 或模拟读数冒充接入；实际传输必须可由资格成立的正向分支调用。发布支持范围依据版本化实机验收记录，不能把初次开发候选写成已验收。

## 后续源码进展（未安装候选）

现已接入完整字节事务、真实 Win32 4MiB 设备传输、固件／PnP／SCM／PSAPI／文件摘要与 catalog 资格检查、跨会话互斥及持久维护 gate；普通用户客户端通过受保护安装位置的固定 helper、UAC 和身份认证 pipe 执行显式 Probe／Apply，保留实际进程退出状态。CLI 与 GUI 的检查、缓存预览、应用和待完成状态已接线。默认构建不启用候选功能，编译时 `windows-fnlock-candidate` 仅用于单独安排的验收候选；当前固定支持清单只有 candidate，没有 `runtime_accepted` 记录。

Windows x64 MSI 配方、发行清单及执行权限清单已编写；维护 DLL 已在 Linux 上通过 MinGW 编译和链接，不能代替 MSVC、WiX／MSI 构建或真实 Windows 安装、升级、Repair、UAC、Fn 按键与恢复验收。最终 Rust／Python 验证仍在进行，结果与未完成项见候选收据；[Windows 构建说明](../packaging/windows/README.md)给出实际参数。此进展不证明全型号兼容，不关闭任何功能族或上游问题。

## 已有事实和本版范围

协议事实以 [Windows EC ABI 回执](verification/windows-ec-abi.json) 为准；原厂方法体、二进制及密钥保留在私有分析目录，不进入本设计、源码或安装包。

| 项目 | 固定值及来源 |
| --- | --- |
| 设备入口 | `\\.\ACPIDriver`；不替换为 `UniwillAcpiDriver`、WinRing0 或相似名字。 |
| 驱动包 | `UWACPIDriver`，`ACPI\INOU0000`，INF 版本 `10/28/2025,10.48.11.331`，KMDF 1.15。 |
| 驱动完整文件 SHA256 | `6e360ee4a0c42b2a4eaf46208c1592c87f8cd71aca2a6c52831a2ad356e61235`。该摘要不是当前加载内存的摘要。 |
| 读 IOCTL | `0x9c40a488`；输入 offset 0 为小端 u32 地址。 |
| 写 IOCTL | `0x9c40a48c`；offset 0 为小端 u32 地址，offset 4 为数据，驱动只消费一个数据字节。 |
| 唯一控制项 | `fn-lock`，Boolean，地址 `0x074e`、掩码 `0x10`。OEM token `06000fc7` / `06000fd6`，另有 [Linux 掩码实现](../src/uniwill_keyboard.h) 与 [地址定义](../src/uniwill_interfaces.h) 交叉依据。 |
| 首个固件候选 | GM6PX0X / MRO07；DSDT 原始表 SHA256 `de42c0d802ed746c1d032bf53ebb3b80f92172b8fb55e115b0e433566f542cda`。DSL 摘要是分析来源，不能代替原始表摘要。 |

底层 ECRR/ECRW 的错误可能被驱动外层吞掉；正常完成信息填的是调用者 OutputBufferLength。不能把 `DeviceIoControl` 成功、返回长度或 DLL 的单字节 getter 当作硬件 ACK。失败可能原样留下地址 DWORD `0x0000074e`；完整 DWORD 的合法字节值 `0x0000004e` 与它不同。[METHOD_BUFFERED](https://learn.microsoft.com/en-us/windows-hardware/drivers/kernel/buffer-descriptions-for-i-o-control-codes) 规定输入和输出共用系统缓冲区，因此必须明确初始化输入。

首版不包含驱动自动安装、原厂服务接管、充电、性能档位、风扇、HID 灯效、任意地址或任意 IOCTL。原厂相关参数不从型号相近、函数名称或接口名称推导。

## 设计形成时的代码缺口（历史）

以下列出接入前的状态，用于解释设计约束，不代表本轮实现后的当前缺口；后续实现及验证范围见开头的候选收据。

- [oem.rs](../console/crates/core/src/oem.rs) 的非 Linux 枚举为空，计划与写入均 unavailable；需要真实 Windows 分支。
- [privilege.rs](../console/crates/core/src/privilege.rs) 的非 Linux `apply_oem` 直接退回 Service，`apply_root` 不支持 Windows；接入写入时必须同时替换此退路，不能悄悄让整个 GUI 提权。
- [windows.rs](../console/crates/core/src/windows.rs) 的 CIM 信息用于展示，其 PowerShell 路径来自环境 `SystemRoot`，不能拿这条链的输出作为提权 helper 的硬件资格。
- 公共 `Observation` 只保存 Boolean 和伴随亮度。两个 Boolean 值必然属于 previous/target，不能发现共享 EC 字节其他位被第三方改变；Windows Fn 锁需要私有完整字节事务。
- 读取 IOCTL 也要求 FILE_WRITE_DATA 对应的设备句柄权限。最终设备 ACL 未验；因此普通用户的检查也必须有显式授权入口，不能只实现 Apply 而让用户永远无法获得计划。

## 资格：把磁盘、加载关联与验收状态分开

本工具信任 Windows 内核、系统管理员和受保护安装位置，不新增防本机管理员篡改内核的威胁模型。**Sysmon、内存取证及新的内核测量驱动都不是首版依赖。** 缺少当前内存摘要不自动禁止已经实机验收的硬件／驱动组合。

`SupportContract` 是随发行版本固定的只读数据：contract ID、精确 SMBIOS 字段、BIOS、原始 DSDT 摘要、驱动包／版本／完整文件摘要、已验 Windows 架构与版本范围、缓冲契约、实机验收回执摘要和 `candidate` / `runtime_accepted` 状态。客户端请求不能提供、替换或扩展 contract。

开发候选必须实现同一真实 Probe/Apply/恢复路径，供明确安排的 Windows 人工验收使用；其输出持续标为 candidate。正式 GUI／CLI 的可写资格由对应组合的实机验收记录决定，不把离线正向 fixture 计作验收。验收构建、正式发行的 contract 列表及测试操作在发布记录中分开；不提供用户自填 hash、任意支持清单路径或忽略资格的正式 CLI 开关。

每次授权检查和 Apply 内均重新取得以下证据，不能仅相信缓存、计划里的来源字段或驱动文件名：

1. 通过原生 `GetSystemFirmwareTable` 读取 RSMB，边界检查后解析 vendor、product、board、BIOS，按 contract 精确匹配；不使用品牌模糊匹配。
2. 枚举并读取原始 DSDT：校验表头、声明长度、校验和与完整 SHA256。Provider 与 TableID 的字节序分别有测试；DSDT 缺失、重复而无法确定、截短、长度变化或摘要不符均拒绝。微软说明同名 ACPI 表只能取得第一份，因此不能忽略重复歧义。[固件表 API](https://learn.microsoft.com/en-us/windows/win32/api/sysinfoapi/nf-sysinfoapi-getsystemfirmwaretable)
3. SetupAPI / Configuration Manager 枚举当前 present、正常启动的 `ACPI\INOU0000` 实例，读取当前节点的 service、driver key、INF、版本及问题状态；选出唯一对应 `UWACPIDriver` 的节点。安装元数据不等于加载证明，多个候选不任意取第一项。
4. SCM 核对 SERVICE_KERNEL_DRIVER、实际 running 状态、配置与映像路径，并与节点对应。`QueryServiceConfigW` 返回注册表配置，运行期间变更可能只在下次启动生效，不能独自证明正在运行的映像。[SCM 限制](https://learn.microsoft.com/en-us/windows/win32/api/winsvc/nf-winsvc-queryserviceconfigw)
5. PSAPI 枚举当前内核模块，由 `GetDeviceDriverFileNameW` 取得实际加载模块路径，与上述候选及同一文件身份对应。记录 base/path/可取得的映像元数据；不能把同名模块或磁盘 PE 头字段写成内容测量。Windows 11 24H2 可能在未启用 SeDebugPrivilege 时返回成功但全部基址为 NULL；这必须是身份未知。若需临时启用该权限，仅在已提权 helper 内、检查确实授予后使用并恢复，不能默认为成功。[PSAPI](https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-enumdevicedrivers)、[模块路径](https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-getdevicedriverfilenamew)
6. 由已解析的系统路径打开文件，检查文件身份、安装位置／ACL、包关联、签名与 catalog 信任，以及完整文件 SHA256；整个事务保留该文件句柄，拒绝并发写／删除共享，不通过 PATH、当前目录或环境变量找文件。`WinVerifyTrust` 的结果与完整文件摘要分别记录；签名成功不能代替版本匹配，也不等于当前内存测量。[信任 API](https://learn.microsoft.com/en-us/windows/win32/api/wintrust/nf-wintrust-winverifytrust)

证据字段必须分开，缺失用 null 和具体错误，不伪填：

| 字段 | 能证明什么 | 不能写成什么 |
| --- | --- | --- |
| `disk_file_sha256` | 当前打开文件的完整字节身份 | 已加载内存 SHA256 |
| `loaded_module_metadata` | 当前模块枚举、路径与 PnP／service 的关联 | 磁盘与映像内容逐字节相同 |
| `load_time_provenance` | 已有可信加载记录或受控安装／完整重启的来源链 | 当前内存持续完整性测量 |
| `current_memory_measurement` | 首版为 null | 从前三项复制一个 hash |
| `runtime_acceptance_id` | 此型号／固件／驱动协议组合实际通过的测试范围 | 本次进程的每条 I/O 都成功 |

首版可接受的常规组合是：已验收 SupportContract + 本次精确固件 + 当前 PnP/SCM/PSAPI 对应关系 + 固定磁盘文件与签名 + 设备句柄绑定 + 新的严格 EC 读取。首次 Windows 验收应使用已记录的官方包／文件身份与**完整 Restart**后复核，消除已知的“磁盘已更新、内核仍加载旧版本”歧义。已有可信 CodeIntegrity 加载日志可作补证；事件缺失或仅有验证消息不能虚构加载成功。

若存在明确的新旧版本冲突、待生效替换或加载来源无法与当前节点解释一致，停止硬件写入并报告需重新检查／完整 Restart。不能用“仅磁盘摘要通过”降级放行，也不能要求普通用户安装额外监控软件才能使用已验收组合。已有 Sysmon DriverLoad/SHA256 等记录可附加，仍只能称加载时来源；不自动安装或改其配置。[可选 Sysmon 证据](https://learn.microsoft.com/en-us/sysinternals/downloads/sysmon)

这里不要求读取或直接 hash 内核映像：加载后的 PE 存在重定位、导入修补和可丢弃区段，整个内存范围也不等同于完整 `.sys` 文件。[PE 格式](https://learn.microsoft.com/en-us/windows/win32/debug/pe-format)

## 原生传输与完整字节事务

只有 helper 的固定 Fn 锁实现可以打开上述设备。原生层使用 `CreateFileW` / `DeviceIoControl`，不加载 ACPIDriverDll，不自动创建 service 或回退到另一个驱动。设备句柄在资格复核、读改写、回读及恢复期间持续持有。

首版沿用已观察的 4MiB 输入、4MiB 输出长度，使用确定初始化、长度检查和可失败分配；不得以驱动只检查非零为由缩成 4/5 字节，也不在真实设备上试探短缓冲。输入地址为小端 u32，写输入 offset 4 的数据按固定 byte 编码，其余输入确定初始化。为调用缓冲区实现独立的 guard/canary 检查，并在原生验收记录具体分配与输出变化范围。

ECRR 成功候选必须同时满足 Windows 调用成功、预期传输长度／guard 契约、完整小端 DWORD `<=255`，且没有未解释的输出区变化。地址原样回显、异常高位、截短、超长、guard 失败或调用失败均返回未知读数。记录长度与已允许前缀即可，不把 4MiB 原始输出或意外内核数据放进 UI／公共日志。

4MiB 大于 u16 DataLength，覆盖的是这处**目标**容量；原驱动 ACPI 局部输出只有 20 字节、数据起点 +16，源长度亦未验证。因此不能把大缓冲或用户侧 guard 写成驱动整体内存安全证明；Windows 的真实复制／异常行为仍是验收项。[既有细节回执](verification/windows-ec-abi.json)

Windows 私有事务的输入只允许 Fn Boolean，顺序固定：

1. 取得本应用跨会话 mutex，重新核资格并严格读取完整 byte B；根据完整资格、运行关联与 B 重新计算 binding，与预览一致才继续。
2. 计算 `T = (B & !0x10) | (target ? 0x10 : 0)`。若 B==T，只返回新的严格观察和无写 receipt。
3. 写前再次检查资格／句柄仍有效、完整 byte 仍为 B；变化即写前拒绝。只下发一次固定 ECRW，不自动重试写。
4. 即使 ECRW 返回成功，也必须再独立 ECRR 并确认**完整 byte==T**，才能报告应用已验证；不是只检查 Fn 位。
5. ECRW 一经提交，FALSE 或后续读失败也不能分类为写前拒绝。保留原始 Win32 错误及阶段，进入条件恢复判断。
6. 若资格或传输生命周期已改变、当前读数未知，或当前严格读回的完整 byte C 不是 B/T，不覆盖它，报告 state unknown。若可靠读回 B，可确认原状态仍在；若仍为 T，再次核对后只恢复本次拥有的旧字节，独立完整回读 B。已知恢复调用失败不能仅凭缓存／最终 Boolean 宣称恢复成功。

binding 是 SHA256 格式的版本化 opaque 值，绑定 contract、固件、设备／驱动关联、相应运行生命周期和**完整 B**。不把任意地址／IOCTL 编码成可由请求解释的字符串。内核基址等内部信息只用于 helper 的一致性检查，不向普通 UI 泄露。

固定 named mutex 仅串行化本应用 helper；验证其安全描述符，拒绝被非可信对象预占。`WAIT_ABANDONED` 后要求重新 Probe，不盲目还原旧事务。现有驱动单次调用虽然同步等待，但队列为 Parallel；mutex、句柄 share mode 和多次比较都不是 OEM 服务／固件的全局锁或原子 CAS。剩余 RMW 竞态必须在回读范围、错误和实机并发验证中明确，不承诺不存在第三方写入。

## 普通用户授权与 IPC

GUI/CLI 保持普通用户身份。构造、自动刷新和预览不启动提权；用户明确点击“检查 Windows Fn 锁（需授权）”或 Apply 才启动固定 helper。只读检查也走这条入口，因为设备读 IOCTL 的 access bits 要求写访问句柄。

安装位置固定为 Known Folder `ProgramFilesX64` 下 `Mechrevo\Console`，不用环境 `ProgramFiles` / `SystemRoot`、当前工作目录或命令搜索。GUI、CLI、helper 与安装清单均检查受保护位置、文件身份、签名／发行摘要；不能从下载目录或用户可写副本启动提权 helper。

启动契约：

- 固定 `mechrevo-console-oem-helper.exe --serve <nonce> <parent-pid> <parent-creation-time>`，nonce 来自 Windows CSPRNG；这些参数只作会话关联，不选择文件或硬件地址。helper 没有通用命令、shell、插件或驱动安装功能。
- 客户端先建立 `\\.\pipe\MechrevoConsole.Oem.v1.<nonce>` 单实例本地双向 pipe，显式 DACL、FIRST_PIPE_INSTANCE、拒绝远端连接。普通用户／管理员凭证授权两种 UAC 情形均要实测；不能使用 Everyone／匿名默认权限。逐项授予所需 pipe 权限，避免 FILE_GENERIC_WRITE 意外包含创建 pipe instance 权限。[管道权限](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-security-and-access-rights)
- `ShellExecuteExW(runas)` 仅执行固定文件，使用 `SEE_MASK_NOCLOSEPROCESS` 持有实际进程句柄。启动成功不等于 helper 结束；没有可确认的进程句柄时保留 lifetime unknown，不能声称已取消或可安全重开事务。[ShellExecuteEx 进程边界](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-shellexecuteinfow)
- 双方用 named-pipe peer PID、进程创建时间、安装映像身份及相应会话／token 检查对端，不能只检查随机名字或 JSON nonce。parent 核对连接者就是其持有句柄的 elevated helper；helper 核对 server 是受保护安装中的 CLI/GUI。helper 作为 pipe client 使用 identification 等级 SQOS，不能允许普通 server 模拟其高权限 token。[peer PID](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeclientprocessid)、[server PID](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getnamedpipeserverprocessid)、[CreateFile SQOS](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)

凭证式 UAC 的管理员 SID 可能不同，不能要求 parent/helper 同一用户 SID。helper 检查自身 elevation，parent 以实际启动的进程句柄、peer PID／创建时间和固定映像关联它；不要求普通 token 必须能读取另一个管理员的 token。管道 DACL／会话验证须允许已认证的这种授权方式，同时拒绝无关连接。

认证完成后才传请求：u32 小端字节长度 + UTF-8 JSON，单请求最大 32768 bytes、单响应最大 65536 bytes；长度先校验再分配，严格重复键／未知字段／版本／整数类型／必需 null／嵌套深度。一次 helper 只执行一个请求，不接受客户端给出的任意输出路径；结果通过 pipe 返回，不依赖 ShellExecute 的 stdout 重定向。

请求顶层固定 `schema_version`, `request_id`, `operation`。operation 只允许 `{"kind":"probe"}` 或 `{"kind":"apply","plan":OemPlan}`。后者只接受 id=`fn-lock`、FnLock、Boolean domain/previous/target、固定 source/readback scope 和由新检查生成的 binding；所有 metadata 由 helper 重建比较，不用于选择路径。禁止客户端传入 SupportContract、寄存器、IOCTL、根目录或驱动文件路径。

响应顶层固定 `schema_version`, `request_id`, `outcome`, `status`, `result`, `receipt`, `error`，nullable 字段必须显式出现。outcome 采用 `observed`, `applied_verified`, `rejected_before_write`, `failed_restored_verified`, `failed_state_unknown`。`error` 沿用 `{code,message,details}`，分别保留 primary、readback、restore、IPC 错误；按阶段／固定 code 判断，不匹配英文句子。receipt 保存本次写入是否尝试、完整字节验证和保护位保持结果，不能成为路径或下一次硬件命令输入。

exit 0 只对应无 error 的 observed/applied_verified；有明确失败的 terminal result 用 exit 2，响应发送失败用 exit 3。客户端同时验证实际进程退出、请求关联、响应 schema、类型化目标与结果、receipt 和 outcome。响应先成功到达但进程未确认退出，仍不能释放 writer/close 门禁。报文损坏／超限后仍有界排空并丢弃，或明确关闭 pipe 使 helper 按发送失败路径收尾，然后等待实际进程结束；不能停读却等待可能被满管道阻住的 writer。不得 timeout/kill 一个可能正在写入的提权 helper，也不自动重放旧 Apply。启动前明确 UAC 取消才是写前拒绝。

parent/pipe 消失后，helper 完成正在进行的硬件确认／条件恢复并退出；无法发送结果不能再开启第二次事务。退出后的 Fn 锁是普通持久设置，不引入风扇式租约，不因正常关闭自动切回。state unknown 在新成功 Probe 前阻止写入；lifetime unknown 还阻止新 helper 和关闭窗口。

## 公共 API 与接线

新增 `core::windows_oem`，其平台无关 DTO/codec/事务可离线测试，Win32 FFI 单独 cfg。建议冻结以下接口，避免和 Linux 的普通枚举／写入生命周期混用：

```rust
Client::new() -> Client                         // 纯构造
Client::cached_status(&self) -> Option<&Status>
Client::probe_authorized(&mut self) -> Update
Client::preview(&self, target: bool) -> Result<OemPlan, ControlError>
Client::apply_authorized(&mut self, plan: &OemPlan) -> Update
Client::helper_reaped(&self) -> bool
```

`Status` 包含 SupportContract ID／支持阶段、qualified、唯一可选 Fn `OemControl`、四类身份证据摘要、blocking_errors、diagnostics；不是温度采样，时间由调用方标“检查完成／收到状态时间”。`Update` 包含 `status: Option<Status>`, `outcome`, `result: Option<OemApplyResult>`, `receipt`, `error`, `helper_reaped`。复用 `OemControl` / `OemPlan` / `OemApplyResult` 的展示和目标类型，但共享 Boolean `transact` 不直接负责 Windows raw-byte 补偿。

NativeBackend 持久保存 Client；snapshot 仅合并缓存，不授权／开 EC。Windows CLI 在一次用户命令内先明确授权 Probe，再本地 preview，Apply 再显式授权。GUI 的新 Probe/Apply reply 保留 helper lifetime、未知状态和 receipt，不把结果压成丢失生命周期的普通 `Result`；共享单 worker／关闭门禁及其他预览冲突规则沿用现有控制器。Linux OEM 接线保持原行为。

## 最小文件与所有权拆分

| 负责人 | 文件范围 | 完整交付边界 |
| --- | --- | --- |
| A：核心协议／状态 | `console/crates/core/src/windows_oem.rs`、`windows_oem/{protocol,client,transaction,tests}.rs` | DTO、纯构造／缓存、严格关联、全字节事务、正反例；不编写 FFI stub 冒充传输。 |
| B：Win32 适配 | `windows_oem/native/{mod,identity,device,ipc}.rs` | 实际固件／PnP／SCM／PSAPI查询、设备 I/O、pipe/UAC/process RAII；仅在该层 unsafe。 |
| C：helper／前端 | 既有 `cli/src/oem_helper.rs` 的 Windows 分支；CLI、GUI 各自已有 owner | 固定 serve、Probe/Apply、真实 wait/lifetime、状态与预览/关闭门禁；不改 Linux `--apply-plan`。 |
| D：安装／验收 | `packaging/windows/{console.wxs,build.ps1,verify-installed.ps1}`、`console/resources/windows-fnlock-support.json`、相应测试 | 可安装 artifact、受保护 ACL／manifest／版本、固定支持数据、升级卸载和实机回执。 |
| 根代理：最终接线 | core `lib.rs` / `oem.rs` / `privilege.rs`、Cargo 依赖与 verifier 指纹 | 各分支合并、避免重复提权／fallback、统一构建与跨模块回归。 |

暂定文件名可在开始编码前统一调整；本轮只落本设计，不抢占正在修改的公共文件。Win32 API 绑定选一个 target-specific 依赖／薄 FFI 层并锁版本，不在调用点散落手写布局。验证指纹须覆盖支持清单、helper manifest、安装规则及真实测试 fixture。

## Windows 安装与维护边界

首版为 x64 per-machine 安装包，GUI/CLI asInvoker，helper requireAdministrator/uiAccess=false，路径按 Known Folder 固定。安装包只包含本项目构建的文件、许可证和支持事实／摘要，不打包原厂 `.sys`、DLL、恢复方法体、密钥或其他私有分析资产；已有官方驱动是前置条件。

安装验收必须检查实际安装路径、owner/DACL、文件／发行 manifest、helper 映像与完整版本，以及普通用户不能替换二进制、支持清单或控制输出位置。无可用签名／发行信任材料的开发包明确标为开发候选，不宣称正式签名发布。

默认安装／升级／卸载不安装、更新、启停或卸载 vendor 驱动，不停原厂服务，不修改其配置，不替用户接受第三方协议，不重置 Fn 设置。已有 writer 未退出时等待或拒绝维护，不用 Restart Manager／脚本强杀正在执行的 helper。升级后旧客户端／helper 版本不一致必须拒绝，不进行猜测兼容。

如另行安排驱动更新，必须独立记录真实行为：在线 PnP `/install` 可能立即卸载／重启设备，并非“先只安装、重启后才生效”。需要严格避免热重载时，已有正确包只核验后做完整 Restart；更换包的离线维护流程另外审核。不将 Fast Startup 关机当完整新内核启动。[Windows 安装行为](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/avoiding-system-restarts-during-device-installations)、[电源／启动状态](https://learn.microsoft.com/en-us/windows/win32/power/system-power-states)

## 必须通过的回归与真实证据

离线测试必须运行生产 codec、事务、资格判定与适配边界，不仅复制一个模型：

- 正向完整资格走到实际适配调用；逐项缺失／版本冲突／DSDT不符／重复PnP／PSAPI全NULL／旧绑定均零 EC 写。磁盘摘要相同但没有运行模块关联不能冒充 qualified。
- 固定路径／IOCTL／4MiB长度／小端偏移、分配失败、DWORD 0/255、原样 `0x074e`、错误高位／长度／guard；写回成功不得只依赖 DeviceIoControl TRUE。
- 全部 256 个初始 byte 的 on/off 掩码保护、no-op零写、预览后其他位变化、写前变化、读回第三方值、失败不盲重试、恢复失败／读失败／生命周期改变；保留原错误和 state unknown。
- UAC取消、启动失败、peer PID／创建时间／安装映像不符、管道预占／远端／错误ACL、duplicate／unknown／missing-null／超限／错误nonce、输出与exit矛盾、实际wait及丢失回包；未知 lifetime 不被新 Probe 清除。
- 普通 Probe 不写 EC；构造／自动刷新／本地预览不启动 helper；部分失败不被遥测刷新清掉；其他待确认预览不被覆盖；关闭／升级不杀 writer、不自动还原持久 Fn 设置。
- 安装 payload、权限／reparse拒绝、用户不能替换 helper、manifest错版、真实 Windows 目标编译；Linux 原有 OEM、CPU、风扇和亮度回归保持。

首次 Windows 实机回执需要逐项记录，不用 cross-compile 或 Linux 同机读数替代：

1. Windows 版本／架构，真实 SMBIOS 与原始 DSDT 摘要；当前 PnP 节点／INF／版本、SCM 状态、加载模块关联、磁盘文件身份和签名结果，各证据的边界与缺失字段。
2. 已有驱动安装及完整 Restart 的来源链；当前版本替换／重启待定状态。已有可信 CodeIntegrity 记录保留原事件关联，不能凭日志标题猜加载成功。
3. 普通 token 直接读权限结果、UAC取消、同账户管理员和凭证式授权、实际受限 helper 的安装身份、pipe peer 与真实退出；不能只测管理员终端。
4. 严格只读 ECRR 的完整 DWORD、实际长度／guard／输出变化范围，与 Fn 当前状态一致；不向真实驱动发送短缓冲、畸形地址或故障探测。
5. 先保存完整原始 byte，on/off可逆写与独立完整回读；物理 Fn 行为单独验证。结束必须确认完整原始 byte 恢复，附各阶段／恢复失败材料，不隐藏错误。
6. 正常原厂软件／快捷键的并发情形、睡眠／恢复或设备重建后旧计划拒绝、新 Probe 恢复；确认没有误改共享位。重启持久化另列，不能从本次 EC bit 回读推定。
7. 安装包／已安装 artifact／运行 helper 的身份分别验证；填入版本化 SupportContract 的接受范围。未知或失败项不提升为 runtime_accepted，也不扩大到别的机型／BIOS。

完成这些之后才把该组合写成真实 Windows OEM 可写支持；在此之前准确称“完整实现的受限候选，Windows 实机验收待完成”。
