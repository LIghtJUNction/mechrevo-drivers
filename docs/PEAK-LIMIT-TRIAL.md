# 短时 CPU 功耗对照

`mechrevo-peak-trial` 是本机限定的临时 PL2 对照工具，不是默认功耗策略。
必须已有健康、无未完成事务的 `mechrevo-power` 长期上限，显卡也必须正常。
本机厂商、型号、BIOS、CPU、thermald 二进制和受管配置均沿用原工具的资格检查。

```sh
sudo mechrevo-peak-trial --peak-watts 55 --seconds 60 --read-thermal-msr \
  --output /home/lightjunction/Backups/projects/mechrevo-drivers/light/power-thermal-20261016/new-trial.json
```

输出父目录须已存在，文件必须是新路径。回执包含恢复需要的原值，应保存在统一备份目录，
不要覆盖或删除未恢复的回执。`--read-thermal-msr` 可省略，启用时需要已有可读 MSR 设备；
工具不加载 MSR 驱动，也不写 MSR 或清除硬件日志位。

工具只向下写 MSR/MMIO 两套 RAPL 的 `short_term` PL2，各执行一次并回读。
目标范围为 35–55 W，不能低于已配置的 PL1 上限，也必须低于两套原始 PL2。
不写 PL1、平均窗口、EC、风扇、服务配置或 CPU 频率策略，不启动压力任务或持续抢写循环。
采样持续 1–300 秒；正常结束和可处理的信号中断都尝试恢复。

写入意图和原值在控制写入之前同步落盘。恢复只覆盖仍等于本试验目标的值；
已经回到原值时不再写入，外部修改则保留并报告待恢复。
新 GPU/PCIe 故障、显卡检查失败或资格变化会持续阻止向上恢复，避免误判下一次空日志为健康。
回执的 `valid`、`restored`、`complete`、`errors` 要分别检查；命令失败不等于硬件已经恢复。

SIGKILL、断电或恢复失败后，先确认显卡和配置恢复健康，再使用新的输出路径：

```sh
sudo mechrevo-peak-trial --recover /absolute/path/original-trial.json \
  --output /home/lightjunction/Backups/projects/mechrevo-drivers/light/power-thermal-20261016/new-recovery.json
```

恢复确认原进程已退出、PID 启动时间与启动身份、硬件路径及受管配置；拒绝跨启动的旧回执。
它使用与 PL1 工具相同的进程锁，不改写源回执。PL2 恢复未完成前，
不要先改变 PL1 配置，否则会使原回执的配置资格不再匹配。
`mechrevo-power clear` 管理 PL1，不能代替本工具的 PL2 恢复。

## 验证边界

本机试验已完成两套 PL2 `140 → 55 → 140 W`，以及 PL1 35 W 时的
PL2 `140 → 35 → 140 W`。两次均成功恢复，没有改变窗口或风扇策略。
这些试验仍出现 96°C 尖峰和新增内部热状态记录，不能声称单靠 PL2 已修复散热。
自然桌面负载会变化，这些结果也不是严格控制负载的性能或因果实验。

错误路径回归包含两接口写失败、回读失败、外部修改、配置变化、GPU/日志故障、
锁竞争、信号及真实子进程 SIGKILL 后的恢复。模拟故障不等于实机故障注入。
原始实机回执保存在统一备份目录；汇总在项目本地验证目录。
