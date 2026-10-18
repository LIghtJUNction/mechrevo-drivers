# 系统电源策略

本轮源码新增 Linux 系统电源服务后端及 GUI 接线。签名提交 `d48692a` 已推送，95 份冻结输入通过最终 580 项 Rust／143 项 Python 控制台检查及格式、Clippy、Windows 目标编译；**尚未安装、未完成本轮实机写入验收**。当前安装仍为 `mechrevo-console 0.1.0.r174.g256fb05-1`，原有功耗工具、驱动和已加载模块不变。[候选收据](verification/os-profile-fan-control.json)、[当前安装记录](verification/deployment-console-r174.json)

## 服务与回退

真实 `/sys` 环境优先查询系统总线上的 PowerProfiles 服务：先 `org.freedesktop.UPower.PowerProfiles`，它明确没有 owner 时才检查旧名 `net.hadess.PowerProfiles`。只有两个名字均确认无 owner、且前后总线身份稳定，才使用 ACPI `platform_profile`。查询失败、服务身份变化、异常响应或 owner 不是 UID 0 都返回不可用，不能绕到另一个接口继续写。测试用的替代 sysfs 目录不连接真实总线。

来源标识为 `linux_power_profiles_daemon`。状态和计划的 `details` 保存总线 ID、唯一 owner、端点、公布的档位、性能受限原因和应用临时请求（holds）；这些信息参与预览绑定，不从服务名字猜测能力。`power-saver` 在 PPD 中使用原名，在 ACPI 后端对应 `low-power`。只允许服务实际公布的档位。

系统电源策略与本项目的 CPU PL1、原厂性能预设、风扇曲线分别处理。PPD 可以执行它自身配置的设备动作；本项目没有因此获得 OEM 模式或固定瓦数的保证。源码调用固定系统 `busctl`，不自动启动服务、不交互提权，也不通过 sudo 或 pkexec 绕过授权拒绝。

## 预览、设置与失败

```sh
mechrevo-console status --json
mechrevo-console profile balanced --dry-run
mechrevo-console profile balanced --expected-current performance
```

这些是新源码候选的接口示例，最后一条会在检查通过后请求更改；`performance` 必须来自实际状态，不能照抄作默认值。`--expected-current RAW` 精确比较原始档位名或 Windows GUID，变化时在 setter 前拒绝。它不是原子比较交换，仍不能排除最后一次观察后的竞争或同值 ABA。

`--dry-run` 读取并生成计划，不设置档位。应用前重新核对服务身份、公布档位、当前值、holds 和限制信息，任何变化都要求重新预览。同档位只确认观察，不调用 setter，也不释放 holds。

不同档位的显式设置可能先释放其他应用的 holds，再因设备或服务错误而失败；旧档位无法重建这些请求。因此 PPD 后端最多提交一次 setter，失败不自动重试、不自动恢复旧档位。回执分别记录是否提交、应答、独立读回、服务是否仍相同、holds 前后观察及原始错误；看见 holds 消失不证明一定由本次操作释放。未收到明确应答时，即使随后看到目标档位，仍保留 `unknown`／`writer_may_continue`，不能把刷新当作先前请求已经结束的证明。[官方接口说明](https://upower.pages.freedesktop.org/power-profiles-daemon/gdbus-org.freedesktop.UPower.PowerProfiles.html)

ACPI 与 Windows 原有直接策略后端使用另一条条件恢复合同：失败后先读当前值；若已是原值，无需恢复；若为第三方值或未知，不覆盖；只有仍是本次目标、且恢复前再次确认后，才尝试恢复原值并读回。原始失败与恢复结果分别保留，这同样不是内核级锁或比较交换。不能把这条恢复逻辑搬到 PPD 的 holds 语义上。

CLI 即使在输出阶段失败，也保留已经完成的结果与原始错误，不通过重试硬件来补输出。成功设置会持续到系统或其他程序再次更改，关闭 CLI／GUI 不自动撤销策略，也不重建 holds。

## 界面与验证边界

GUI 显示服务来源、性能受限原因、holds 和预览中的释放提示，只在显式应用时提交。刷新保留先前失败和不确定回执；新的显式预览／应用可表达新意图，不会自动重试旧请求。本地应用工作或 worker 未退出时延迟关闭；本地工作结束后，远端请求可能继续本身不会永久禁止关窗，也不证明远端请求已结束。

统一离线回归已通过；实际 Linux 发布特性测试仍在进行，真实系统总线读写、权限拒绝、服务变更和原生 GUI 操作仍待本轮验收。已有 r174 安装结果与历史 PL1、亮度写入不转算为本轮功能通过；Windows 继续使用既有系统电源方案后端，PPD 是 Linux 后端。
