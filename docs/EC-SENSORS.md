# Linux 标准 EC 温度接口

`mechrevo_ec_sensors` 将已核对的两路 EC 温度接入 Linux hwmon，供 `sensors`
和其他标准监控程序读取。目前严格限定 MECHREVO `Kuangshi16Pro Series GM6PX0X`、
BIOS `N.1.09MRO07`，不接受相似型号或其他 BIOS。

安装包含该模块的驱动包后：

```sh
sudo modprobe mechrevo_ec_sensors
sensors 'mechrevo_ec-*'
```

模块提供 `temp1_input` / `temp2_input` 和标签 `CPU EC` / `GPU EC`，
输入单位是毫摄氏度。CPU EC 来自现有 WMI 读取的 `0x043e`，GPU EC 来自 `0x044f`。
它们不是 `coretemp` 的 CPU 内部温度，也不等于 NVIDIA 驱动的 GPU 温度。
已有本机 WMI 与固件内存读取路径对照见[原厂分析](<../reports/GM6PX0X 原厂控制台静态分析 20261016.md>)。

模块不写风扇表、不设置功耗、不启动后台采样；只有读取属性时才读取 EC。
EC 读取本身沿用已安装 Uniwill 驱动的命令邮箱协议，不能据此声称物理总线完全没有写操作。
通信失败返回错误，不复用旧值；大于 125°C 的字节返回 `ENODATA`，不显示成有效温度。
零度仍是允许的原始读值，不保证传感器已校准或设备健康。

只提供两个只读温度通道，没有写回调、RPM 或风扇百分比接口。
注册前检查硬件身份、Uniwill WMI 接口和两路温度；失败时不留下半注册设备。
模块依赖现有核心驱动，可单独卸载：

```sh
sudo modprobe -r mechrevo_ec_sensors
```

模块包含精确 DMI 匹配别名及 `uniwill_wmi` 加载依赖，不新增全局强制加载配置。
首次显式加载和运行读取需要实机核对；下次开机能否由系统自动加载应单独验收。

## 风扇模式冲突

现有 universal 风扇控制在 full fan 位已置位时，原先会跳过设速却返回成功。
现在返回 `EBUSY`，保留当前模式和风扇目标。调用者应提示模式冲突，不能将请求视为已应用。
此修复不新增曲线布局、档位或控制写入，也不证明原厂风扇策略已迁移完成。
