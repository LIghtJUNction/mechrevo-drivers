# 机械革命控制台使用说明

从应用菜单打开“机械革命控制台”，或运行 `mechrevo-console-gui`。以普通用户启动即可；普通状态查看、编辑和预览不需要管理员认证，点“应用”时按提示授权。CPU 功耗策略检查另需显式授权，不会在启动或自动刷新时弹出。

终端常用命令：

```sh
mechrevo-console status
mechrevo-console oem list
mechrevo-console thermal --samples 30 --interval-ms 1000
mechrevo-console profile balanced --dry-run
mechrevo-console brightness 50 --dry-run
```

`status` 查看设备概况，`oem list` 列出本机支持的原厂控件。Linux 的 `thermal` 只读采集温度和转速，共 30 次，间隔设为一秒，不接管风扇。热读数与完整设备信息分别记录刷新时间；温度更新不表示型号、电源等信息也已刷新。缺失或过期读数不能当作零。

`profile` 调整系统电源方案，不等于原厂性能档位或指定功耗；`brightness` 调整系统识别的屏幕亮度。先用 `--dry-run` 预览，确认后去掉该参数才会修改；多屏需选择设备。

亮度接口没有直接写权限时，Linux 会检查当前用户的活动桌面会话，符合条件时通过系统 logind 调节，无需以 root 启动控制台。设备列表中的 `writable` 仍表示直接写权限；`write_backend` 表示实际可用通道。预览不修改亮度；应用前重新检查设备、会话和当前值，应用后分别显示请求值与驱动报告值，后者不等于测得的屏幕亮度。

脚本需要防止旧请求覆盖已经变化的亮度时，可附带 `--expected-current RAW`。`RAW` 是设备列表中的 `control_value` 原始值，不是百分比；不匹配时不写入。它与应用前检查一起拒绝陈旧计划，但系统接口没有原子比较写入，不能排除外部程序恰好在最终检查后改值。通信中断或恢复失败会保留独立的错误和结果，不自动重试；系统写入是否结束尚不明确时，刷新读数也不会解除界面的写入限制。

Linux 的“控制 → CPU 功耗上限”复用独立功耗工具；缺少工具或本机不符合支持条件时不能应用。先点“检查 CPU 功耗策略（需授权）”，再编辑完整策略、预览，最后“授权应用并回读”。检查只读，不设置上限。终端对应命令：

```sh
mechrevo-console cpu-power probe
mechrevo-console cpu-power set 55 --dry-run
mechrevo-console cpu-power set 55
mechrevo-console cpu-power clear
```

CPU 的 `--dry-run` 也会先授权只读检查，再本地预览；不应用策略。55 W 只是示例，实际允许范围从 35 W 到检查得到的初始运行上限，不是推荐功耗。`set` 省略的温度、轮询、递进选项会保留当前完整设置，包括已有的 90°C／1 秒／0 秒或原生设置。它只管理 CPU 长期功耗 PL1，不改短时功耗 PL2、平均时间窗口或硬件 TCC 温度保护点。关闭窗口或退出命令不会撤销已应用的 CPU 策略；`clear` 返回原生温控策略，不是固定恢复某个瓦数。未完成事务仅在检查允许时用 `cpu-power recover` 恢复，`cpu-power reapply` 用于重新保持现有完整策略。

风扇页按“导入可信曲线 → 预览 → 显式应用”操作，曲线必须适用于本机。驱动过旧、设备不支持或状态不可用时不能应用。结束前恢复原设置；直接关窗时等待恢复完成，恢复报错不能视为成功。曲线不会自动保存成开机策略。

风扇控制尚未完成实机写入验收，不保证全机型支持或异响改善。Windows 当前提供系统电源方案、亮度后端，上述 CPU PL1 后端尚未实现，原厂控件与风扇控制尚未完成。
