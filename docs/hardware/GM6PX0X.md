# 旷世 16 Pro GM6PX0X 本机验证

日期：2026-10-14（Asia/Taipei）。这是单台机器的部分功能验证，不是完整兼容认证。

## 环境与安装状态

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
