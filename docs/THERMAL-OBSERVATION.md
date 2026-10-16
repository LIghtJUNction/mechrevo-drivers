# 温度与功耗只读采样

`mechrevo-thermal` 用于 GM6PX0X／N.1.09MRO07 的现有 Uniwill 读取接口。
它不调整功耗、风扇、EC 控制参数或服务，不启动压力负载。
GPU 已失联、内核日志不可读或功耗状态异常时，拒绝开始采样。

安装 `mechrevo-power-control` 后执行：

```sh
sudo mechrevo-thermal --seconds 60 --interval 1
sudo mechrevo-thermal --seconds 60 --interval 1 --output /absolute/path/new-capture.json
sudo mechrevo-thermal --seconds 60 --read-thermal-msr --output /absolute/path/new-msr-capture.json
```

默认将 JSON 写到标准输出。`--output` 只创建新文件，不覆盖已有文件或符号链接，
不自动创建目录。读取 EC 和受保护的功耗状态需要权限，因此示例使用 sudo。
命令不安装服务或持续后台进程；手动中断也会尽量输出已有样本并关闭设备。

使用 `--output` 时，会在预检前及每组采样后保存完整 JSON 进度；文件权限为 0600。
保存先写同目录临时文件并同步，再替换本命令创建的文件。写入失败时保留上一份完整进度；
替换与文件身份更新期间短暂阻塞 SIGINT／SIGTERM／SIGHUP，完成后恢复原信号掩码，
避免在两个操作之间中断而把自己的新文件误判为外部替换。
发现输出路径被其他程序替换则拒绝覆盖。采样尚未完成时始终是
`complete=false`、`valid=false`，只有全部结束检查通过后才保存成功结果。
SIGTERM／SIGHUP 会尝试保存中断原因；即便 SIGKILL 无法被处理，最后一次完整进度仍可读取。
强制结束可能丢失最近尚未保存的样本或留下 `.mechrevo-thermal-*` 临时文件。
这不保证突然断电或磁盘故障下的数据持久性。标准输出重定向没有逐组保存保证，
需要保留故障前数据时应使用 `--output`。

退出码：0 表示观测完成且通过检查；1 表示硬件／日志／状态检查失败或采样中断；
2 表示命令参数或输出文件错误。**失败／中断后留下的文件可能包含有价值的故障前样本，
应同时查看退出码、`valid`、`complete` 和 `errors`，不能当作成功对照。**

## 采样内容

- CPU package 和最高核心温度、读取前后 package 温度、单组读取耗时。
- 现有驱动的两个 EC 温度字节及双风扇原始刻度；原始刻度不是 RPM。
- 从 `mechrevo_ec` hwmon 的 CPU fan/GPU fan 接口读取的 OEM RPM，独立记入 `fan_rpm`。
- CPU0 暴露的封装热节流计数、RAPL 能量计数及计数回绕范围。
- CPU0 对应核心的节流计数、封装节流累计/最大记录时间；接口缺失记为 `null`，不是零。
- MSR／MMIO 的 PL1、PL2 及两类时间窗口。PL2 可以高于设置的 PL1。
- 启动身份、开始／结束及每组采样前后的 GPU 可访问性、托管功耗状态，以及增量内核日志游标。

每个新样本附加 `read_windows`，仍使用 schema 1 并保留全部旧字段。
各组记录绝对单调时钟秒数 `start_monotonic_s` / `end_monotonic_s`，实际包围该组读取：
`package_before`、`core_max`、`throttle_count`、`energy`（能量及回绕范围）、
`ec`、`rapl`、`fan_rpm`、`package_after`、`throttle_details`，顺序与原先相同。
显式请求 MSR 时再有最后一组 `thermal_msr`；未请求时没有此组。
RPM 接口不存在时仍记录探测窗口，不补造转速或零耗时。
时钟必须为有限非负数，单次样本内各组之间及组内须非递减；否则本次读取失败。
此检查不比较不同样本的时钟。已有样本照常保留，失败组不冒充成功样本。
这些窗口只给出用户空间读取的起止界限，不是设备内部采集时间，也不保证同组内同时读取。
旧文件没有此字段仍是原来的格式。当前 `analyze_thermal.py` 仍用整组 `monotonic_s`
近似能量区间，并未自动改用窗口中点；不能把它的平均功率当成精确对齐的瞬时结果。

各通道按顺序读取，不能当成严格同时的硬件测量；coretemp 自身也可能缓存读值。
能量计数可用于计算区间平均功耗，不能当成瞬时功耗。热节流计数不是节流时长。
该计数记录处理中断时观察到内部高温状态的次数，不是独立过热事件数；
CPU0 对应核心没有增加，也不能代表其他物理核心没有过热。
内核累计时间有过滤和计时精度限制，不是精确的节流占空比。

`coretemp` 温度约缓存一秒，短间隔的读取前/后值可能复用同一缓存，不能据此证明期间温度稳定。
秒级能量平均值也不能排除毫秒级 PL2 突发。

`fan_rpm` 与原有 `ec.fan1_raw/fan2_raw` 分开，不能把占空比刻度换算或改名成 RPM。
有接口时记录 `available=true`、`source=mechrevo_ec`、`cpu_rpm` 和 `gpu_rpm`；
无此模块或旧版只有温度接口时，明确记录 `available=false` 及原因，不填零。
真实接口返回零属于有效读数，但不构成独立机械转速测量。CPU/GPU 也不是同时读取。
每组重新识别设备和两路标签；重复设备、缺少部分风扇文件、错误标签、读取失败、
非十进制或超出原厂 16 位公式范围的读数使观测失效。一次观测中已经出现的 RPM 接口
随后消失也会使观测失效，保留此前样本，不能把驱动消失解释成风扇停转。

可选 `--read-thermal-msr` 在原有 DMI/BIOS 限制外，核对 Intel family 6/model 151 的
CPU0 身份及内核暴露的 MSR、DTS、package thermal、power-limit CPUID 能力。
还要求正在运行的内核能证明 MSR 支持为内建：检查 `/sys/module/msr`、无加载模块的
`initstate`、当前内核 `modules.builtin`、内核 notes 和启动身份。仅加载的模块也拒绝，
避免模块在检查后卸载而使打开字符设备触发内核自动加载请求；工具不会尝试加载模块。
逐层使用目录描述符和 `O_NOFOLLOW` 打开 `/dev/cpu/0`，只接受已有的 `202:0`
字符设备；普通文件、最终或父目录符号链接、其他设备号均拒绝。打开后核对设备身份，
每组固定三次读取前后再次核对内核/启动、支持、路径和描述符身份。任何变化使本组失败，
不返回这组数值，并尝试关闭全部自有描述符。这些检查不替代对系统特权写者的信任。
MSR 检查覆盖在 `read_windows.thermal_msr` 中；此窗口包含资格再核对及实际寄存器读取。

仅以只读方式读取 CPU0 的 `0x19c`、`0x1b1`、`0x1a2`；前两者记录内部热状态、
外部 PROCHOT 和功率限制的当前/日志位，温度目标寄存器保留原始值。
设备不存在、权限失败或短读会使显式请求失败，不写寄存器或清日志位。
关闭失败也会使结果失效，仍尝试关闭其余描述符，保留之前的样本与读取错误及 elapsed 时间。
字符设备打开可能经内核自动加载路径；`O_RDONLY` 本身不是“不加载模块”的证明。
参见 [Linux 字符设备打开/自动加载路径](https://github.com/torvalds/linux/blob/v6.12/fs/char_dev.c)。
日志位可能早于本次观测，内核也可能清除它们；没有抓到状态不代表采样间隔内没有事件。

## 结果失效的条件

采样前建立当前启动的内核日志游标，只监测此后新增的错误；旧 Xid 仅作为起点，
不会被误算为新故障。采样每组前后检查新日志；新 Xid、GPU 掉线、不可纠正 PCIe 错误
或 AER 恢复失败会立即结束观测，保留已有样本并设置 `valid=false`、`complete=false`。
日志查询失败不能被解释成“零错误”。

开始／结束或每组采样前后的 GPU 检查失败、设备集合改变、启动身份改变、托管配置发生变化、
任一已采样 PL1 超过请求上限、EC 读取失败或读值越界也会使结果失效。
GPU 检查使用 nvidia-smi，可能唤醒休眠的独显，因此这不是完全无干扰的空闲功耗测试。
每组额外检查两次，检查耗时也会影响实际采样间隔。即使内核尚未记录错误，
GPU 检查失败也会停止后续 EC 采样；这不能阻止检查与读取之间发生的硬件故障。
检查不复位显卡，也不重启机器或服务。

`valid=true` 只表示完成了这一段观测并通过上述检查，不是完整硬件健康认证。
短于采样／日志检查间隔的变化、未记录到日志的瞬态故障以及其他后台负载变化仍可能漏掉。
本工具没有控制任务、环境温度或初始散热状态，单独一个有效文件不能证明性能或散热改善。

2026-10-16 重启后 GPU 已恢复，此前版本的正常采样和可选 MSR 读取已在本机通过；
源码 `cbb3396` 形成时新增保护仅完成隔离回归；随后 `0.1.0-15` 已安装并完成一次3秒只读实机诊断，
3个样本的MSR/RPM读取及前后保护核对通过。该单次结果不代表长期采样、其他型号或散热效果验收；
此前 GPU 失联时拒绝采样的结果仍保留。温度尖峰及内部热状态依然存在，
有效采样不等于散热通过。工具不会替用户重启。


## Q71 tracefs registration qualification, 2026-10-16

A private tracefs backend was tested on the current 7.2.9-arch1-1 boot using registration-only mode. The first v3 trial rejected the actual dynamic pathname field format; cleanup completed and the before/after runtime guard matched. The frozen failure evidence is retained. Linux's dynamic string field is `__data_loc char[]`, size 4, signed 1; the earlier unsigned-only check was wrong.

A separate v4 fixes that exact format contract and durably records the original definition, event ID and raw format before validation. All 8 entry/return probes registered and passed actual format checks, producing 24 retained audit records. Probe hit/miss counts remained 0/0 because none were enabled. All owned definitions and the private instance were removed; global/foreign tracing state, five protected configuration files, CPU online state, module-name list and the daemon/GUI process identities matched before and after. Backend final duration was 0.792 seconds; wrapper wall time was 2.053 seconds. The v4 filesystem fixture suite passed 45 tests, while the actual registration/cleanup wrapper tracking was complete with no gaps or unresolved events.

At this registration-only stage, observation, decoding of real captured events, real signal/crash recovery and enabled-probe overhead were unverified; the later v5 observation below adds a separate, limited qualification; no MSR samples were collected. No AML, EC query or SMI was deliberately triggered. The dynamic TCC-offset writer and thermal spikes remain unresolved. Both the failed v3 and successful v4 receipts have independently verified recovery archives.


### V5: one actual 3-second trace-only observation

The first preflight stopped before launching a supervisor or registering probes because `ib_core` had appeared since the v4 reference. CPU online state was unchanged. Its installed kernel package ownership, vermagic, srcversion and three runtime ELF note sections were independently matched; boot/BTF, five protected files and six target driver notes remained unchanged. A separate reference records only the additional module-name entry. The original v4 reference and the failed preflight are retained; the loader responsible for `ib_core` is unknown. No module was loaded or unloaded by this reconciliation.

One new observation using that explicit reference passed: 244 records comprised 8 method-attempt entry/return pairs, 8 AML-parse pairs and 106 port-write pairs; handler probes had zero hits. All targeted calls paired on their threads, all eight probe miss counters and all 24 CPUs' overrun/commit-overrun/dropped counters were zero. Actual timestamps have 1-microsecond resolution. Observed method paths were battery `_BST`, two `_TMP` paths and AC `_PSR`; these return fields are ACPI execution statuses, not returned temperature values. No `_Q71` handler was captured. Port writes were 104 requests to `0x186d` with value `0x40`, plus two to port `0x80` with value `0xb2`, all width 8. The latter is not a write to port `0xb2`, and the target `0xb2/0x99/8` sequence was absent.

The wrapper completed in 5.769 seconds, with no timeout, signal or recovery. All owned probes and the private instance were removed. Captured guards, including five file identities/hashes, all loaded module notes, CPU online state, four process identities and global tracing state, matched before/after. This proves normal observation/decoding for the captured three function families; the handler payload path had no real hits. Real forced-signal/crash cleanup remains tested only with fixture backends. No MSR samples or artificial AML/EC/SMI triggers were used, and this window did not overlap the completed control-center build. It does not establish the TCC-offset writer or a thermal fix.

The command succeeded, but file tracking was partial: zero recorded gaps, 166 retained unresolved events across 77 paths. Original diagnostics and raw trace/format/counter/guard evidence were archived without rewriting that producer status; the separate archive copied 44 original files and independently verified them.

### Current thermald recovery evidence

A separate bounded journal window from 18:40 through 18:45 on this boot contains 675 records, below the 2,000-record cap. It has 203 temperature-bearing control actions, with RAPL action temperatures spanning 68–100°C. All 14 forced-to-min actions immediately followed an action temperature below 81°C. Retained source computes the polling trip as the lowest 90°C trip minus 10% (81°C), then resets cooling devices to minimum cooling; the configured RAPL minimum-cooling state is 55W. This explains the observed request sequence, without proving a physical power-limit readback, a temperature between action logs or the cause of fast temperature changes.

A distinct source defect was reproduced using actual sensor/zone/trip/RAPL functions with mocked sysfs and engine services: failed, empty or negative sensor reads became zero and could relax the simulated 27.5W limit to 55W. The checked-read private prototype retains cooling for invalid reads in that zone; valid zero remains a successful reading and preserves the original release behavior. Multi-sensor failures are collected before zone updates. Virtual sensor dependency updates, error propagation and adaptive resource cleanup have isolated regression coverage. A separate preference-state delta preserves successful `zone_temp` replay state. Neither prototype nor delta is publicly packaged, linked into an installed daemon or hardware-accepted. Other zones/owners can still adjust shared cooling devices. The journal window contains no zero-temperature action establishing this defect as the cause of the actual spikes.


## 2026-10-16：温控候选加入虚拟传感器重启保护

真实 pthread 与实际 zone active/inactive、read_zone_temp 入口的隔离测试复现：
先缓存合法低温 0 或 75°C，停用 worker 后让底层读取失败，重新启用且首采尚未执行时，
旧实现仍可向温区提交旧低温。首次失败采样完成后才变为无效。通知函数仍是记录替身，
没有把该测试写成实机 RAPL 调整，也没有将它归因为当前日志里的温度突升。
第三补丁在真正创建新 worker 前持 sample_mutex 使缓存失效；重复启用活动 worker 不清掉有效缓存。
真实创建/退出、属性释放、创建失败后重试、重复启停及活动对象析构也有回归；
受控等待替身不能证明生产 sleep 的退出延迟或任意并发 enable/disable 安全。

三补丁 `thermald-mechrevo 2:2.5.13-2` 已完成全新生产编译和链接（38.744 秒），
12 项公开回归经独立复测通过（17.269 秒），包括旧源码精确失败对照与 prepared 源码比对。
候选为 x86_64_v3、439327 字节，SHA256 `5478e28da267a76e59c9c92928ee0b8d80e6cb0757a53ea1eb299bb23c162661`。
独立审查核对 9 个包内文件、17 个公开冻结文件、171 个 prepared 源文件；
48 文件恢复备份逐 SHA/大小/权限/属主独立读回通过。旧双补丁候选不包含重启保护，保留作历史证据。
候选未安装、未执行新 daemon，当前 stock thermald 和功耗配置保持原样。

构建、打包命令成功不等于文件跟踪完整：新生产跟踪 partial，6 gaps / 382 unresolved；
打包 partial，4 gaps / 2 unresolved；独立公开回归跟踪 partial，0 gaps / 112 临时删除事件。
完整备份读回不会补齐这些历史缺口。候选不带安装脚本或 hooks，普通系统 hooks、D-Bus 激活、
现有自定义配置保存及服务恢复仍须受控核验，不能据此宣布硬件散热改善或安装资格已通过。


## 2026-10-16：温控公开回归补齐至 25 项

自适应温控启动失败、真实 FD/别名资源释放、无效温度与普通不匹配的区分、条件短路，
以及虚拟传感器依赖环、滤波/缓存事务和真实线程并发，现在有固定 GPL 原源的便携公开测试。
使用实际三补丁 prepared 输入独立运行，25 项全部通过、无跳过（31.547 秒）。
配方 check() 强制四类测试文件存在并执行完整模式；源码/变体负例要求特定断言。
这些回归仍不是整个 daemon 或物理散热验收，DBus 分配失败清理的便携覆盖仍待补。

pkgrel3 仅扩展构建测试门禁并重新封装，三个生产补丁和 daemon 与 pkgrel2 完全相同；
没有重复生产编译。候选 439443 字节，SHA256 `e3c5eb728bb2eb40102d0f1c331b8244375854076ec811314a7857fa9cf4662d`，
22 文件恢复材料已逐项独立读回。测试跟踪 partial（0 gaps / 248 临时删除事件），
重包跟踪 partial（2 gaps / 4 unresolved），原始诊断保留。候选尚未安装或运行。

PackageKit 自然恢复 inactive/PID0 后，新只读 preflight4 完整通过了开始/结束的
boot、六个运行模块身份、五份保护配置、服务/进程和 hook 核对；没有停止 PackageKit 或刷新 pin。
它只证明当次检查的一致性，不能阻止将来的 D-Bus 再激活，也不代表安装/激活已获资格。
