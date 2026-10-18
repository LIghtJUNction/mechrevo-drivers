# 机械革命控制台使用说明

从应用菜单打开“机械革命控制台”，或运行 `mechrevo-console-gui`。以普通用户启动即可；查看、编辑和预览不需要管理员认证，点“应用”时按提示授权。

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

风扇页按“导入可信曲线 → 预览 → 显式应用”操作，曲线必须适用于本机。驱动过旧、设备不支持或状态不可用时不能应用。结束前恢复原设置；直接关窗时等待恢复完成，恢复报错不能视为成功。曲线不会自动保存成开机策略。

风扇控制尚未完成实机写入验收，不保证全机型支持或异响改善。Windows 当前提供系统电源方案、亮度后端，原厂控件与风扇控制尚未完成。
