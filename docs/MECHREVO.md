# 机械革命支持范围

这是社区维护分支，与 MECHREVO、TUXEDO 没有官方授权或支持关系。上游作者、版权和 GPL 许可证全部保留。

## 当前改动

基线为 TUXEDO 正式版本 `v4.24.0`。仅在
`src/tuxedo_compatibility_check/tuxedo_compatibility_check.c` 的品牌检查表中增加三个独立条目：

- `DMI_SYS_VENDOR` 包含 `MECHREVO`；
- `DMI_BOARD_VENDOR` 包含 `MECHREVO`；
- `DMI_CHASSIS_VENDOR` 包含 `MECHREVO`。

`DMI_MATCH` 是区分大小写的包含匹配，三个条目是「任意一个命中即可」，不是要求三个字段同时满足。
不添加泛化的 OEM 品牌、不关闭检查，也不添加任意绕过参数。
原有 TUXEDO 条目和旧 CPU 平台跳过检查的逻辑不变。

这与 AUR `mechrevo-drivers-dkms` 的品牌放行思路一致，但这里直接维护源代码及上游更新，
不依赖每次下载上游后再应用外部补丁。

## 能力与边界

上游包括 Clevo、Uniwill、ITE 等平台模块，可能提供 Fn 键、键盘背光、传感器、风扇和功耗接口。
实际可用性还取决于具体型号、控制器和 BIOS/EC 固件；模块内的独立识别规则仍会生效。
通过统一品牌检查不代表每个模块都能匹配，也不代表这些接口对某台机器是安全的。

当前没有增加机械革命型号专属 EC 寄存器、风扇曲线、CPU/GPU 功耗参数、睡眠或续航修复。
不承诺自动提高性能，不承诺自动降低温度。没有实机验证的机型不得标为「完整支持」。

尤其注意：上游明确提醒，不兼容的 EC 操作可能损坏设备。不要在未知型号上强制写风扇、
充电阈值或功耗控制；不要运行网上来源不明的 EC 写入脚本。

## 查看本机信息（只读）

这些命令不加载模块、不写 EC，也不需要完整 `dmidecode` 输出：

```sh
uname -r
for field in sys_vendor product_name product_version board_vendor board_name chassis_vendor bios_version; do
    printf '%s: ' "$field"
    cat "/sys/class/dmi/id/$field" 2>/dev/null || true
done
```

反馈时附上：型号、CPU/GPU、BIOS 版本、发行版及内核、已有驱动版本，
以及背光/Fn/传感器等每个功能的具体结果。不要上传序列号、UUID、MAC 地址或未脱敏的完整系统报告。

## 构建验证

需要对应内核的 headers、编译器和 make。Arch 示例（选择已安装内核的对应 headers）：

```sh
# 安装依赖会更改系统；此命令仅为用户自行操作的示例。
sudo pacman -S --needed base-devel linux-headers
make -j2 KDIR=/usr/lib/modules/$(uname -r)/build
```

构建成功仅证明该内核下能编译，不证明硬件兼容。不建议为了试验直接 `make install`：
上游 install 目标会写系统模块及 `usr/` 中的规则，且不提供本分支的包管理卸载保障。
优先在明确兼容型号上通过发行版包管理/DKMS 打包测试，避免同时安装多个提供同名模块的包。
本项目暂未发布自己命名的 AUR 包；AUR 上现有同名包不会自动变成这个仓库的版本。

## 维护原则

1. 先保留小而可审核的兼容性补丁。
2. 新型号适配需要型号/固件范围、只读探测依据和验证记录。
3. 风扇、功耗及充电控制单独评审，不以品牌放行代替安全检查。
4. 不重命名内核模块和用户态接口，避免破坏已有软件依赖；这也意味着不能与原版并装。
5. 不把本地编译结果、源码提交或 GitHub 发布说成已安装或已通过实机测试。
