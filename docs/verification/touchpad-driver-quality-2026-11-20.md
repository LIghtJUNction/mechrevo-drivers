# 触摸板与驱动质量验收 — 2026-11-20

本轮完成触摸板控制链路修复、全部驱动 C/H 审阅、软件包构建安装和 AUR 推送。触摸板在当前会话已通过内核状态回读；新内核和新驱动已写入磁盘，仍需重启后验证实际加载及硬件行为。本文日期和时间使用 UTC+08。

## 环境与源码

| 项目 | 本轮范围 |
| --- | --- |
| 实机 | 旷世 16 Pro，板号 `GM6PX0X` |
| 会话 | Arch Linux / Omarchy / Hyprland，活动本地用户 |
| 触摸板 | `UNIW0001:00 093A:0255 Touchpad`，seat0 |
| 运行内核 | `7.3.4-arch1-1` |
| 新安装内核及构建目标 | `7.3.5-arch1-1`；pacman 版本 `7.3.5.arch1-1` |

| 提交 | 改动 |
| --- | --- |
| [`97e6656`](https://github.com/LIghtJUNction/mechrevo-drivers/commit/97e66567bc779d37eb032f0e29a37fa9973658e6) | 触摸板 helper、命令行与热键共用控制、恢复入口及回归 |
| [`4194801`](https://github.com/LIghtJUNction/mechrevo-drivers/commit/419480149f71af5073aa86556838f2f69521f8bc) | Arch 包安装 helper、polkit、udev 和睡眠恢复文件 |
| [`2308286`](https://github.com/LIghtJUNction/mechrevo-drivers/commit/23082864752383e1448b10c82853fe1030d60434) | 25 个驱动 C/H 的质量修复、Makefile 与故障注入回归 |
| [`74814a9`](https://github.com/LIghtJUNction/mechrevo-drivers/commit/74814a9a8a5aec69a5904aba5f5a07415e9e18f5) | 将新增驱动和触摸板测试接入对应验证域，并验证测试选择行为 |

## 触摸板：从通知改为可核对的状态

控制入口通过 udev 的 `ID_INPUT_TOUCHPAD=1` 识别设备，修改 Linux input 的 `inhibited` 属性，再读取该属性确认结果。内核由此决定是否继续向输入处理方提供设备事件；它不要求调用 Hyprland、GNOME 或 KDE 的私有接口。接口语义见 [Linux input 文档](https://www.kernel.org/doc/html/latest/input/input-programming.html#inhibiting-input-devices)。

普通用户的写入由专用 polkit 动作授权，helper 通过 logind 核对活动本地用户和 seat。helper 只接受固定操作，不接受任意目标路径；读状态不提权。状态文件使用独立的 root 私有目录，操作加锁，写入失败时尝试回滚，并报告写入及回滚错误。设备重新出现和睡眠恢复共用恢复入口。

通用性有明确条件：内核导出 `inhibited`、udev 正确标识触摸板，并存在可核验的活动本地会话。桌面自身的禁用设置仍可屏蔽事件，因此内核启用状态不能单独证明光标一定移动。

| 实机操作 | 结果 |
| --- | --- |
| root helper 短暂禁用、启用 | 实际设备 `inhibited: 0 → 1 → 0`；其他输入设备状态未改变 |
| 已安装 `/usr/bin/mechrevo-control touchpad off/on`，UID 1000 | 两次操作退出 0，分别读回禁用、启用；恢复到原始启用状态 |
| 向真实 tuxedo_keyboard evdev 节点注入两次 F21 按下/释放 | 用户守护进程触发 `enabled: true → false → true`，每次稳定等待后状态一致 |
| root `touchpad-helper restore-all` | 成功，错误列表为空，最终状态启用 |

这次还清除了本机会让 F21 被处理两次的配置：Hyprland 与用户守护进程原来同时切换同一个按键。现在 F21 由守护进程处理，其他触摸板快捷键统一调用已安装的 CLI；旧桌面禁用标记已清除。原绑定与脚本已备份。

上述 F21 验证使用事件注入。没有把它写成用户实际按键或手指移动验收；本轮也未执行真实挂起、唤醒或热插拔测试。

## 驱动质量改动

逐文件审阅了基线的全部 **43 个 `src/` C/H**，其中 **25 个生产代码文件**有修改。另有 Makefile、测试和夹具改动；这些不计入 25 个文件。

| 范围 | 修复依据与结果 |
| --- | --- |
| Clevo / Uniwill | 单字节参数改用匹配宽度的写入函数；ACPI/WMI 返回先检查类型与长度；EFI 第二次读取后重新核对实际长度；无效事件不再使用未初始化值 |
| ITE / LED | 检查分配失败、HID 短传输和部分注册失败；按实际已取得的资源逆序清理；会等待硬件的操作改为允许阻塞的 LED 回调 |
| `tuxedo_io` / GXTP7380 | 补齐设备初始化中途失败的释放路径和 ACPI 引用释放 |
| NB04 / NB05 | WMI 访问与设备移除同步；停止异步工作后不再重新排队；仅在硬件写入成功后更新缓存；核对键盘事件载荷长度 |
| TUXI / STK8321 | 区分读取返回码和 PWM 数值；检查控制通道、模式及轴编号；I2C 写入失败不提交新状态 |
| 构建 | Makefile 使用递归 `$(MAKE)`、`$(CURDIR)` 和可覆盖的 KDIR，保留 jobserver 语义 |

回归使用实际生产 C 函数及硬件接口替身，覆盖错误返回、截短数据、资源分配失败、部分注册失败和工作停止等路径。它们证明对应程序边界，不证明未接入实物的灯光、风扇或传感器效果。

没有新增未知 EC 地址、机型功耗上限或风扇曲线。NB05 一条 RPM 路径重复读取同一地址的疑点，以及 TUXI 最大转速占位值，仍缺乏硬件依据，未猜测修改。

## 验证结果

最终验证入口为 `scripts/verify.py`，以普通用户运行，保留结构化结果。

| 验证 | 结果 | 边界 |
| --- | --- | --- |
| `--domain driver` | **584 / 584，通过，0 跳过** | 已包含三组新增 C 故障回归 |
| `--domain console` | **205 / 205，通过，0 跳过** | 已包含新的触摸板 helper 和打包回归 |
| `--domain all` | **0 失败、0 错误，结果为 incomplete** | `discovered=1282`、`ran=1278`、22 条跳过记录 |
| 28 个模块，Linux 7.3.5 headers，`W=1` | **完整编译成功，编译日志无警告** | 不代表其他硬件功能已通过 |
| 从驱动安装包解出的源码进行隔离 DKMS 构建 | **28 个模块构建成功** | 使用独立 DKMS/source/install 目录及显式内核头文件路径 |

完整范围的跳过来自旧 TCC 测试未提供显式 Node/锁定版本 TypeScript/N-API 输入，以及两个 thermald 测试未提供预处理后的包源码。22 条记录中包括一个 `setUpClass` 跳过，覆盖 4 个未计入 `ran` 的用例，因此不能直接把这些计数相减后称为通过数。受本轮修改直接影响的 driver、console 两域均完整通过。

没有为补齐这些旧范围自动安装额外依赖，没有把完整范围标记成 passed，也没有触发或等待例行远端 CI。

## 安装与发布

| 项目 | 已安装版本 | 安装后核对 |
| --- | --- | --- |
| `mechrevo-console` | `0.1.0.r1553.g97e6656-1` | `pacman -Qkk`：37 个文件，0 更改；普通用户安装路径操作通过 |
| `mechrevo-drivers-dkms-git` | `4.24.0.r220.g2308286-1` | `pacman -Qkk`：150 个文件，0 更改；新内核 28 个模块的签名和 vermagic 全部核对 |

控制台使用源码 `97e6656`，驱动包使用源码 `2308286`。安装包 SHA-256：

```text
mechrevo-console-0.1.0.r1553.g97e6656-1-x86_64.pkg.tar.zst
4b777a163c5a457c5c0675ace14f2ae3d7630fd70805aa258a8351c56e78fbc9

mechrevo-drivers-dkms-git-4.24.0.r220.g2308286-1-x86_64.pkg.tar.zst
aeb57722cd0eb6845cd91655ff15af6a1baf07418b4267fa08dc64a2147f24d3
```

`namcap` 未报错误。保留源码驱动包的 x86_64 范围、DKMS 运行依赖及控制台显式 `!strip`；相关启发式警告没有作为删除实际依赖或改变架构范围的依据。

- [控制台 AUR](https://aur.archlinux.org/packages/mechrevo-console)：远端提交 `101df44b2164a16530ba91e5089a9efd71ea4a2a`，公开 RPC 已读回 `0.1.0.r1553.g97e6656-1`。
- [驱动 AUR](https://aur.archlinux.org/packages/mechrevo-drivers-dkms-git)：远端默认分支提交 `e68f3d80c12e3375da0245b47d3eefaa73d3cf09`，提交包含 `4.24.0.r220.g2308286-1` 的 PKGBUILD、.SRCINFO 及 28 模块配置；17:03:24 通过公开 RPC 读回 `4.24.0.r220.g2308286-1`，维护者为 `lightjunction`。

Git 包每次构建会读取 `main`，因此以后构建出的修订号可能高于本次固定提交安装包。

DKMS 为已有 headers 的五个内核完成安装。7.3.4 曾保留 5 个同源码版本的无版本号文件；先归档，再由当前 DKMS 包统一后，`dkms status` 已无 built/installed 差异提示。运行中的模块没有热卸载。

## 系统与配置

本轮通过正式 `omarchy update -y` 入口更新系统，15:57 启动、16:36 完成。Linux 与 headers 已更新到 `7.3.5.arch1-1`，NVIDIA 已更新到 `620.73.04`。驱动安装后的发行版 hooks 已重新生成并签署 `/efi/EFI/Linux/omarchy_linux.efi`；随后独立运行 `sbctl verify`，确认该文件已签名。

最后只读查询中，官方仓库没有待更新包；AUR 查询仍列出 `gcc14` 和 `gcc14-libs`，因为 Omarchy 的更新脚本显式传入 `--ignore gcc14,gcc14-libs`。该更新策略保持原样。

`/etc/tcc/profiles` 和 `/etc/tcc/settings` 安装前后的 SHA-256 一致，tccd 保持 active。触摸板最终处于启用状态。

磁盘安装与运行状态分别记录：当前 `uname -r` 仍为 `7.3.4-arch1-1`，六个已加载项目模块的 sysfs `srcversion` 与安装前一致。例如运行中的 `tuxedo_io` 为 `29412F99639205DFCA6F7E8`，7.3.5 磁盘模块为 `5992296476417FFF7489B0A`。新驱动在重启后才会进入运行状态。

## 恢复材料与未完成的硬件验收

恢复材料位于本机 `~/Backups/mechrevo/20261120-touchpad/`，包含旧控制台包、旧驱动包、DKMS 归档、7.3.4 模块归档，以及原 TCC 和 Hyprland 配置。

旧驱动包 `4.24.0.r208.gf0b7544-1` 从原提交重建，其 118 个普通文件逐一与安装前内容比较一致。保留的包可通过 `sudo pacman -U <包路径>` 回退，再按内核与模块加载流程验证；本轮没有执行回退演练。

本机原始 JSON 收据及日志集中在 `~/.local/state/work-maintainer/mechrevo-drivers/`。公开文档仅保留支持结论所需的版本、计数、摘要和验证边界。

仍待确认的范围：

1. 重启进入 7.3.5 后的实际模块加载、平台控制和恢复行为。
2. 用户真实按键、手指移动、挂起唤醒及设备重新出现。
3. 其他桌面环境及其他型号的实物行为。
4. 完整测试范围中上述外部输入尚未提供的旧 TCC / thermald 验证。
