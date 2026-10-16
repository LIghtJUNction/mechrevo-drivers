# Windows OEM 静态提取工具

`scripts/windows_oem_static.py` 把已核验的 v5.17.51.34 `GCUService.exe` 内两个 compact native 载荷提取成维护分析材料。它只做读取、RC4 数据转换和标准容器构造，不启动、加载或执行 OEM 代码，不操作硬件，也不属于驱动安装包。

```sh
python -B scripts/windows_oem_static.py --source /明确路径/GCUService.exe --output /新的目录
```

源文件必须是 SHA256 `a2b7d81ecf5c1dbdfc19738f17ba5e01e884d1b3688beb668858727307e2a318`。没有绕过 hash 的 CLI；其他版本需要先独立调查格式。`pefile` 是已有环境依赖，缺失时明确报错，工具不会安装依赖。输出父目录须存在，输出目录本身必须不存在并以700权限创建；既有目录、文件或符号链接均不覆盖。源 hash 和两个载荷解析通过后才创建目录。

每个载荷输出 compact 原始明文和 `.analysis.exe` 静态分析容器；这个后缀只为分析器识别 PE。**它不是可运行 DLL，不是字节相同的原厂 PE，也不证明完整原始控制流已恢复。** 保留提取节内容、RVA、image base、entry、characteristics、前六项 data directory。DOS/PE 头布局、未表达的头字段、节名、4096/512 对齐、文件偏移和 padding、后十项清零 directory 是 synthetic，manifest 明确记录。密钥从输入中读取，不写入回执或仓库。

`manifest.json` 记录源/compact/analysis/每节 SHA256、实际 exports/imports、relocation 数量和 pefile 原始 warnings。warning 不隐藏；零 warning 也不证明可执行或完整还原。`complete=true` 仅表示本次两个输出均写完。异常退出保留不完整目录和 `complete=false` 回执；如果创建首份回执前失败，可能没有 manifest，任何此类输出都不视为完成，不自动删除材料。

边界检查包含真实文件长度、唯一且完整文件支持的 RVA 映射、源和 compact 的节 raw/virtual 重叠、32 位整数范围、section table 和 data directory 范围、64 MiB 源/1,000,000 字节 compact/16 MiB image 上限。这里只支持已观察的六目录布局；非零 security directory 因其语义是文件偏移被拒绝。

回归测试使用自行生成的小型 compact 结构，不包含 vendor 二进制。RC4 已对照 [RFC 6229 第 2 节](https://www.rfc-editor.org/rfc/rfc6229.html#section-2) 的 key 1、40 bit、offset 0 和 256 向量。RC4 在此仅用于还原原厂历史格式，不用于安全存储。运行：

```sh
PYTHONDONTWRITEBYTECODE=1 TMPDIR=/已选定临时目录 python -B -m unittest discover -s tests -p test_windows_oem_static.py
```

合成回归不代替真实源验收；真实源需在全新目录运行，并分别对照冻结的 `native-receipts.json` 和 `rebuild-receipts.json` 两个 compact 与两个 analysis SHA。OEM 输出留在本地调查目录，不提交到源码仓库。


## 可选 FWSN 方法体恢复

```sh
python -B scripts/windows_oem_static.py --source /明确路径/GCUService.exe --output /另一个新目录 --recover-methods
```

默认不恢复方法体，原来两个 native 输出保持相同。显式选项在同一固定源 hash 下读取 JIT stub 的 FOAP 相对指针和 FWSN 表；逐条校验 token 唯一、MethodDef RID 在原 CLR metadata 表中存在、signed relative 与 fallback RVA 一致、密文完全由源文件支持。RC4 key 从第一个 native analysis 的四条 `mov` 指令立即数读取并校验指令形式，不在源码或 manifest 保存 key。

真实源有 5183 条记录，输出 `methods/0600xxxx.recovered.bin`。manifest 的 `method_recovery` 记录表及 key 指令来源、原 MethodDef 行 RVA/方法 RVA、每条 record RVA、密文及明文 SHA256、tiny/fat 头、CodeSize、local signature token、EH section 尺寸、尾部 padding 和 VBPD 标记。FWSN 的 `local_signature_offset` 与 `extra` 保留原字段，不猜测其含义，前者不是已证明的 body-relative offset。

方法体检查只证明头和字节边界、EH 区段尺寸及 MethodDef RID 对应关系。**没有宣称 CLR stack verification、完整 CIL 语义校验、恢复原程序集或恢复签名。** Opcode/branch 验证由独立 .NET 工具产生另份回执。恢复前先检查所有记录，异常不创建输出目录；写入异常仍保留 `complete=false`，不自动删材料。成功回执需与冻结 `all-record-receipt.json` 逐 token 比较全部 5183 个明文 SHA，不能仅凭合成测试声称完整恢复。


## 可选六个 DynamicMethod 内层恢复

```sh
python -B scripts/windows_oem_static.py --source /明确路径/GCUService.exe --output /又一个新目录 --recover-dynamic-methods
```

此开关独立于 `--recover-methods`。内部恢复并核验 FWSN wrapper 来源，但默认只落盘两个 native 容器及六个内层 body/cipher/适用的 external local signature，不写整套 5183 外层。同时指定两个开关才写两套。六个内层对应已有 wrapper，不能计为 5183 之外六个新方法。

资格链为固定原源 SHA → FWSN wrapper 精确 SHA → 已核验 builder `0600182f` 参数描述符（RVA、长度、call offset、local 来源和 discriminator）→ 密文及内层精确 SHA。描述符仅包含来源数值和哈希，没有 vendor IL。整组六条canonical JSON还绑定独立冻结SHA，录错call offset/discriminator会拒绝。工具另外从实际wrapper bytes执行有限的静态prefix解释：只处理已识别的常量、符号参数、locals、算术和已知条件分支，最多1000步，核对真实call offset、call token `0600182f`、current-method及末七项实际RVA/长度/key/local/discriminator实参。未知opcode、未知分支条件、截短operand、已访问指令重叠均拒绝。它不执行OEM/CLR，不解析被跳过字节，也不声称完整原始CFG或完整CIL验证。32-byte fixed key 从原 PE FieldRVA `040025ac` 对应 `0x792328` 读取，按已核验 callback 变换，再 RC4 解出 `0x70b6f0` 的16-byte layer key；密钥字节与明文 key hash 均不写回执或源码。此固定格式不提供 CLI hash 绕过。

输出 `dynamic-methods/` 权限700，各二进制600。manifest 记录 descriptor/wrapper/table/key来源和所有输出回读 SHA。内层是标准 tiny/fat 方法体，**不添加或要求 VBPD**；严格检查 CodeSize、真实长度、payload重叠、EH try/handler/filter范围、finally/fault字段、external local signature源边界和prefix/compressed-count最小字节边界。完整 local类型语法、opcode/branch及CLR stack校验需独立回执，不从这些边界检查推断通过。六个真实输出须逐项对比冻结 manifest；`0600196f` 的75-byte结果还需与先前独立恢复结果相同。

这些文件仍是原厂派生私有分析材料。禁止把 body、cipher、local signature、解密 key、vendor IL 或可能含敏感值的输出提交公开源码或远程服务；允许提交的维护工具只有解析逻辑与资格来源数值/哈希。异常或部分写入始终保留不完整回执，工具不会执行任何 OEM 或构造/签名原程序集。
