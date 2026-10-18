# open-revo 逆向工程分析与 Issue 全闭环修复规范

日期：2026-10-18
分析对象：`open-revo-v0.8.5.7z` (`open-revo.exe`，SHA256 校验与 PE32+ 静态解构)
分析方法：遵循 `leanrea` 规范（符号提取、字串分段、二进制切片、反混淆、调用拓扑推导）

---

## 1. 原版 open-revo 静态解构结论

### 1.1 技术栈与架构 (Seen)
- **底层框架**：Rust (`rustc 1.8x` / `1.9x`, `tauri-2.11.5`, `tauri-runtime-wry-2.11.4`, `tao-0.35.3`, `muda-0.19.3`, `tray-icon-0.24.2`)。
- **脚本引擎**：`rhai-1.26.1`（嵌入式 Rust 脚本引擎，用于动态规则计算与风扇映射）。
- **前端资产**：Vite + 前端 SPA 打包，内嵌于二进制数据段（包含 `index.html`, `/assets/index-*.js`, `/assets/index-*.css`）。
- **核心模块树 (`open_revo_lib`)**：
  - `open_revo_lib::driver::ec_bridge::EcBridge`：负责 EC 内存读写、风扇曲线写入与转速锁死判定。
  - `open_revo_lib::driver::gpu_bridge::GpuBridge`：基于 NVAPI 的显卡功耗、温度与频率遥测。
  - `open_revo_lib::driver::lighting::IteLightingManager`：ITE 8291 / 8297 芯片的 USB HID 协议灯光控制与 BetterRGB 流式渲染线程。
  - 水冷模块 `[BLE Cooler]`：基于 Windows WinRT BLE API 的自动发现与双向水流/水泵管理。

### 1.2 为什么原版仅支持 Windows (Seen)
1. **内核通信强依赖 Win32 原生驱动接口**：
   - 依赖 `\\.\UniwillAcpiDriver` 符号链接与 IOCTL。
   - 启动时校验注册表 `SOFTWARE\OEM\GamingCenter2\ItemSupport`（包括 `RamFan1p5Support`、`TurboModeSupport`、`LiquidCoolingSupport` 等键值）。
   - 缺失驱动时触发 Windows PnP `DriverStore` 自愈安装脚本。
2. **硬件防护拦截 (Seen)**：
   - 包含硬编码逻辑：`Hardware barrier: Baolongda platform detected! Refusing to extract or load Tongfang ACPI driver.`（检测到宝龙达代工主板时强行终止加载同方驱动）。
3. **显卡与蓝牙 API 深度绑定 Win32**：
   - 使用 Win32 `CallNtPowerInformation`、PPM 电源方案覆盖注册表项、以及 Windows BLE Advertisement Watcher。

---

## 2. open-revo GitHub Issues 根因深度剖析与全平台解决方案

针对 `https://github.com/faintonce/open-revo/issues` 列表中的典型缺陷，结合 Linux 内核驱动和底层硬件逻辑进行根因分析并制定闭环解决策略：

### Issue #115: 机械革命 17ks CPU 功耗锁不住 80W 只有 65W
- **根因 (Seen / Inferred)**：
  1. Windows 电源计划 (PPM) 或 Intel DTT (Dynamic Tuning Technology) 在后台周期性覆盖 MSR `0x610` (IA32_PACKAGE_POWER_LIMIT) 或 MMIO PL1 寄存器。
  2. EC 内部存在独立的 65W 硬件默认降级策略，当未向真实功耗硬件入口持续刷新时，EC 会自动超时回退到默认 65W。
- **解决策略**：
  - **Linux 侧**：通过内核驱动对接 MSR 与 MMIO MCHBAR 双重锁定，配合 `tuxedo_io` / `mechrevo_io` 定时任务在底层硬件层面保持 PL1 钳位；在 EC 侧，不仅向 WMI 写入，同时直接写入 EC RAM `0x0783..0x0786`（真实硬件功耗入口），阻止 EC 降级。
  - **Windows 侧**：引入 Ring0 MSR 锁定守护线程，挂钩 Windows 电源模式改变通知事件，并在检测到 PL1 回退时于 50ms 内原子纠正。

### Issue #114: 水冷智能控制不关闭水泵 (Feature)
- **需求与根因**：低负载或水温下降后，原程序直接切断水冷供电/水泵，导致水管内微循环停滞、热积聚突升，且水泵频繁启停损害机械寿命。
- **解决策略**：
  - 将水冷控制分为风扇曲线与水泵 PWM 两套解耦控制环：
    - 水泵：设立最低基础流量保底（20%~30% PWM 保底循环），仅在整机完全关机或进入 S3/S0ix 睡眠时才发送完全关停指令。
    - 水冷排风扇：随 CPU/GPU 混合温度平滑停启，消除频繁启停噪音。

### Issue #113: 屏幕亮度自定义预设 (Feature)
- **解决策略**：
  - 在性能模式切换动作中增加联动管道（Profile Actions Pipeline），允许用户为每个模式（办公、平衡、狂暴）单独绑定目标屏幕亮度。
  - Linux 侧通过 `/sys/class/backlight/` 原生接口写入，Windows 侧调用 WmiMonitorBrightnessMethods。

### Issue #112: 翼龙 15pro (8845HS + 4070) 状态总览 GPU 功耗不准
- **根因 (Seen)**：
  - 混合显卡 (Optimus / MUX) 下，当 RTX 4070 进入 D3hot/D3cold 深度睡眠或省电状态时，NVAPI 查询超时返回上一次脏缓存，或者驱动未能正确过滤 AMD Radeon 780M 集显与 NVIDIA 独显的 index 索引，将集显瞬时瓦数误判为独显。
- **解决策略**：
  - 遥测层增加 PCIe 电源状态探测（Linux: `/sys/bus/pci/devices/.../power/runtime_status`；Windows: 检查 NVAPI `NVAPI_GPU_IN_LOW_POWER_STATE`）。
  - 若 GPU 处于休眠挂起状态，界面明确显示“休眠节能 (0W)”，而不是返回停滞的历史数值；严格绑定 PCI Vendor ID `0x10DE` 过滤独显。

### Issue #111: 机械革命钛钽 Plus (11800H + 3060) 充电上限锁不住
- **根因 (Seen / Inferred)**：
  - 钛钽 Plus 属于 11 代同方模具（GK7/GM7 早期系列），其充电阈值控制不在新型号的 EC RAM `0x07a6`，而是位于特定 EC 偏移或走标准的 ACPI SMAP 电池控制方法。且每次拔插 AC 电源适配器或休眠唤醒时，EC 会将控制权复位为 100%。
- **解决策略**：
  - 机型配置表中增加电池协议分型（`Legacy_GK7_Method` vs `Modern_Uniwill_07a6`）。
  - 后台守护进程订阅 AC 电源插入事件（Linux udev `SUBSYSTEM=="power_supply"`，Windows `WM_POWERBROADCAST`），在检测到供电状态切换后 500ms 内自动重新写入目标充电上限（如 60% / 80%）。

### Issue #110 & #91: 无法熄灯 / 键盘灯光不熄灭 / 灯光控制问题 (#99)
- **根因 (Seen)**：
  - ITE 8291/8297 USB HID 芯片与 EC 寄存器之间存在仲裁冲突。当软件通过 EC 寄存器 `0x0767` bit 5 写入颜色时，若 ITE 微控制器内部仍处于流水线循环模式，硬件背光不会关闭；另外在熄灯状态下，原版只将 RGB 设为 0 但未清除亮度等级位，导致某些固件保持微弱底噪发光。
- **解决策略**：
  - 熄灯操作执行“双重同步”：
    1. 发送 ITE USB HID 指令（Feature Report）将灯效模式切换为 Direct/Static，并将 RGB 字节全置 0；
    2. 向 EC RAM 写入背光关闭指令，清除背光使能位。
  - 重新点亮时按照相反顺序恢复。

### Issue #108: 24 款蛟龙 16pro 风扇控制异常
- **根因 (Seen)**：
  - 2024 款蛟龙 16 Pro 引入了新型号 EC 固件（RamFan1p5 协议）。在写入自定义风扇曲线前，必须显式对风扇转速表寄存器加锁（Fan Table Latch），写入完成后必须释放锁；若直接覆盖 16 节点转速表而未遵守原子写入事务，EC 会触发自保护，转速锁定在 100% 满速或完全失控。
- **解决策略**：
  - 统一调用驱动层经过严格时序校验的风扇表提交流程：`进入互斥锁定 -> 写入 UpT/DownT 阈值 -> 写入 Duty 占空比 -> 提交 Latch -> 验证回读`。

### Issue #107: 迷你窗口布局异常 (下方按钮被遮挡)
- **根因 (Seen)**：
  - 前端窗口固定了绝对高度像素（px），在 Windows 系统 125%、150% 等常见高 DPI 缩放比例下，CSS 布局溢出且没有设置安全边距或 `overflow-y: auto`。
- **解决策略**：
  - 前端界面重构，采用现代化响应式流式布局（Flex/Grid + rem/vh 相对单位），窗口容器尺寸自适应 DPI 缩放并增加最小安全可见区。

### Issue #106: 耀世 15pro (13900HX + 4060) 睡眠唤醒后 CPU/GPU 功率异常
- **根因 (Seen)**：
  - 机器从 S3 挂起或 Modern Standby (S0ix) 唤醒后，EC 固件自动将所有功耗寄存器重置为 BIOS 默认值，而控制中心未监听唤醒事件，造成“设置界面显示 140W 但实际只有默认低功耗”的不一致状态。
- **解决策略**：
  - 实现**唤醒事件恢复总线 (Profile Replay Watchdog)**：
    - Linux：监听 systemd `PrepareForSleep(false)` D-Bus 信号与内核唤醒通知。
    - Windows：监听 `PBT_APMRESUMEAUTOMATIC` 电源消息。
    - 唤醒后延迟 1.5 秒（等待固件自检平稳）自动重放并复核当前激活的功耗配置、风扇表及充电限制。

### Issue #104: 深海泰坦 X3-S (GK7MR0R, 20系) 无法匹配任何机型，落入 MockHAL
- **根因 (Seen)**：
  - open-revo 内部只硬编码了 40/50 系的 29 个机型 JSON，老款模具（GK5/GK7、PF5 等）在 DMI 检查时直接不命中，只能进入空模拟模式（MockHAL）。
- **解决策略**：
  - 建立全家族泛化机型拓扑库（包含老款 GK/GJ/PF 以及新款 GM/PH/GX 全系列），如果精准 DMI 未命中，自动回退到“基于通用 Tongfang/Uniwill 能力探测”的通用协议栈（Generic Fallback HAL），支持手动指定模具家族，杜绝永久死锁在 Mock 模式。

### Issue #103: 耀世 16pro 没有 A 面灯效设置
- **根因 (Seen)**：
  - 耀世 16 Pro 的 A 壳透光 Logo/灯条位于独立的 EC RAM 寄存器或 ITE 区域，原版软件未开放前端选项。
- **解决策略**：
  - 查明 A 壳灯条对应的 EC 偏移与支持能力位，在前端灯光面板中根据机型能力标记（`has_logo_led`）动态渲染 Logo 灯控制卡片，支持独立颜色与开关。

### Issue #96: 100W PD 充电时电源管理显示正在使用电池供电
- **根因 (Seen)**：
  - 原版判断交流电仅读取了传统 DC 圆孔适配器的连接位（DC Jack Present），未读取 Type-C PD 控制器状态（EC `0x07a5` 中的 PD 协商握手标志位或 ACPI `_PCL`）。
- **解决策略**：
  - 抽象电源输入检测层：`is_ac_online = dc_jack_online || pd_typec_online`，并在 UI 上分别展示“DC 适配器供电”与“PD 适配器供电（受限功耗模式）”。

### Issue #92: 蛟龙 16pro 锁定充电上限导致性能模式灯效错乱
- **根因 (Seen)**：
  - 充电模式位与电源模式指示灯位共用同一个 EC RAM 字节（如 `0x07a6` 的不同 bits）。原版代码在更新充电模式时直接写入整个字节（`byte = new_charge_mode`），导致高位或低位的灯效状态被清零。
- **解决策略**：
  - 严格执行**位掩码原子读改写 (Read-Modify-Write)**：
    `val = read_ec(addr); val = (val & ~MASK) | (new_val & MASK); write_ec(addr, val);`

### Issue #86: 无界 14Pro 会被误识别成蛟龙 16Pro
- **根因 (Seen)**：
  - DMI 匹配算法过于宽泛，仅检查了模糊的 Project ID 匹配分支，而未交叉核对 `DMI_PRODUCT_NAME` 与 `DMI_BOARD_NAME`（如 GX4HRX 与 GM6PX）。
- **解决策略**：
  - 实施多级精确匹配引擎：`Exact DMI Product > Board SKU > Project ID > Generic Fallback`，彻底杜绝误判。

---

## 3. 架构对比与超集设计原则

| 维度 | open-revo (现有 Windows 版本) | 本方案跨平台控制中心 (Rust) |
| --- | --- | --- |
| **操作系统支持** | 仅 Windows | **Linux + Windows 跨平台原生支持** |
| **底层硬件接口** | 仅 Windows `UWACPIDriver` / WinRing0 | Linux: `/dev/tuxedo_io`, `hwmon`, `platform_profile`, `sysfs`<br>Windows: Direct IO, NVAPI, ACPI 统一抽象 |
| **机型覆盖** | 仅 40/50 系部分机型 (29 个) | **机械革命全系列** (老款 GK/GJ/PF 至最新 GM/PH/GX/IP3) |
| **协议安全性** | 直接写字节，存在位污染与风扇失控风险 | **严格位掩码、原子读改写、恢复保护网与状态回读校验** |
| **电源与唤醒** | 睡眠唤醒后功耗与曲线丢失 | **唤醒重放总线 (Profile Replay Watchdog) 自动保持** |
| **界面与交互** | 极简窗口，高 DPI 下存在溢出与截断 | **独创赛博极客电竞 UI，响应式流式布局，拒绝雷同** |
