# 上游贡献

正式上游是 [TUXEDO GitLab](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers)。
GitHub 上的 TUXEDO 仓库是只读镜像，贡献和问题反馈应交到 GitLab。

## 2026-10-15 已提交

七项合并请求基于正式 `main` 的 `a929f9ce143316b3b18d5f3fbf9cc54f0c1122e2`，
保留上游 4.25 的新增型号与配置支持，不包含 MECHREVO 品牌匹配、GPU 默认值、AUR 配方或项目规则。
!61 通过真实 merge 提交纳入 !62 的分支历史，明确依赖该 helper 修复；!62 仍保持独立可审查。
这项分支依赖不表示上游已经合并 !62。

| 合并请求 | 修复 | 本地验证 | 当前状态 |
| --- | --- | --- | --- |
| [!58](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/58) | WMI 分配、请求格式及回复缓冲区检查，失败时保留输出 | 18 项实际 C 测试，ASan/UBSan；27 模块编译 | 已提交，待审查 |
| [!59](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/59) | Uniwill ioctl 错误传播、用户复制检查、风扇参数边界、TDP 先验证后写入及性能模式能力限制 | 46 项实际 C 测试；当前版本 27 模块编译 | 已更新，待审查 |
| [!60](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/60) | 仅在支持 Mini LED 且接口创建成功后写默认值 | 3 项实际 C 初始化测试；27 模块编译 | 已提交，待审查 |
| [!61](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/61) | 风扇状态、错误及并发；失败绑定和 probe 回退；排空按键任务、重置序列；适配器/PM 状态门禁及背光恢复 | 当前 `13d69cf`：131 项实际 C、故障与 pthread 测试；两个内核各完整编译 27 模块 | 已更新，Draft，实机验收未完成 |
| [!62](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/62) | custom profile 读取/reset/enable 失败后停止依赖操作，调用处报告错误并继续其他初始化 | 16 项实际 C 测试；27 模块编译 | 已提交，待审查 |
| [!63](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/63) | Clevo ioctl 原错误传播、复制失败阻止固件调用、失败读取保留输出 | 当前 `e8e8b39`：17 项实际 C 测试，普通及 DEBUG 构建；27 模块编译 | 已提交，待审查 |
| [!64](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/64) | LED 阻塞回调、最新亮度请求、睡眠队列排序、初始化错误和重绑状态 | 当前 `ee8ec49`：33 项实际 C/LED core 测试；27 模块编译 | 已提交，Draft，实机 PM 验收未完成 |

同一测试运行原代码能捕获对应错误。故障输入和线程交错由测试模拟，不是实机故障注入。
最终七组补丁组合 `668bbd2` 后 **248 项测试通过**，在 `7.2.7-arch1-1` 和 `7.2.9-arch1-1`
下各重新隔离编译全部 27 个模块，无编译警告。七个贡献提交均保留为真实合并祖先，4.25 机型与功耗表保持。
组合时 ioctl 的上下文冲突已处理，同时保留有符号错误返回、复制失败检查和模式操作串行化。
性能模式能力检查保留，串行化位更新位于内部 apply helper；测试夹具适配了完整 probe 和 helper 的返回值。
组合分支仅本地保存，没有推送或安装其模块；不能用它的验证代替本项目 4.24 维护分支验收。

!61 与已有 [!55](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/55)
修改同一个风扇函数，但没有吸收其缓存策略，也不声称解决 #399 的 EC 中断开销问题。
若 !55 先合并，需要重新处理上下文并复测。

## 验证和运行状态

本轮 Clevo ioctl、probe EC 错误、适配器/PM 门禁及 LED 回调修复已合入维护源码 `2c749ac`。
该版本完整 **281 项测试通过**，两个内核各在新独立目录完整编译 27 模块，无编译警告。
LED 修复包括睡眠前排空任务、初始化依赖错误与重新绑定状态，以及未初始化时拒绝恢复写入。
发布源码 `f1aa652` 仅增加文档，已构建、安装并发布 AUR `4.24.0.r23.gf1aa652-1`。
包内源码在两个内核下各通过 27 模块隔离 DKMS 编译，系统 hooks 也完成 54 模块编译、签名和安装。
仅重载 `tuxedo_io`，新 IO 的磁盘与运行 build ID 一致；另外四个运行模块仍保持 r10。
性能模式、TDP、无效风扇参数及无 Clevo 接口的拒绝路径通过实机检查；这不是 Clevo 固件功能验证。
TCC 配置字节、原背光/RGB/Fn 锁值、cTGP=25 及三个服务状态均保持；包完整性与启动文件签名检查通过。
新 keyboard 的绑定、probe、PM 和 LED 修复尚未运行，按用户安排留待以后重启及睡眠验收。
细节见 [GM6PX0X 记录](hardware/GM6PX0X.md)。AUR Git 和新网页已核对；发布初期 RPC 缓存仍返回 r16，
最终观察保存在本地发布和收尾记录中，不把旧缓存当作新版本元数据证明。
probe/PM/LED 测试使用实际驱动函数，LED 用例还编译固定版本的实际 Linux LED core 函数；
EC、资源、队列及计时服务由测试模拟，没有做物理 EC 故障注入或真实睡眠/闪烁验收。

上游 fork 的自动 CI 无法访问继承的构建模板项目，任务没有开始。
未修改的上游基线也出现同一错误。这不属于代码测试结果，不能写成 CI 通过。
已在 [!58 的说明](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/58#note_4014626534)
反馈；没有修改或重跑官方 CI。

本地证据位于 `.local-tests/upstream-20261015/`：
`submitted-merge-requests.json` 保存远端请求、正文及逐文件字节核对结果；
各 `*-validation` 目录保存日志、冻结源码哈希、补丁和 cpyref 上下文。
`combined-validation/receipt-final-keywork.json` 对应最终组合验证，
`local-binding-validation/final-receipt.json` 对应本项目维护源码验证。
上一轮证据在 `.local-tests/remaining-20261015/`，包含各补丁独立测试、
`upstream-combined-validation/receipt.json`、`package-validation/receipt.json`、
`package-validation/aur-published.json` 和 `live-runtime/output/receipt.json`。
本轮证据在 `.local-tests/next-20261015/`：各 `*-validation` 保存独立测试和冻结输入；
`main-validation/receipt-final.json` 记录维护源码 `2c749ac` 的 281 项测试及双内核完整构建；
`upstream-uniwill-validation/published-mr.json`、`clevo-upstream-validation/published-mr.json`
及 `led-upstream-validation/published-mr.json` 记录 !61、!63、!64 的公开版本；
`upstream-combined-validation/final/receipt.json` 保存最新七组组合验收。
`package-validation/receipt.json`、`system-install/receipt.json`、`live-runtime/output/receipt.json`
和 `postinstall/receipt.json` 分别保存包内验证、系统安装、有限激活及签名/完整性核对。
主动文件跟踪存在已记录缺口，不能把最终扫描当作完整创建、删除历史。
这些本机材料不上传为原始机器日志。

## 已知未解决或未验收

- NVIDIA GPU `0xC5` 通知断言尚未修复；厂商语义未确认，见 [诊断记录](NVIDIA-0xC5.md)。
- GM6PX0X 缺少可核验的 TDP 寄存器及安全上下限规范，仍不开放猜测性功耗写入。
- 真实睡眠恢复、重启、手动风扇、长期温度和噪声、电池及物理按键等仍需实机验收，TUXEDO 型号未做硬件矩阵测试。
- 新增完整实际 probe 的过滤器所有权和序列生命周期回归，初始化服务及同步由测试模拟；任意诊断 EC 写入仍不属于新增状态锁的保障范围。
- 独立 LED/充电 sysfs 事务及其他未纳入的 WMI 操作不受本轮 PM 门禁统一串行化；普通 TDP 的最终 EC 写入仍在模式 helper 解锁之后。
- RGB 部分写入失败和 probe 部分曲线写入没有事务回滚，LED 请求缓存不能作为硬件成功状态。

已提交不代表上游已合并；编译及模拟测试通过不代表硬件完整兼容。
