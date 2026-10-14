# 旷世 16 Pro GM6PX0X 本机验证

日期：2026-10-14（Asia/Taipei）。这是单台机器的部分功能验证，不是完整兼容认证。

## 环境与安装状态

- DMI：`MECHREVO / Kuangshi16Pro Series GM6PX0X`，主板 `GM6PX0X`。
- BIOS：`N.1.09MRO07`。
- 系统：Omarchy 4.0.4，运行内核 `7.2.9-arch1-1`；另保留 `7.2.7-arch1-1` 的 headers。
- 安装包：`mechrevo-drivers-dkms-git 4.24.0.r3.g6598274-1`，驱动源码提交 `6598274`。
- 按仓库 AUR 配方从 GitHub `main` 构建，经 `pacman -U` 升级；不是手工覆盖系统模块。
- 两个内核的 DKMS 均完成全部 27 个模块的编译、签名和安装。
- 在当前会话安全卸载、重新加载相关模块，未强制卸载，未重启系统。
  `tuxedo_io`、`tuxedo_keyboard` 的运行模块 GNU build ID 与新安装模块一致；
  其余已加载模块的 `srcversion` 与磁盘一致。
- 安装 hook 重新生成并签名 UKI；`sbctl verify` 通过。当前 Secure Boot 关闭，
  不能据此认定 DKMS 签名密钥已在启用 Secure Boot 时受信任。

## 修复与结果

| 项目 | 实测结果与边界 |
| --- | --- |
| Uniwill 识别 | 使用 `uniwill_wmi`，硬件 model ID 为 16；没有强制启用 Clevo 路径 |
| TDP 读取 | 原包将不支持返回值 `-ENODEV` 显示成 `4294967277`；修复后全部九个当前值/最小值/最大值命令返回 `ENODEV`，用户缓冲区不被改写 |
| Mini LED | 本机没有局部调光控制节点；回归测试验证不支持及 sysfs 注册失败时不发送初始化命令；未做 WMI 总线跟踪 |
| RGB 键盘亮度 | 标准 sysfs 亮度 `1 → 0 → 1` 写入与回读通过；恢复原 RGB 强度 `4 4 4`，未由人工确认实际发光效果 |
| Fn 锁 | 标准 sysfs `0 → 1 → 0` 切换与回读通过；未人工逐键验证 Fn 组合键 |
| 温度与风扇 | 三次样本均可读取 CPU/GPU 温度和两个风扇原始值；样本为 CPU 71°C、GPU 53°C，风扇原始值约 156–160，不能当作 RPM 或散热稳定性结论 |
| 充电 | 可读取 `high_capacity / balanced / stationary`，原配置 `high_capacity` 保持；未切换充电模式，未测实际充电行为；无标准 `charge_control_end_threshold` 节点 |
| TCC | 重载前短暂停止，验证后恢复运行；原键盘设置与 `ctgp_offset=25` 在守护进程启动后恢复，未主动调节 GPU 功耗 |

`GM6PX0X` 未匹配现有 TUXEDO TDP 白名单。这里修复的是正确报告“不支持”，
没有借用相近型号的功率上限来启用调节。

34 项本地测试全部通过，其中 10 项针对实际 C 函数及外层 ioctl 入口：
包含九项成功读取命令、负错误码传播、EC 失败、错误用户缓冲区和 Mini LED 支持判断。
将外层 `long status` 临时改回旧的 `u32` 的内存回归实验能触发预期失败。
`namcap` 无错误，保留源文件架构及 DKMS hook 依赖的两个预期警告。

## 未解决与未验证

- 重载及 TCC 启动时仍观察到 NVIDIA `nvAssertFailedNoLog: Assertion failed: 0 @ osapi.c:2198`。
  该消息在本次修复之前已经出现；没有证明其根因或已被修复。
- 未验证重启、睡眠/唤醒、外接显示器切换、实体热键、充电阈值或长期高负载稳定性。
- 没有主动修改风扇曲线、TDP 白名单、GPU 功率或充电策略；模块加载本身包含上游的
  EC/WMI 初始化，已有 TCC 服务也会应用原配置，因此不能称整个过程“没有 EC 写入”。
- 风扇/温度样本只证明正常读取路径可用，未模拟实机 EC 故障；TDP 写接口也不属于此次验证范围。

## 本地证据与恢复

日志和只读快照位于项目的 `.local-tests/local-adaptation-20261014-2253/`：
`before.json`、`makepkg.log`、`install.log`、`runtime-checks.json`、`after-tccd.json`、
`kernel-reload.log` 及两个内核的构建日志。不将机器日志提交到源码仓库。

旧安装包和两个内核的 54 个项目模块已归档到
`~/Backups/projects/mechrevo-drivers/light/20261014-2253-adaptation/`，
独立解包校验内容、权限和所有者，并保存 `SHA256SUMS` 与恢复说明。
恢复优先使用保存的包及 DKMS，不要手工删除 `/usr/src` 或 `/var/lib/dkms`。
