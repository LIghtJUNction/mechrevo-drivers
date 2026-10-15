# GM6PX0X MRO07 NVIDIA 固件协议核对

本轮补齐了 DSDT 之外的 SSDT15 定义，确认 NVPCF 动态参数单位及一处固件调用契约不一致。
扩展后的只读探针已经在本机编译、读取并卸载。没有据此开放功耗设置，也没有确认 GPU 失联根因。

## 来源与可复核依据

- 本机：MECHREVO Kuangshi16Pro Series GM6PX0X，BIOS N.1.09MRO07，内核 7.2.9-arch1-1。
- DSDT SHA-256：`de42c0d802ed746c1d032bf53ebb3b80f92172b8fb55e115b0e433566f542cda`。
- SSDT15：18135 字节，OEM Table ID `Opt2Tabl`，SHA-256
  `33575761681e9e96579cd876ee3720530772aef64702fa381a7cd6cd4250cbf9`。
- NVIDIA 615.78.08 标签对应提交 `1a82fbfda8f321b056eb3568850f9316c5e465c5`。
  [nvpcf.h](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/1a82fbfda8f321b056eb3568850f9316c5e465c5/src/nvidia/inc/kernel/platform/nvpcf.h#L307)
  中动态参数单位为 125 mW，common param0 的低 16 位名为 CTGP_AC_OFFSET。
  本机该文件与 GitHub Git blob 都为 `7d6771f3a6f83e57596dcad1cf5c228d449e241f`。
- [acpidsmguids.h](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/1a82fbfda8f321b056eb3568850f9316c5e465c5/src/nvidia/interface/acpidsmguids.h#L76)
  和 [os.c](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/1a82fbfda8f321b056eb3568850f9316c5e465c5/src/nvidia/arch/nvalloc/unix/src/os.c#L3190)
  显示驱动 NVPCF 2.x 使用 revision 0x0200。

完整 AML/反编译保留本机，不把原厂整张固件表提交到公开仓库。
公开[机器可读记录](data/gm6px0x-nvpcf-20261016.json)保留摘要、哈希、源码版本及实际读数。

## 单位和数据流已确认的部分

DSDT T1WR(0x1171, Arg1, …) 将 Arg1 放进 8 位 EC CTWA（超宽输入会被截断）。
当 TBME!=1 或 UFME!=0 时，else 分支将 CTWA 乘 8 放入 NPCF.ACBT；
另一分支进入 PWUP(OCPL)，会按模式选择其他值，不能套用同一公式。
SSDT15 的 NPCF function 2，在 GET 分支且 CTGP=1 时，把 ACBT 放进返回包偏移 5 的 TGPA。
该位置对应 NVIDIA NVPCF 2.x common param0 的 AC CTGP 偏移量。
因此该 else 分支的换算是 `CTWA字节 × 8 × 125 mW = CTWA字节 W`，语义是偏移量。
例如 ACBT=160 表示 20 W 偏移，不能写成“160 W 显卡功耗上限”。

这没有证明 Windows SMAPCTABLE_STRUCT 与 SMRW 12 字节缓冲区的绑定，
也没有证明 CPU PL1/PL2/PL4 字节的单位、最大允许值、启用和恢复流程。
两套协议不能因名称相似就套用单位。

本次固定命名对象的实际读数：

| 对象 | 原始值 | 能得出的结论 |
| --- | ---: | --- |
| NPCF.ACBT | 0 | 当前保存的 AC CTGP 偏移为 0 W |
| NPCF.CTGP | 1 | GET 分支会使用 ACBT |
| NPCF.DBAC | 0 | 原始标志值；不当作独立的 Dynamic Boost 实机验收 |
| NPCF.ATPP | 640 | 放入有符号16位 param1 SIGNED0；当前正值的协议尺度为 80000 mW，不是实测或已执行的 CPU 上限 |
| NPCF.AMAT / AMIT | 0 / 0 | 当前保存的原始动态参数 |
| 根对象 MOID | 1 | function 3/4 分别有 WTBX/WSLX 分支 |

MOID 当前有对应的 NVPCF 表分支；这和 GPS 目标温度/电源模式读取是不同协议路径。
这些是依次读取的固件状态，不是原子快照；失联 GPU 是否消费、接受或执行这些值仍未知。
没有调用 NPCF/_DSM 的 GET 或 SET，更没有写功耗。

## 可确认的固件调用契约问题

DSDT 的 EC0._Q80/_Q81 会先发送 NPCF 0xc0 通知，再直接调用 NPCF.NPCF：

- GUID：36b49710-2483-11e7-9598-0800200c9a66。
- revision：0x0100。
- function：2。
- 最后一个参数分别是整数 One / Zero。

但 SSDT15 的 NPCF 方法开头要求 revision=0x0200，否则立即返回 0x80000001。
因此这两个直接调用不能通过现有版本检查。这是当前固件源码可确认的不一致。
此外，2.x function 2 使用缓冲区参数并创建字段；不能只把调用中的版本改成 0x0200，
保留整数参数，就宣称修复完成。

限制：事件方法先发出的 Notify 仍可能由驱动处理，直接调用的返回值没有被使用。
因此不能由版本不一致推导“整个电源管理必然失效”。目前没有证据证明 _Q80/_Q81
在 Xid 79 前执行，也没有把它们与 GPU PCIe 失联建立因果关系。
驱动自己的 2.x revision 是 0x0200，这处不一致也不能代表驱动所有调用都传错版本。
未安装 BIOS/ACPI 覆盖，未发送通知或人为触发事件。

## 两条读取失败日志属于另一套 GPS 协议

独立复核追到 `platform_request_handler_ctrl.c` 的目标温度/电源模式日志：
`GetPerfSensorCounterById → SampleCounter → _SampleSensorLimit_ACPI → CallACPI`，
最终调用 GPS function `0x2a`（PSHAREPARAMS），不是上面的 NVPCF 表查询。
依据：[处理与采样源码](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/1a82fbfda8f321b056eb3568850f9316c5e465c5/src/nvidia/src/kernel/platform/platform_request_handler_ctrl.c)、
[GPS 功能编号](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/1a82fbfda8f321b056eb3568850f9316c5e465c5/src/common/inc/gps.h)。

本机 SSDT15 的 GPS 0x2a query1 返回 RETN=0x0101，query2 返回 0x0102。
按驱动字段定义，bit8 是 TGPU，bit16 是 PPMD，因此该静态实现宣告了目标温度支持，
**没有宣告电源模式 PPMD 支持**。这与电源模式 counter 进入不支持/不可用路径相符。
当前已加载静态 DSDT/SSDT 中 PDTS 只出现于字段创建，未见赋值；
不能排除运行时通过别名或整缓冲区写入，也不能把未支持功能伪装成读取成功。

TGPU 的 GET 分支则从 EC0.CGCT 填值，只有该对象不存在才回退到 0x57（87）。
因此目标温度读取失败不能顺带归为“固件没有支持”，仍需恢复后的实际响应、
驱动 counter 缓存/有效位及调用状态证据。失联状态下没有人为执行 GPS 查询。
MOID=1 仅说明 NVPCF 表选择分支存在，不验证这两条 GPS 日志路径。

## 验证与交付状态

扩展 [factory_policy_probe](../scripts/factory_policy_probe/README.md) 只读取上述固定数据对象，
保留精确 DMI/BIOS 限制、root-only 权限和原有 0xbb 固定读取。
NPCF flag、16 位数值和 8 位 MOID 越界均失败退出，不输出成功快照。
8 项实际 C 回归通过，包括成功路径、前后对象读取失败、越界/非法标志、未知身份、请求边界和锁释放。
有符号字段的16位编码保留原始值，测试覆盖 32768/65535，不擅自按无符号瓦数转换。
最终源码已在本机重新编译，root 读取成功，普通用户读取被拒绝，随后确认卸载。

本轮没有改变 EC/风扇/功耗设置或服务，没有 GPU reset，也没有重启。
适配功能验收仍约 45%；这次增加的是协议证据和诊断能力，不能计成散热或稳定性优化完成。
