# GM6PX0X 的 NVIDIA ACPI 0xC5 通知

2026-10-15 在同一台机器补充验证。本文是可复核的问题记录，尚不是 NVIDIA 兼容修复。

## 环境

- MECHREVO `Kuangshi16Pro Series GM6PX0X`，BIOS `N.1.09MRO07`。
- Linux `7.2.9-arch1-1`，NVIDIA open `615.78.08`，TCC `3.0.10`。
- 采集时运行 mechrevo-drivers 源码提交 `4bbbdc9`（包版本 `4.24.0.r6.g4bbbdc9-1`）。
- `tccd`、`thermald`、`nvidia-powerd` 均运行。没有更换 NVIDIA 模块或重启系统。

## 实际捕获的链路

停止 TCC、安全重载项目模块并重新启动 TCC，捕获到两组通知。每组的顺序相同：

```text
nv_acpi_nvpcf_event: NPCF event=192 (0xC0)
nv_acpi_notify_event: GPU event=197 (0xC5)
nvidia-powerd: nv_acpi_dsm_method nvpcf=1 subfunction=1
nv_acpi_dsm_method returns 0 (NV_OK)
nvidia-powerd: nv_acpi_dsm_method nvpcf=1 subfunction=2
nv_acpi_dsm_method returns 0 (NV_OK)
```

同时，GPU 的 `0xC5` 通知对应内核日志中的 `osapi.c:2198` 断言。
这证明 NPCF 通知已经到达 NVIDIA 的独立处理路径，`nvidia-powerd` 也完成了动态参数读取；
不能因为另一个通知产生断言就认定功耗更新链路整体失效。

操作前后的 `ctgp_offset` 均为 25。`nvidia-smi` 报告的当前功率上限均为 140 W，
默认上限 115 W，最大上限 140 W。这是**功率上限**，不是实际功耗、负载性能、持续运行能力，
也不能证明所有动态预算字段都已正确应用。查询使用 `enforced.power.limit`，
本机的 `power.limit`（软件请求上限）返回 `N/A`，两者不能混用。

单独启停 TCC 的另一轮采集中没有收到通知或断言，所以复现步骤和采集窗口必须一起记录。

## 固件及驱动证据

当前 DSDT 与已反编译材料一致，SHA-256 为
`de42c0d802ed746c1d032bf53ebb3b80f92172b8fb55e115b0e433566f542cda`。

NPCF 设备位于 `\_SB.NPCF`，当前 sysfs 路径与 SSDT 的设备和方法定义一致；
NVIDIA 的 NPCF notifier 注册及 `\_SB.NPCF._DSM` 路径没有发现命名空间错配。
AML 中四处 GPU `0xC5` 通知均位于电源预算更新路径，紧随 NPCF `0xC0`。
NPCF 的动态参数返回值包含这些路径更新的预算变量。

[NVIDIA NPCF 回调](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/615.78.08/kernel-open/nvidia/nv-acpi.c#L147)
将 `0xC0` 转为供用户态消费的 NVPCF 事件；
[GPU 回调](https://github.com/NVIDIA/open-gpu-kernel-modules/blob/615.78.08/src/nvidia/arch/nvalloc/unix/src/osapi.c#L2163)
没有 `0xC5` 分支，因而进入默认断言。相关本地源码与官方版本逐字节一致。

[ACPI 6.6 的通知定义](https://uefi.org/specs/ACPI/6.6/05_ACPI_Software_Programming_Model.html#device-object-notifications)
将 `0xC0` 及以上留给厂商定义，不能从标准推导 `0xC5` 的确切含义。

## 结论与待确认项

- 已确认 `0xC5` 是断言的直接触发条件，NPCF 正常路径同时工作。
- 现有证据不足以认定 `0xC5` 与 NPCF `0xC0` 完全等价，也不足以保证它可忽略。
- 不把 `0xC5` 映射为 GPS `0xC0`，不额外转发一次 NVPCF 事件，不删除断言冒充功能修复。
- 仍需 NVIDIA 或 OEM 明确 `0xC5` 的语义、是否需要独立处理及确认方式。
  已在 [NVIDIA 官方 Q&A #1447](https://github.com/NVIDIA/open-gpu-kernel-modules/discussions/1447)
  提交协议问题，询问事件含义、是否需要独立处理及可核验的状态；尚待回复，没有提交猜测性修复。
  公开内容未附原始机器日志，也明确说明尚未对比同版本闭源模块。

2026-10-15 后续核对官方主分支提交
[`ca2d03e6`](https://github.com/NVIDIA/open-gpu-kernel-modules/commit/ca2d03e6fafd4d4b56b383497d931c25b283bfcf)：
`nv-acpi.c` 和 `osapi.c` 与本机 `615.78.08` 对应文件逐字节一致，仍没有 `0xC5` 处理。
另检查 [PR #1299](https://github.com/NVIDIA/open-gpu-kernel-modules/pull/1299) 和
[PR #1181](https://github.com/NVIDIA/open-gpu-kernel-modules/pull/1181)，它们针对 NVPCF 通知造成的
待机唤醒，不定义或处理这里的 GPU `0xC5`，不能作为本问题的修复依据。

## 采集与恢复

使用现有 tracefs 上的独立 instance，探测 GPU/NPCF 通知、DSM 入口与返回状态及 WMI 调用。
所有探针及 instance 已撤除，TCC 恢复运行。没有更改功耗参数；TCC 启动期间会应用原配置。

本机日志位于 `.local-tests/iteration-20261015/`：`nvpcf-trace.txt`、
`nvpcf-observations.json`、`nvpcf-reload-trace.txt`、`nvpcf-reload-observations.json`、
`nvpcf-reload-kernel.log`。不向源码仓库提交原始机器日志。
