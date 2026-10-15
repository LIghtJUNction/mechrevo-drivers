# Windows OEM 方法体恢复进度

2026-10-16 的静态核验已经确认：关键方法在磁盘里的 IL 真的是同一个异常桩，不是 C# 反编译器丢失逻辑后补出的文本。目前已恢复原厂保护层的两个 native payload，并按 loader 字段重建为静态分析用 PE；尚未恢复布局判定、RPM 换算和风扇表消费方法的真实指令，不能据此认定完整移植或推导 EC 写入。

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

RVA `0x7a5aa8` 映射文件偏移 `0x7a3ca8`，12 字节（含 tiny header）为 `2e72b7a80270736305000a7a`。其 IL 长度 11：加载 `Runtime exception` 字符串、构造 `System.Exception`、`throw`。所有上述关键方法共享它，原始 IL 中没有可据以计算 RPM、选择布局或消费 SMRW 的分支。

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

## 复现与继续恢复

本机证据目录：`.local-tests/postboot-20261016/thermal-iterate/oem-body-audit/`。

- `Program.cs` / `audit.csproj`：用 .NET `PEReader` 与 `MetadataReader` 读取 MethodDef、FieldRVA、CLR header；没有使用 `Assembly.Load`、反射调用或执行原厂入口。
- `metadata51.txt`：原始 51 版元数据；控制字符名称保持原样，按 token/RVA 查找，不依赖渲染后的名称。
- `service51-il.txt`：用 `ilspycmd -il ORIGINAL_EXE` 生成的原始 51 版 IL。
- `service51-static-il.txt`：既有静态 de4dot 产物的 IL，仅用来阅读简化控制流；偏移和结构必须回原 PE 交叉核对。
- `extract_native.py`：先验证原 PE hash，再以只读 PE 数据和自行实现的 RC4 提取两个紧凑 image。
- `native-receipts.json`：尺寸、格式和 hash；`native-*.compact.bin` 是本地证据，未提交。

本地复现：

```sh
dotnet build .local-tests/postboot-20261016/thermal-iterate/oem-body-audit/audit.csproj --no-restore
dotnet .local-tests/postboot-20261016/thermal-iterate/oem-body-audit/bin/Debug/net10.0/audit.dll ORIGINAL_EXE
python -B .local-tests/postboot-20261016/thermal-iterate/oem-body-audit/extract_native.py
```

下一步应从已重建的 native PE 继续只读解析 `vb_onload` 如何安装方法处理器、`vb_dn_vm_idx/dec/next` 如何取得映射。分别对 `0x060005da`、`0x06000d4b`、`0x06000d4c` 找到唯一记录、边界和可解码指令，保留映射及解码前后 hash；不能根据明文里偶然出现的 token 字节宣称已定位。若目标走 native 编译或另一个解密路径，应记录实际路径而不是强套 VM 模型。

只有真实分支、输入来源、边界、单位和失败路径都可追踪，才可称某个 OEM 方法已恢复。之后仍需与本机 ACPI/EC 实测交叉核对；二进制恢复本身不授权写未知寄存器，也不能代替硬件验收。

## Native header 和节映射已重建

原厂 compact loader 的两层处理已分别复现：外层 RC4 取得 compact image；再根据 `94 + 36 * 节数` 的节描述表，对每个节独立 RC4 解密。每项记录的五个整数依次是 RVA、虚拟长度、原始长度、compact 内偏移、节属性，随后是 16 字节节参数。脚本校验完整长度、原始范围、映像范围、节重叠和 64 位 PE 类型后才输出。

`rebuild_native.py` 生成标准 PE 容器，使 `pefile` 和 `objdump -x/-d` 可以解析。保留了真实节内容、RVA、entry、image base、节属性和前六项 data directory；DOS/PE 头摆放、节名、文件对齐和 raw offset 是供分析使用的合成值。没有执行入口、加载依赖、绑定 imports 或应用运行时 relocation，不能声称得到可运行的原厂 DLL。原厂 loader 运行时还会把多个真实节压成一个合成节记录；本工具为了静态工具可见性保留了七个节，这也是有意的分析差异。

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

还独立按 `vb_onload` 的反汇编实现了范围表解码和 XOR/ROR 校验（`verify_native_integrity.py`），在原 PE 的映射数据上三项均精确通过：`RVA 0x6edb48 / 7,175,864 bytes -> 0x19b3fbc6`、`RVA 0x2000 / 6,994,144 bytes -> 0x8d0b5d9b`、`RVA 0x6adcf8 / 261,688 bytes -> 0xf1b611d6`。预期值来自原厂范围记录，结果不是根据脚本自身输出再设定。回执为 `native-integrity-receipt.json`。这确认 loader 数据和 native 校验算法解析正确，仍不证明目标方法体已恢复。

## vb_dn_vm_idx 首段可达控制流

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

首段的真实效果已经缩小：入口 `RCX` 保存到 `RDI`，入口 `RDX` 保存到 `RBX`；`RCX` 改为 native `RVA 0x10310` 的地址，随后通过 `RVA 0x10248` 的运行时函数指针转出。此时栈指针相对函数入口为 `-48`，包含为外部调用保留的返回地址。两个输入参数的业务含义、间接函数的身份和返回行为尚未确定，不能把 `0x10310` 命名为索引表或把这一步说成已完成 token 查询。

精确阻断是 `RVA 0x177f` 的机器码 `ff25c3ea0000`。继续恢复需要先追踪 `RVA 0x10248` 的写入/绑定来源，证明该调用的参数和返回约定，再继续它返回后的可达路径；不能把原文件中未绑定的指针当零地址继续运行。

`idx-static-trace.json` 保留每条指令的来源 RVA、机器码、栈偏移、常量分支和引用；`idx-static-summary.json` 是有界摘要。脚本另外逐条与 `pefile.get_data` 比较机器码，并断言保留输入、地址形成和最终停止位置，防止反汇编行截断或错误栈传播悄悄通过。复现：

```sh
python -B .local-tests/postboot-20261016/thermal-iterate/oem-body-audit/trace_idx_static.py
```

本轮原始51版服务、静态脚本及回执已逐文件核验并归档到
`~/Backups/projects/mechrevo-drivers/light/windows-oem-analysis-20261016/`；
`manifest.json`记录SHA-256、权限、所有者与原路径。分析映像不可作为驱动安装或执行。
