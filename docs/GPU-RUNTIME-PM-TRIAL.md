# GPU 运行时省电隔离测试

`scripts/gpu_pm_trial.py` 是待实机验收的实验命令，尚未安装进系统或 AUR 包。
它只针对 MECHREVO GM6PX0X／N.1.09MRO07、当前 PCI 0000:01:00.0 的 RTX 4060
（10de:28a0）。默认只检查准备状态，不改动硬件。

```sh
python scripts/gpu_pm_trial.py
```

执行临时测试需要 root、显式 `--apply` 和已有 Backups 目录内的新回执路径：

```sh
sudo python scripts/gpu_pm_trial.py --apply --seconds 60 \
  --output /home/lightjunction/Backups/gpu-pm-trial-new.json
```

显卡已失联、身份不匹配、内核日志不可读、显卡不是 active 或开关已经是 on 时，
拒绝测试。失联检查发生在创建锁和回执之前，不尝试解绑、复位、模块重载或重启。
并发执行由 root 所有的 `/run/lock/mechrevo-gpu-pm-trial.lock` 排他锁阻止；
锁只协调本命令，其他电源管理程序仍可能改变 sysfs。

## 改动与恢复

唯一硬件写入是显卡渲染功能的 `power/control`：原值 auto → 临时 on → 恢复 auto。
不更改 HDA 音频功能、ASPM、模块参数、风扇、CPU 功耗、EC、服务或开机配置。
这是 Linux 运行时电源管理的一项隔离变量，不等同于覆盖所有 GPU 内部节能机制。
也不能完全复现 `NVreg_DynamicPowerManagement=0x00` 的驱动内部行为。

- [Linux 对 power/control=on 的定义](https://docs.kernel.org/power/runtime_pm.html)
- [NVIDIA 运行时 D3 文档](https://download.nvidia.com/XFree86/Linux-x86_64/615.78.08/README/dynamicpowermanagement.html)

第一次写入前，以 0600 权限保存当前启动、设备、原值及恢复命令。
回执使用同目录临时文件、同步并替换，拒绝覆盖已有文件或符号链接。
持有期间每秒读取 sysfs 状态和增量内核日志，不持续运行 nvidia-smi，不启动压力负载。
准备和结束检查仍会通过 nvidia-smi 唤醒独显，因此不是完全无干扰的对照。

正常结束、Python 异常、Ctrl-C、SIGTERM／SIGHUP 后，只有在启动未变、设备仍匹配、
GPU 可访问、没有新故障且开关仍为 on 时才恢复 auto。恢复后再次检查开关、GPU 和日志。
新 Xid／PCIe 错误、GPU 失联、日志不可读、启动变化或外部改值会停止后续电源切换。
此时 `restored=false`，开关可能继续为 on；查看回执，不要对失联显卡强行执行恢复命令。
如果 auto 已回读成功、随后发现故障，`restored=true` 但 `valid=false`，两者含义不同。

SIGKILL、断电、进程或内核卡死不能保证自动恢复；`write_attempted=true` 的未完成回执
表示必须核对实际状态。仅当本机身份和启动仍匹配、显卡已经确认可用时，才使用回执中的
恢复命令。重启会丢弃本命令的 sysfs 改动，并由原有系统规则重新设值；本命令没有新增持久设置。
1–300 秒是正常持有时长，不是对内核 sysfs I/O 或故障恢复耗时的强制截止保证。

退出码 0：默认检查准备就绪，或显式测试完成并确认恢复；1：拒绝、失败或中断。
`valid=true` 只表示本段执行及恢复检查通过，不证明省电机制是根因，也不证明长期稳定或散热改善。
关闭设备运行时省电可能增加空闲功耗及发热。

## 本机验收边界（2026-10-16）

16 项回归测试覆盖成功、预检拒绝、回执写失败、写入已生效后异常、中断、故障、
启动变化、外部改值、恢复时掉线及并发锁拒绝；文件系统适配器测试确认只改变指定 control 文件。
还用真实子进程验证 SIGTERM／SIGHUP 恢复及 SIGKILL 保留未完成回执。
真实中断测试发现共享回执类在替换文件后、更新 inode 身份前被信号打断的竞态：
程序会把自己刚写出的文件误判为外部替换，导致最终恢复记录不能保存。
修复在这两个操作期间短暂阻塞 SIGINT／SIGTERM／SIGHUP，随后恢复原信号掩码。
确定性回归覆盖这一中断点、原信号掩码保留及 rename 失败后重试。
这些是模拟设备／临时文件测试，不是正常硬件路径验收。

本机 GPU 仍处于此前 Xid 79 后的失联状态。默认和显式测试入口均已核对拒绝路径，
control 仍为 auto，没有创建测试锁或回执，也没有写入硬件。
成功切换／恢复、实际功耗代价及长期掉线频率，仍需用户手动重启恢复显卡后验证。
