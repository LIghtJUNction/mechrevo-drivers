# GM6PX0X 功耗 固件 适配调查

2026-10-15 核对。**找到了精确型号的逆向项目、匹配 i9-12900HX／N.1.09MRO07 的公开样本，以及机械革命官方软件目录；仍不足以安全开放本机 EC TDP，也没有找到 NVIDIA GPU `0xC5` 的可采用修复。** EC 侧缺的是本机固件到原厂配置的对应关系、编码与安全范围；NVIDIA 侧缺的是通知的处理语义。下一步可核对原厂配置身份映射和通知处理协议。网上资料也不能代替睡眠、实体热键、充电、外屏和长期负载验收。本轮只做资料调查与本机只读核对，没有安装、加载或新增硬件验收。

## 精确型号找到了，固件身份还没有闭合

公开 DMI 样本同时记录了 MECHREVO、`Kuangshi16Pro Series GM6PX0X`、i9-12900HX 和 BIOS `N.1.09MRO07`。它证明这个组合确实出现在公开原始记录中，但 DMI 是机器上报的身份信息，不是固件镜像认证，也不包含功能验收。不能据此认定同板号的其他 CPU、品牌或 BIOS 可互刷，更不能借用其 EC 参数。([匹配样本](https://github.com/linuxhw/DMI/blob/57b21bc6fdd863380e99c228c22aafcd1509528c/Notebook/MECHREVO/Kuangshi16Pro/Kuangshi16Pro%20Series%20GM6PX0X/22CB98B41F6A))

机械革命官方精确商品“旷世 16 Pro”的关联目录返回 72 项驱动，覆盖多个平台分组，但本次列表没有 BIOS、EC 或控制台条目，也没有给出本机 CPU／BIOS／EC 的对应表。这只描述当前查询结果，不等于厂商没有相关软件。更关键的是，同型号社区项目的作者也明确写出：原厂控制中心使用 `PH6PGEx`、`PH6PG7x` 等目录，**GM6PX0X 应映射到哪一个目录仍待确认**。找到商品入口与软件名称，还没有找到可移植的 profile 身份。([官方关联目录](https://gwapi.mechrevo.com/api/v1/client/drivers?q%5Bby_display_product%5D=gxZSDX&limit=100)，[作者记录的映射缺口](https://github.com/Semcosm/kuangshi16pro-series-gm6px0x-laptop-control-center/blob/cae0afbaa704d253e29a268cc2d5f7fa4167106b/windows-control-center-analysis.md#L182))

## EC 功耗已有候选协议，不能据此开放写入

Semcosm 的同型号项目分析了 `ControlCenter_5.17.51.33_Mechrevo` 中的 `GCUService.exe`，公开二进制哈希及静态地址：模式 `0x751`、PL1／PL2／PL4 `0x783`／`0x784`／`0x785`、温度偏移 `0x786`。这是可审查的逆向线索，不是机械革命公布的编程规范；其原始安装包与本机固件的完整对应仍未取得。([GCUService 逆向报告](https://github.com/Semcosm/kuangshi16pro-series-gm6px0x-laptop-control-center/blob/cae0afbaa704d253e29a268cc2d5f7fa4167106b/windows-control-center/GCUService_reverse_report.md#L1))

TUXEDO 官方源码也使用这组功耗地址，但限值按 SKU 等条件选择；相关 `130／130／200 W` 表对应 `STELLARIS1XI05`，不是任意 GM6PX0X。其 PL4 还会根据探测标志使用倍数编码。因此地址相同不能推出单位、前置模式和安全范围相同，不能把该表或 Intel CPU 规格抄成本机白名单。([官方门禁 L217、功耗编码 L434](https://github.com/tuxedocomputers/tuxedo-drivers/blob/a929f9ce143316b3b18d5f3fbf9cc54f0c1122e2/src/tuxedo_io/tuxedo_io.c#L217-L485))

还必须分清作者的成功结果经过哪条通路。该项目只有在 standard 与 AMW0 两后端均可用、均支持应用功耗，且请求同时跨两组字段时，才将 PL1／PL2 交给标准 powercap、PL4／温度偏移交给 AMW0；否则 primary／fallback 仍可整组交给 AMW0。不能简化成“PL1／PL2 永远走 RAPL”。作者以 turbostat、RAPL 和 MSR 一致支持 PL1／PL2 结果，并承认 PL4 尚未完全由 Intel 遥测证实；这些记录**不能单独证明 EC `0x783／0x784` 写入已形成闭环**。本项目已有的 55 W RAPL 验证同样不等于开放 EC TDP。([实际路由](https://github.com/Semcosm/kuangshi16pro-series-gm6px0x-laptop-control-center/blob/cae0afbaa704d253e29a268cc2d5f7fa4167106b/kuangshi16pro-lcc/src/backends/converged/backend.c#L665)，[作者硬件证据边界](https://github.com/Semcosm/kuangshi16pro-series-gm6px0x-laptop-control-center/blob/cae0afbaa704d253e29a268cc2d5f7fa4167106b/kuangshi16pro-lcc/docs/hardware-validation.md#L218))

## NVIDIA 的断言路径已清楚，通知含义仍待确认

截至本轮核对，官方 main 仍为 `ca2d03e6`（615.78.08），GPU 通知处理函数仍无 `0xC5` 分支，未知事件进入默认断言。这与此前本机 trace 的触发关系一致；本轮没有重新做实机采集。ACPI 标准把 `0xC0` 及以上留给厂商定义，并未规定这个通知的动作。官方 Q&A #1447 仍没有回复或答案。当前缺口是“需要读什么、是否确认、如何验证完成”。([官方 GPU 处理函数](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/ca2d03e6fafd4d4b56b383497d931c25b283bfcf/src/nvidia/arch/nvalloc/unix/src/osapi.c#L2163)，[ACPI 通知标准](https://uefi.org/specs/ACPI/6.6/05_ACPI_Software_Programming_Model.html#device-object-notifications)，[官方 Q&A #1447](https://github.com/NVIDIA/open-gpu-kernel-modules/discussions/1447))

PR #1181／#1299 尚未合并，处理的是空闲 GPU 被 NVPCF／GPS 通知唤醒的问题，均未给 GPU `0xC5` 增加处理。#1299 采用 NVPCF 延迟重放，并明确不处理 GPS。#1181 的一位作者在另一台 ASUS 上观察到，直接丢弃 NVPCF 通知虽阻止唤醒，却留下旧功耗限额，直到下一次通知才更新。这是他机观察，不能当成本机复现，但足以说明“吞通知就好了”缺少依据。NPCF `0xC0`、GPU GPS `0xC0` 与 GPU `0xC5` 必须分别判断，不能因时间相邻就互相映射。([旧限额观察](https://github.com/NVIDIA/open-gpu-kernel-modules/pull/1181#issuecomment-5308056264)，[#1299 实际改动](https://github.com/NVIDIA/open-gpu-kernel-modules/pull/1299/files))

## 下一步缺的材料已经具体化

EC 侧最有价值的补证，是取得适用于 **12900HX／MRO07** 的原厂包或厂商说明，核对版本与哈希，确认 ROM-ID／ProjectID 到配置目录的选择关系，再查明 AC／DC 条件、模式初始化、PL4 编码和各字段范围。身份不匹配时继续保持只读；不能用另一个功率常量补这个缺口。

GPU 侧需要 NVIDIA／OEM 对 `0xC5` 的动作与确认语义作答，或出现可审计的官方实现，然后才能设计正常、失败和并发回归，再安排获准的加载验收。固件更新也需精确型号、CPU、原 BIOS／EC 的适用说明与恢复路径；本轮没有找到足以推荐本机刷写的证据。

现有 Fn-lock 磁盘安装仍待获准重启后的统一模块核对与实体功能测试。睡眠恢复、热键、实际充电、HDMI／USB-C 各种连接状态及长期负载仍须分别验收。
