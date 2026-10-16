# Windows OEM 方法体恢复进度

2026-10-16 已从原始 5.17.51.34 服务静态恢复方法表中的全部 **5,183 个 CIL 方法体**。
CIL 是 .NET 的中间指令；这次取得的是可逐条解码的真实指令，不再只是共享异常桩。
布局判定、CPU/GPU 转速公式及 RamFan1p5 曲线写入顺序已经追到实际分支。
恢复代码仍有调用动态 IL 的包装方法；之前将入口 `0x0600182f` 称为 VM 分派的判断已纠正，
它创建并缓存 DynamicMethod，与 native netvm 是不同路径。动态载荷还在逐项分析，
这不等于全部保护逻辑已还原，更不等于 Linux 完整移植。
全程只分析文件数据，没有加载或执行 OEM 程序、native 模块或恢复后的方法。

原服务共有 8,702 个 MethodDef 元数据行，其中 6,841 个有非零 RVA：
5,183 个属于已恢复保护表，另 1,658 个原文件方法体不在该表中，不能据此声称已审计完。
1,861 个零 RVA 行含声明/导入，不能简单视为缺失实现。
客户端、native/驱动、嵌套配件包、剩余 VM 和 Linux 界面等价功能还需逐项分析；
已建立 28 类行为清单，但不声称穷尽所有功能。详见 [完整功能覆盖](OEM-FEATURE-COVERAGE.md)。

## 锁定分析对象

本页 token/RVA 全部针对 5.17.51.34 安装包提取出的原始 `AiStoneService/MyControlCenter/GCUService.exe`：

- 长度：14,447,864 字节。
- SHA-256：`a2b7d81ecf5c1dbdfc19738f17ba5e01e884d1b3688beb668858727307e2a318`。
- CLR flags：`ILOnly`；`ManagedNativeHeaderDirectory.RelativeVirtualAddress = 0`，没有 ReadyToRun/native-image header。
- `.local-tests/windows-oem/service-il.txt` 是旧的分析材料，其中的公共桩 RVA `0x672d50` **不能套用到 51 版**。
- 既有 `de4dot51.exe` 的 SHA-256 为 `408cde6dda3973129d505110009581930bfd2080b7bbf350f14c53851af91674`。其重写 RVA 也不能当作原始 PE RVA。

## 原始 MethodDef 核对

| 方法 | 原始 token | 原始 RVA |
| --- | --- | --- |
| `MyECIO.AcpiCtrl.Read` | `0x060005b4` | `0x007a5aa8` |
| `MyECIO.AcpiCtrl.Write` | `0x060005b5` | `0x007a5aa8` |
| `MyECIO.AcpiCtrl.CustomRW` | `0x060005be` | `0x007a5aa8` |
| `MyECIO.MyEcCtrl.GetProjectIdFromEC` | `0x060005ca` | `0x007a5aa8` |
| `MyECIO.MyEcCtrl.IsSuportRamFan1p5` | `0x060005da` | `0x007a5aa8` |
| `MyControlCenter.FanInfo.GetEcCpuFanRpm` | `0x06000d4b` | `0x007a5aa8` |
| `MyControlCenter.FanInfo.GetEcGpuFanRpm` | `0x06000d4c` | `0x007a5aa8` |
| `<Module>..cctor` | `0x06000002` | `0x007a5818` |

RVA `0x7a5aa8` 映射文件偏移 `0x7a3ca8`，12 字节（含 tiny header）为 `2e72b7a80270736305000a7a`。其 IL 长度 11：加载 `Runtime exception` 字符串、构造 `System.Exception`、`throw`。所有上述关键方法共享它，原始磁盘桩本身没有可据以计算 RPM、选择布局或消费 SMRW 的分支；真实方法位于另一个受保护记录表中。

`NoInlining`、原始模块初始化逻辑、内存保护/分配函数和 VM 解释器共同说明它需要保护层在运行时接管；不应把 `throw` 理解为厂商功能本来不可用。既有静态 de4dot 检测为 .NET Reactor，NETReactorSlayer 日志却明确报告 `Couldn't find any encrypted method`。工具保存文件成功不等于这些方法恢复成功。VM 的存在也不证明以上每个目标方法都采用同一种保护方式。

## 本轮新增恢复结果

只读取原始 PE，按保留的初始化代码逐步追踪：

1. `FieldDef 0x040025ad` 的 FieldRVA 为 `0x799688`，64 字节数组提供 loader 配置。配置偏移 `+0x0c` 的 RVA 指向 `0x752e68`。
2. 此处是 packed 56 字节 native 描述记录；前两项分别引用 payload RVA `0x663bd8` 和 `0x6c1600`。第三项为结束记录。
3. 保留的 managed loader 使用 RC4 的标准 KSA/PRGA 解密 payload。分析脚本现场读取原始 PE 内的参数，没有把密钥写入本文或仓库。
4. 解出的数据是 OEM 自定义的紧凑 native image：两者开头均声明数据偏移 346、machine `0x8664`、7 个节、PE32+ optional magic。它们不是直接以 MZ 开头的标准 DLL。后续已按 managed loader 的字段映射、逐节解密和边界检查生成静态分析用 PE；其合成头和节名不是原厂原件，不应执行。

| 原始 payload RVA | 解出长度 | 明文 SHA-256 |
| --- | --- | --- |
| `0x663bd8` | 195,930 | `7a2365e9e8f2560e6da01c79b6e3665a466b2f3afcd7e866179f648aa9ebe017` |
| `0x6c1600` | 126,298 | `88df1c3073e34a987887d4a5f3ddef1f0a22fbfe1ec98078a0b107430a923470` |

保留代码包含 native 入口 `vb_onload`，以及 `netvm` 下的 `vb_dn_vm_idx`、`vb_dn_vm_dec`、`vb_dn_vm_next`。对应 VM 注册入口原始 MethodDef 为 `0x06001ca2`，RVA `0x7a8430`；主要 loader 初始化入口 `0x0600196c`，RVA `0x7a1b38`。这些是进一步解析 token 到 VM bytecode 映射的入口，尚不是映射本身。

## 按 token 恢复的真实方法表

第一个 native 描述项是 `jithook`，第二个才是 `netvm`。真实方法恢复不需要先完成 VM 解释器解析：
`jithook` 的恢复函数从共享桩的 IL 末尾取得 `FOAP` 标记及有符号相对位移，
定位原 PE 的 `RVA 0x76cc60`、`FWSN` 方法表。表包含 5,183 个唯一且有效的 MethodDef token，
每条记录 24 字节，给出对应加密方法体的位置和长度。不是扫描偶然出现的 token 字节，
也不是把共享 RVA 当成唯一方法记录。

恢复工具从 native 指令中的参数现场导出解密参数；仓库和本文不保存密钥。
所有记录均通过独立的 OEM `VBPD` 尾标记检查；错误参数和损坏密文的负例会被拒绝。
结果包括 1,742 个 tiny、3,441 个 fat 方法头。2,123 个非零本地变量签名 token
均能解析到原元数据的 StandAloneSig 本地变量签名。
逐条解码共 328,357 条指令；20,342 个显式分支或 switch 目标全部落在有效指令起点，
没有越界或错误目标。以上是静态格式和边界核验，**不是 CLR 栈/类型验证或 Windows 运行测试**。
记录的附加签名字段还没有完整语义说明，不能把它直接当作方法内偏移。

| 已恢复目标 | 原始 token | 已确认的实际行为 |
| --- | --- | --- |
| RamFan1p5 支持判定 | `0x060005da` | 读取 EC `0x078e`，检查 bit 6 |
| CPU 转速方法 | `0x06000d4b` | 高字节 `0x0464`、低字节 `0x0465`，拼成 16 位值 |
| GPU 转速方法 | `0x06000d4c` | 高字节 `0x046c`、低字节 `0x046b`，拼成 16 位值 |
| RamFan1p5 表写入 | `0x060017a4` | 每只风扇 16 项 duty；双表共 94 次字节写入 |

拼接公式为 `(hi << 8) | lo`，其中 `|` 是按位或；没有额外缩放、型号分支或裁剪。
间接 delegate 已解析到 `Object.GetType`、`MemberInfo.Name` 和字节转换；
`AcpiCtrl.Read` 忽略传入的名称，不能根据该字符串猜另一个 EC 地址空间。
GPU 低字节明确为 `0x046b`，没有把未经本机验证的 `0x046d` 套进来。
但源码中的 RPM 名称和换算仍不能替代独立机械转速校准。
`0x060019f6`、`0x06001c9b` 等恢复结果还调用动态 IL 构建器；后续已恢复其内层正文。
其中 `0x060019f6` 的正文才是 native VM 取指桥，`0x06001c9b` 是数据读取器，不能混称 VM 执行器。

本机新十组只读样本均为 `0x078e = 0xec`、`0x0740 = 0x10`。
结合恢复后的工厂选择分支，正常读取成功时会选择 `MyFanManager_RamFan1p5`
及 `FanTable_Manager1p5`。同组 OEM 公式字值为 CPU 3,816–3,878、GPU 4,939；
不是独立转速仪结果，也没有在本机运行 Windows 选择函数。
表写入、失败处理及 Linux 实施边界见 [原厂策略迁移](OEM-MIGRATION-NEXT.md)。

## DynamicMethod 与字符串解码的新证据

原始 managed IL 已确认 `0x0600182f` 经方法构建器生成、缓存 DynamicMethod，
其包装调用使用实际 `MethodBase.Invoke`，不是 native netvm context。
字符串入口 `0x0600196f` 的实际 75 字节帧已经解出，含 12 字节方法头及 63 字节 CIL，
共 35 条指令；不能把 75 字节全部当作裸 IL。独立有界静态模型对 72 组用例核对算术与边界，
覆盖空值/空串、UTF-16 单元、回绕及长度边界，没有执行 OEM 或 CLR DynamicMethod。

从原始 `#US` 堆已静态恢复 3,320 个唯一字符串，关联原始/恢复 IL 中 10,965 处
相邻 `ldstr` 与该解码器调用。此计数范围不含尚未证明的其他数据流或全部配置使用，
也不代表每个字符串对应的功能已移植。完整语料、OEM 指令和解密参数仅留私有证据。
六个已定位动态包装载荷已全部恢复，长度为 19/75/1841/27/608/87 字节，两个 tiny、四个 fat。
独立校验代码/跳转/元数据/本地签名/异常区间；17 个损坏数据负例通过。
它们对应原有六个包装方法的内层正文，不把恢复方法总数算作 `5183 + 6`。
`0x060019f3` 建 opcode 处理器字典，`0x060019f4` 查表调用，`0x060019f6` 调用 native dec
取得 opcode/operand 记录并缓存；`0x06001c9b` 读取指定 RVA 数据并可选 RC4。
取指桥已定位不等于所有 VM 程序、动态方法、客户端或 native 行为已完整还原。

## 功耗/TCC 方法恢复的新增结论

选中 RamFan1p5 manager 的 Intel 路径已追到真实 profile 应用分支：Custom 模式
把 profile PL1/PL2/PL4 写到 `0x0783..0x0785`，其他三模式传入零作为默认/覆盖请求。
所有模式按 profile 开关决定向 `0x0786` 写入带 bit 7 的 TCC offset 或零。
零不等于实测 0W，字节转换和吞错也不证明固件已接受请求。
不能把 base manager 的 PL4 换算或忽略 Tau 的方法套入选中的 manager。

动态默认 getter 已与本机十五个固定地址的三组只读样本对应：
Gaming 的 PL1/PL2/PL4/DState 为 75/75/200/1、TCC 为 5；
Office 为 45/45/200/1、TCC 为 20；Turbo 为 130/130/200/1、TCC 为 5。
这些只是按恢复后字段标签解释的原始默认字节，不是运行中的 RAPL 限值、
温度限制执行结果或安全功耗边界。本轮探针已移除，原模块/PID/配置保持，无新增受监测故障，
没有为此切换原厂性能模式或写入功耗/TCC。完整字段地址、manager 边界及待验收条件见
[功耗模式证据](OEM-MIGRATION-NEXT.md#原厂功耗模式的真实分支与本机默认字节)。

## 电源事件与睡眠恢复的静态边界

原元数据已闭合 OS 注册、delegate 和异步状态机之间的关联。
普通 PowerModeChanged 处理器只处理电源状态变化；默认另一条通知路线注册的是显示状态，
不是实际挂起/唤醒。配置选择的替代路线包含 Resume 通知，但不区分 S3/S4。
恢复路径有 3 秒和 2 秒 `Thread.Sleep`，并会重放 profile 与排队曲线；
同步方法返回不保证异步曲线提交完成，也不能据显示关闭推断风扇已回自动模式。

源码允许已排队曲线在服务停止后再次使能：已检查路径没有队列排空/代次或停止状态检查。
这不是 Windows 运行复现，也未证明外层所有同步条件；Linux 无须复制此行为。
真实 OS 常量、完整事件链及 Linux 待验收边界见
[事件覆盖说明](OEM-FEATURE-COVERAGE.md#电源与显示事件的新证据)。

## 复现与证据

维护工具及用法见 [Windows 静态工具](WINDOWS-STATIC-TOOLS.md)。工具固定原始来源哈希、
拒绝覆盖已有输出目录，恢复方法需显式选择 `--recover-methods`；输出只留在本地。
不能把恢复结果作为可执行 OEM 程序安装，不能将 OEM 指令、完整方法体、密钥或原厂二进制上传到仓库。

私有证据位于 `.local-tests/postboot-20261016/oem-binding/`：

- `mapping/FINDINGS.md`、`descriptor-receipt.json`：恢复入口、描述项、方法表定位及边界。
- `mapping/all-record-receipt.json`：全部记录的位置、token、密文/明文 hash 和独立尾标记结果。
- `mapping/il-boundary-receipt.json`、`locals-check.txt`、`negative-recovery-checks.json`：指令边界、本地签名及损坏负例。
- `fan-layout-body/layout-semantics.json`、`control-semantics.md`：布局分支、16 项曲线、启用及生命周期调用。
- `rpm-bodies/RPM-ANALYSIS.md`、`rpm-static-receipt.json`：delegate、地址、公式及失败路径。
- `hardware/hardware-receipt.json`：十组实机只读样本、探针移除、原服务/模块/配置保持及 GPU 健康核验。
- `power-semantics/FINDINGS.md`、`evidence.json`、`mode-matrix.json`、`defaults-receipt.json`：实际功耗模式分支、逐方法来源 hash 和十五个默认字节的三组实机只读核对。
- `hardware/rpm-trial-receipt.json`：Linux 只读 RPM 临时加载/读取/恢复验证；包安装状态另行记录。
- `mapping/feature-coverage/`：服务方法与安装文件/嵌套包清单、28 类功能、manager 和本机支持边界。
- `fan-layout-body/sleep-events/SLEEP-EVENTS.md`、`metadata-links.json`、`event-delegate-records.json`：OS 注册、事件身份和异步状态机的静态绑定证据。
- `managed/string-decoder/string-decoder-receipt.json`、`static-model-receipt.json`、`corpus-receipt.json` 及 `native/string-decoder/`：DynamicMethod 构建路径、实际帧、静态模型和私有字符串计数。
- `power-semantics/setting-gates/`：OEM 51 充电优先级固定关闭；EFI 188/180 字节差异及不具格式资格的前缀只读结果。

早期 PE 元数据、native 解密与重建证据继续保留于
`.local-tests/postboot-20261016/thermal-iterate/oem-body-audit/`。
以下 native 首段追踪是此前恢复阶段的记录；其当时停止位置不再表示方法恢复的当前阻断。

## Native header 和节映射已重建

原厂 compact loader 的两层处理已分别复现：外层 RC4 取得 compact image；再根据 `94 + 36 * 节数` 的节描述表，对每个节独立 RC4 解密。每项记录的五个整数依次是 RVA、虚拟长度、原始长度、compact 内偏移、节属性，随后是 16 字节节参数。脚本校验完整长度、原始范围、映像范围、节重叠和 64 位 PE 类型后才输出。

`rebuild_native.py` 生成标准 PE 容器，使 `pefile` 和 `objdump -x/-d` 可以解析。保留了真实节内容、RVA、entry、image base、节属性和前六项 data directory；后续目录中的零值为合成值，不能据此推断原厂 CFG/load-config 缺失。DOS/PE 头摆放、节名、文件对齐和 raw offset 是供分析使用的合成值。没有执行入口、加载依赖、绑定 imports 或应用运行时 relocation，不能声称得到可运行的原厂 DLL。原厂 loader 运行时还会把多个真实节压成一个合成节记录；本工具为了静态工具可见性保留了七个节，这也是有意的分析差异。

| 来源 | 分析 PE SHA-256 | 确认的 exports（RVA） |
| --- | --- | --- |
| `native-663bd8.compact.bin` | `4ec5c2b70aa2a0ddac5d13ea45fc65d47ffe898eacc7f55ff775731d1209e5ec` | `vb_onexit 0x1020`，`vb_onload 0x1030` |
| `native-6c1600.compact.bin` | `57ae870348dbf72c597b75940a74c93417023ad9efb09fa85ed41ccd2016d0ce` | `vb_dn_vm_dec 0x1020`，`vb_dn_vm_idx 0x1200`，`vb_dn_vm_init 0x1270`，`vb_dn_vm_next 0x12c0`，`vb_onexit 0x1330`，`vb_onload 0x1340` |

两者的 import/export 和 9 个 relocation block 均可解析。第二个 image 的 unwind 信息在 `0x18c90` 触发 pefile 的 chained-function 解析警告，已原样保留在回执，未通过修改字节消除警告。

第二个 native 模块的初始化数据来自原 PE `RVA 0x717210`，外层 flags 为零，managed loader 不对这块数据解密。真正传入 `vb_onload` 的结构在 `0x717220`，有 `JNTM` 标记。结构 `+8` 指向 `RVA 0x6ad8e0`，`+12` 指向 `RVA 0x6edb30`；native onload 确认会读取后者的 state，并从前者遍历范围、长度及校验值。这个表目前确认的是完整性检查输入，**尚不能称为方法 token 到 VM bytecode 映射表**。

`vb_dn_vm_idx` 等入口跳入另一个代码节的栈/返回地址转换代码；线性反汇编把未执行的内嵌字节解释成大量 bad 指令。已从真实指令核对一段入口路径：`0x1200 -> 0x20640`，其中自取指令地址和栈转换使控制转到 `0x20740`，随后回到 `0x1210`。仍须逐段证明控制流，不能凭固定 call 长度跳过任意字节，也不能把这些数据直接认作 VM opcode。

新增证据：`rebuild-receipts.json`（合成差异、全文件/逐节 hash、imports/exports、解析警告），`*.analysis-headers.txt` 和 `*.analysis-disassembly.txt`。重建命令：

```sh
python -B .local-tests/postboot-20261016/thermal-iterate/oem-body-audit/rebuild_native.py
```

还独立按 `vb_onload` 的反汇编实现了范围表解码和 XOR/ROR 校验（`verify_native_integrity.py`），在原 PE 的映射数据上三项均精确通过：`RVA 0x6edb48 / 7,175,864 bytes -> 0x19b3fbc6`、`RVA 0x2000 / 6,994,144 bytes -> 0x8d0b5d9b`、`RVA 0x6adcf8 / 261,688 bytes -> 0xf1b611d6`。预期值来自原厂范围记录，结果不是根据脚本自身输出再设定。回执为 `native-integrity-receipt.json`。这确认 loader 数据和 native 校验算法解析正确，仅验证 native 完整性算法；目标方法体的当前结果见上文。

## 早期 vb_dn_vm_idx 首段可达控制流（历史记录）

本轮进一步对单个 `vb_dn_vm_idx` 入口做了静态栈和常量传播，读取机器码作为数据分析，没有加载 OEM 模块、调用函数或借助 CPU 仿真器执行原厂指令。分析脚本 `trace_idx_static.py` 对来源 hash 固定，最多 500 条指令；遇到未知操作或非恒定分支就停止，不能跨边界猜测。磁盘里的函数指针值不会当成完成运行时绑定后的真实地址。

实际追踪了 138 条可达指令。重要控制流（均为第二个 native image 的 RVA）：

```text
0x1200 CALL 0x20640
  0x20652 CALL 0x20657      ; 自取指令地址
  0x206b7 RET  0x20740     ; 栈上的常量返回目标
  0x20788 JMP  0x1210
0x1217 CALL 0x20b30
  0x20b45 CALL 0x20b4a
  0x20bc7 RET  0x20e10
  0x20e18 CALL 0x20e1d
  0x20e7d JMP  0x177f
0x177f JMP QWORD PTR [RIP + 0xeac3]  ; 读取 RVA 0x10248，停止
```

首段的真实效果已经缩小：入口 `RCX` 保存到 `RDI`，入口 `RDX` 保存到 `RBX`；`RCX` 改为 native `RVA 0x10310` 的地址，随后通过 `RVA 0x10248` 的运行时函数指针转出。此时栈指针相对函数入口为 `-48`，包含为外部调用保留的返回地址。该首段追踪当时尚未确定两个输入参数的业务含义、间接函数的身份和返回行为，不能把 `0x10310` 命名为索引表或把这一步说成已完成 token 查询。

该阶段的停止位置是 `RVA 0x177f` 的机器码 `ff25c3ea0000`。后续已确认 `RVA 0x10248` 不属于 imports，磁盘初值指向模块内的返回桩，并继续解析了 idx/next/dec 的条件路径；未证明的运行时改写仍须保留限制。方法体实际来自第一个 jithook 模块的恢复表，不能强套第二个模块的 VM 索引模型。

`idx-static-trace.json` 保留每条指令的来源 RVA、机器码、栈偏移、常量分支和引用；`idx-static-summary.json` 是有界摘要。脚本另外逐条与 `pefile.get_data` 比较机器码，并断言保留输入、地址形成和最终停止位置，防止反汇编行截断或错误栈传播悄悄通过。复现：

```sh
python -B .local-tests/postboot-20261016/thermal-iterate/oem-body-audit/trace_idx_static.py
```

本轮原始51版服务、静态脚本及回执已逐文件核验并归档到
`~/Backups/projects/mechrevo-drivers/light/windows-oem-analysis-20261016/`；
`manifest.json`记录SHA-256、权限、所有者与原路径。分析映像不可作为驱动安装或执行。
