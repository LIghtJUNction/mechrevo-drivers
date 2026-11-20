# Arch / AUR 控制台包

`mechrevo-console` 提供 Rust CLI、原生 GUI、OEM helper 和 `mechrevo-control`
桥接工具。控制台与 DKMS 包独立安装；内核驱动更新、已加载模块和控制台版本分别管理。

当前配方固定源码 `97e66567bc779d37eb032f0e29a37fa9973658e6`，版本为
`0.1.0.r1553.g97e6656-1`。固定提交保证每次构建使用同一份源码，工作区改动不进入包。
[公开发布记录](https://github.com/LIghtJUNction/mechrevo-drivers/releases)与本机安装验收分别记录。

## 构建与安装

Arch Linux 需要 Git、Rust、pkgconf、desktop-file-utils 和 libxml2；最低 Rust
版本以固定提交的 `console/Cargo.toml` 为准。运行依赖包含 Python、systemd、polkit
和图形库，具体包名由 `PKGBUILD` 声明。中文显示可安装 `noto-fonts-cjk`。

在此目录构建，再用包管理器安装：

```sh
makepkg -s
pacman -Qlp ./mechrevo-console-0.1.0.r1553.g97e6656-1-x86_64.pkg.tar.zst
sudo pacman -U ./mechrevo-console-0.1.0.r1553.g97e6656-1-x86_64.pkg.tar.zst
mechrevo-console --version
mechrevo-console status --json
mechrevo-control touchpad status
```

使用已有本地仓库时，设置 `MECHREVO_CONSOLE_SOURCE=file:///absolute/path/to/mechrevo-drivers`。
`CARGO_TARGET_DIR` 可指定绝对缓存路径，同一目录应由一个 Cargo 构建使用。
`prepare()` 执行 `cargo fetch --locked` 准备依赖；构建使用
`--release --workspace --all-features --locked --offline`。缓存完整时可设置
`CARGO_NET_OFFLINE=true`，锁文件不随构建更新。

## 触摸板

触摸板控制使用 Linux input 的 `inhibited` 属性。udev 负责识别触摸板，helper
写入后核对每个设备的实际状态。命令和热键共用这一入口，不依赖特定桌面的命令或
设备名称。系统需要提供 udev、logind、polkit 和该内核属性。

```sh
mechrevo-control touchpad off
mechrevo-control touchpad on
mechrevo-control touchpad toggle
```

查询直接读取内核，不提权。变更由 `pkexec` 调用
`/usr/lib/mechrevo-control/touchpad-helper`：活动本地用户可控制自身唯一活动 seat
上的触摸板；远程或非活动会话不获得该权限。该独立策略只授权固定 helper，OEM
设置继续使用原有管理员认证。

状态保存在 root 管理的 `/var/lib/mechrevo-touchpad/`。设备到达时，由
`mechrevo-touchpad-restore@.service` 的独立实例恢复状态；休眠恢复时由
`/usr/lib/systemd/system-sleep/mechrevo-touchpad` 调用同一 helper。
锁保护状态变更，失败会尝试恢复已修改的设备并报告回滚错误。卸载前会重新启用本工具
保存为禁用的触摸板。旧共享目录里的 I2C 状态不自动导入。

`on` 恢复内核输入事件。桌面自己的触摸板禁用设置独立生效；若使用过旧桌面脚本，
需先把桌面的触摸板设置恢复为启用。设备缺失、缺少内核属性或写入回读失败都会返回
错误。旧工具若已解绑设备，应先用旧工具恢复或重启，再使用新命令。

## 性能热键

选择一个性能键处理者，避免一次按键切换两次：

```sh
# TCC profile 与性能灯同步；需要 tccd 才能切换性能档位
systemctl --user enable --now mechrevo-gaming-hotkey.service

# 或使用 EC-only；只监听性能键，不处理触摸板键
systemctl --user disable --now mechrevo-gaming-hotkey.service
sudo systemctl enable --now mechrevo-ec-hotkey.service
```

用户服务在 tccd 不可用时仍可处理触摸板热键。安装包不自动启用这两个常驻服务。
EC-only 控制仍限于受支持的设备接口，模式参数和硬件验证范围见
[性能档位说明](../../../docs/PERFORMANCE-PROFILES.md)。

安装时仅迁移未修改的 TUXEDO 默认 profile，保留用户参数。五档 GUI 控制需要可信、
有效的 tccd 配置。OEM 应用由受限 helper 执行类型化计划；活动会话使用
`auth_admin_keep`，非活动会话使用 `auth_admin`，`allow_any=no`。授权拒绝或 helper
不可用会作为失败返回。GUI 本身以普通用户运行。

## 包内容与验证

主要入口为 `/usr/bin/mechrevo-console`、`/usr/bin/mechrevo-console-gui`、
`/usr/bin/mechrevo-control`。两个 helper 安装于各自固定的 `/usr/lib/` 目录，
归 root 所有、模式为 0755、无 setuid；polkit 策略为 0644。desktop ID 与 Wayland
app ID 均为 `best.lmm.mechrevo-console`。许可目录保留项目 GPL 和内嵌 Manrope 字体 OFL。

配方检查固定源码、文件类型、desktop 文件、策略的 action/路径/权限，并验证打包后
helper 和策略的字节、权限。安装后运行 `pacman -Qkk mechrevo-console`，再检查普通
用户 CLI、GUI 和实际硬件结果。以前的验证记录集中在
[verification](../../../docs/verification/)，其中每份记录只对应列出的提交和安装版本。

回滚可用保留的旧安装包执行 `sudo pacman -U /path/to/old-package.pkg.tar.zst`。
卸载使用 `sudo pacman -R mechrevo-console`，包管理器保留用户 GUI 偏好。

## 公开二进制发布

公开的 Arch x86_64 安装包应在干净的官方 Arch 环境构建。C/C++ 使用
`-march=x86-64 -mtune=generic`，Rust 使用 `-C target-cpu=x86-64`；检查三个 ELF
的最低 ISA 要求及链接环境。AUR 本机构建尊重用户的 makepkg 设置。

发布附件包含安装包、配方源包、`SHA256SUMS`、OpenPGP 分离签名和维护者公钥。
从可信渠道核对指纹后验签并检查摘要。源码标签、构建来源和二进制摘要随发布记录保存。
