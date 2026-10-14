# 上游贡献

正式上游是 [TUXEDO GitLab](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers)。
GitHub 上的 TUXEDO 仓库是只读镜像，贡献和问题反馈应交到 GitLab。

## 2026-10-15 已提交

五组补丁均从正式 `main` 的 `a929f9ce143316b3b18d5f3fbf9cc54f0c1122e2` 分离，
保留上游 4.25 的新增型号与配置支持，不包含 MECHREVO 品牌匹配、GPU 默认值、AUR 配方或项目规则。

| 合并请求 | 修复 | 本地验证 | 当前状态 |
| --- | --- | --- | --- |
| [!58](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/58) | WMI 分配、请求格式及回复缓冲区检查，失败时保留输出 | 18 项实际 C 测试，ASan/UBSan；27 模块编译 | 已提交，待审查 |
| [!59](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/59) | Uniwill ioctl 错误传播、用户复制检查、风扇参数边界、TDP 先验证后写入及性能模式能力限制 | 46 项实际 C 测试；当前版本 27 模块编译 | 已更新，待审查 |
| [!60](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/60) | 仅在支持 Mini LED 且接口创建成功后写默认值 | 3 项实际 C 初始化测试；27 模块编译 | 已提交，待审查 |
| [!61](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/61) | 风扇状态、错误及并发；失败绑定回退；排空按键任务及重置半截按键序列 | 85 项实际 C、故障与 pthread 测试；27 模块编译 | Draft，实机验收未完成 |
| [!62](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/62) | custom profile 读取/reset/enable 失败后停止依赖操作，调用处报告错误并继续其他初始化 | 16 项实际 C 测试；27 模块编译 | 已提交，待审查 |

同一测试运行原代码能捕获对应错误。故障输入和线程交错由测试模拟，不是实机故障注入。
此前四组版本组合后 **137 项测试通过**，在 `7.2.7-arch1-1` 和 `7.2.9-arch1-1` 下各编译全部 27 个模块。
组合时 ioctl 有两处上下文冲突，已同时保留有符号错误返回和模式操作串行化；没有推送组合分支或安装其模块。

!61 与已有 [!55](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/55)
修改同一个风扇函数，但没有吸收其缓存策略，也不声称解决 #399 的 EC 中断开销问题。
若 !55 先合并，需要重新处理上下文并复测。

## 验证和运行状态

本项目的 4.24 维护分支独立移植了绑定、按键任务、序列重置、性能模式能力限制及 custom-profile 错误处理，
**201 项测试通过**。能力和序列修复组合在两套内核下各编译全部 27 模块；
最终 custom-profile 源码另做包内隔离 DKMS 验证后安装。
本节源码验证时新改动尚未安装或加载，运行包仍为 `4.24.0.r10.gc79a268-1`。
原有 r10 的有限实机验证见 [GM6PX0X 记录](hardware/GM6PX0X.md)。

上游 fork 的自动 CI 无法访问继承的构建模板项目，任务没有开始。
未修改的上游基线也出现同一错误。这不属于代码测试结果，不能写成 CI 通过。
已在 [!58 的说明](https://gitlab.com/tuxedocomputers/development/packages/tuxedo-drivers/-/merge_requests/58#note_4014626534)
反馈；没有修改或重跑官方 CI。

本地证据位于 `.local-tests/upstream-20261015/`：
`submitted-merge-requests.json` 保存远端请求、正文及逐文件字节核对结果；
各 `*-validation` 目录保存日志、冻结源码哈希、补丁和 cpyref 上下文。
`combined-validation/receipt-final-keywork.json` 对应最终组合验证，
`local-binding-validation/final-receipt.json` 对应本项目维护源码验证。
这些本机材料不上传为原始机器日志。

## 已知未解决或未验收

- NVIDIA GPU `0xC5` 通知断言尚未修复；厂商语义未确认，见 [诊断记录](NVIDIA-0xC5.md)。
- GM6PX0X 缺少可核验的 TDP 寄存器及安全上下限规范，仍不开放猜测性功耗写入。
- custom-profile 的适配器事件与 PM 仍有竞争：成功 suspend 后的迟到事件可以重新置位，需要独立 PM 状态门禁；尚未实机复现。Clevo ioctl 错误处理未修复。
- 真实睡眠恢复、重启、手动风扇、长期温度和噪声、电池及物理按键等仍需实机验收，TUXEDO 型号未做硬件矩阵测试。
- 新增完整实际 probe 的过滤器所有权和序列生命周期回归，初始化服务及同步由测试模拟；任意诊断 EC 写入仍不属于新增状态锁的保障范围。

已提交不代表上游已合并；编译及模拟测试通过不代表硬件完整兼容。
