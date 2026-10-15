# CPU 长期功耗上限

`mechrevo-power-control` 是独立、可选的用户态工具。它配置本机的 Intel RAPL
长期功耗约束（PL1），由原生 `thermald` 继续温控。它不开放 MECHREVO EC TDP
接口，也不提供 GPU、风扇或整机总功耗调节。

目前只允许以下组合：MECHREVO 旷世 16 Pro `GM6PX0X`、BIOS `N.1.09MRO07`、
Intel Core i9-12900HX、`thermald 2.5.13`，并且本机必须满足程序检查的固件、
RAPL 和服务配置条件。其他型号、BIOS、PPCC 或自适应温控平台会拒绝应用。
本次还固定已核对的发行版二进制 SHA-256；更新 `thermald` 后需要重新核验，不能仅凭版本相同放行。
本轮实机验收的目标值是 55 W；命令接受的其他值不等于已经实机验收或厂商额定安全值。

## 使用

安装独立工具包后：

```sh
sudo mechrevo-power status
sudo mechrevo-power set 55
sudo mechrevo-power status
sudo mechrevo-power clear
```

`status` 同时报告请求值和两套实际长期限值。只存在配置文件不代表设置已生效。
状态检查发现错误（包括损坏的状态、未完成事务或上限未保持）时，仍输出 JSON，
但退出码为 1；正常的未配置状态退出码为 0，`holding_now` 为 false。
缺少状态文件但托管 XML 或服务 drop-in 仍存在时（包括断开的符号链接），也报告错误并返回 1，
保留原文件供恢复核对。`status` 是一次只读观察；配置并发修改时可能读到中间状态，
不能作为跨文件的原子快照。
状态及其嵌套的事务、初始快照和应用收据必须通过类型及一致性检查，不能将字符串
`"false"` 当作应用成功。损坏状态会阻止 set/apply/clear/recover 写入，保留原文件及运行中的温控；
应保留并核对恢复材料后修复，不能把手工删除 state 当作正常撤销或恢复流程。
`set` 设置 CPU package 的长期功耗上限，温控可以使用更低的值。它保留原生温控区、
温度阈值及其他降温设备，短暂重启 `thermald` 以读取配置。
`clear` 撤销本工具配置并重启原来的温控策略；恢复后功耗限值继续由该策略决定，
不是固定写回某个旧的高限值。

程序拒绝覆盖已有的其他温控配置、其他服务覆盖文件或被外部修改的托管文件。
新恢复材料统一放在 `/home/lightjunction/Backups/projects/mechrevo-drivers/`。
发生应用失败时应查看命令错误和 `status`，不能只看退出前打印的目标值。
卸载工具前先运行 `clear`，确认温控服务仍 active，再移除工具包。

若命令被强制终止或断电，`status` 显示未完成事务，可运行 `sudo mechrevo-power recover`。
恢复命令确认原事务进程已退出、受管文件仍属于本工具后，撤销配置并恢复原温控策略；
原命令仍在运行时会拒绝恢复。不要通过手工删除 state 文件绕过这项检查。

## 本地打包

配方在 `packaging/arch/mechrevo-power-control/`，只安装命令和说明，安装本身不会
启用功耗上限。它没有加入现有 DKMS 包，也没有作为新包发布到 AUR。
使用配方指定的源码提交构建；本地验证可以将来源切换为同一提交的本地 Git 仓库：

```sh
cd packaging/arch/mechrevo-power-control
MECHREVO_POWER_SOURCE=file:///absolute/path/to/mechrevo-drivers \
MECHREVO_POWER_COMMIT=<full-source-commit> makepkg --cleanbuild
```

不要把包含未提交更改的目录当作已经冻结的发布源码。

## 实现和边界

本机原来传给 `thermald` 的命令含 `--adaptive`，但没有可用的 INT3400 接口，
实际回退为 default engine。工具启用时明确选择 default engine 并指定独立 XML；
撤销时恢复原服务命令。没有替换 `thermald` 的二进制程序，也不开放其 D-Bus 控制权限。

XML 对 MSR 与 MMIO 两个 RAPL cooling device 同时设置范围：最高功耗为用户上限，
最低功耗按 `thermald 2.5.13` 自身的 50% 算法从该上限计算。
这里的 `MinState` 表示“最少降温、最高功耗”，`MaxState` 表示“最多降温、最低功耗”。
只修改前者可能弄反范围，使温控无法继续升级到其他降温设备，因此必须成对设置。
保留原步长后，控制状态的变化顺序会改变；这是一份带用户上限的原生温控配置，
不是原热控功率序列完全不变的承诺。

XML 本身只修改控制器缓存，不保证冷机时立即写入硬件。服务启动后的事件回调
检查两套长期约束，对高于上限的值各执行一次降限；已有更低值保持。
没有反复抢写功耗的后台循环。回调失败不会停止温控服务，但必须视为上限应用失败。

`thermald` 正常退出会恢复自身的启动快照。因此服务重启期间可能短暂回到原限值，
启动回调完成后再应用上限。服务停止期间、固件接管以及其他控制器写入不属于
持续保持保证。sysfs 没有原子比较后写入机制，初始化时也不能排除同时写入的竞争。
机器重启、睡眠恢复和 BIOS 更新需要另行验收。

这里只限制长期平均 CPU package 功耗。PL2 仍保留原策略，短时功耗可以高于 PL1；
平均窗口和能量采样也不能当成瞬时整机功耗。55 W 来自本机热设计参考值，
不是已验证的整机最大安全功耗。

源码和测试依据为 Intel 官方 `thermal_daemon` 的
[`v2.5.13`](https://github.com/intel/thermal_daemon/tree/v2.5.13)，
冻结提交 `5fdc5bbca7f8c3f5bb2ad6540e8211a159a88a3a`。
实机结果单独记录在 [GM6PX0X 验证记录](hardware/GM6PX0X.md)，未验收项目不能由代码测试代替。
