# 本地 Arch 控制台包

`mechrevo-console` 独立安装 Rust CLI、原生 GUI、受限 OEM helper 与风扇 helper，不依赖 DKMS 包，也不替换
`mechrevo-control`、TCC 或已有热键守护进程。安装不新增服务、不修改风扇曲线，
GUI 从应用菜单中的“机械革命控制台”启动。

当前配方固定源码提交 `58e8d9c0c1b63dcb7a4af5f0d5c877a6214413e5`，包版本为
`0.1.0.r163.g58e8d9c-1`。这是本地部署配方，不表示已经发布到 AUR。
后续更新应一并修改提交、版本和部署验证记录，不能只替换安装目录里的二进制。

本配方包含风扇编辑器与会话 helper；源码回归见
[界面与代理验证收据](../../../docs/verification/fan-gui-broker.json)。实际安装、驱动加载与硬件效果由部署收据分别记录。

构建需要系统已有 Rust 1.95 以上、Git、pkgconf、desktop-file-utils、libxml2，以及锁文件
对应的 Cargo 缓存。Cargo 使用 `--release --workspace --all-features --locked --offline`，
缺少依赖时直接失败，不安装依赖或修改锁文件。内嵌字体和型号目录随二进制分发；
中文字体通过 fontconfig 使用系统回退字体，可选安装 `noto-fonts-cjk`。
运行依赖包含 `polkit`。桌面会话需有可用的授权代理才能显示管理员认证；本包不安装
新的常驻服务，也不自动启动授权代理。

在此目录执行；将本地仓库路径设为实际位置，Git 只取固定提交，工作区改动不进入包：

```sh
MECHREVO_CONSOLE_SOURCE=file:///home/lightjunction/Documents/GITHUB/mechrevo-drivers \
CARGO_BUILD_JOBS=2 \
nice -n 15 ionice -c 3 makepkg
```

不设置 `MECHREVO_CONSOLE_SOURCE` 时，makepkg 从项目 GitHub 仓库取得同一固定提交。
`CARGO_TARGET_DIR` 可指定绝对路径，以复用受跟踪的构建缓存；未设置时，产物在
makepkg 的 `src/cargo-target` 中。不要并行复用正在被其他 Cargo 构建写入的目录。

检查包内容和部署材料后，通过 pacman 安装本地产物：

```sh
pacman -Qlp mechrevo-console-0.1.0.r163.g58e8d9c-1-x86_64.pkg.tar.zst
sudo pacman -U ./mechrevo-console-0.1.0.r163.g58e8d9c-1-x86_64.pkg.tar.zst
mechrevo-console --version
mechrevo-console status --json
```

安装目标为 `/usr/bin/mechrevo-console`、`/usr/bin/mechrevo-console-gui` 和
`/usr/share/applications/best.lmm.mechrevo-console.desktop`；desktop ID 与 GUI
的 Wayland app ID 一致。另安装 root 归属的
`/usr/lib/mechrevo-console/oem-helper`（0755，无 setuid）和
`/usr/share/polkit-1/actions/org.mechrevo.console.oem.policy`（0644）。
风扇 binary `mechrevo-console-fan-helper` 安装为
`/usr/lib/mechrevo-console/fan-helper`（0755，无 setuid），独立策略为
`/usr/share/polkit-1/actions/org.mechrevo.console.fan.policy`（0644）。
固定入口为 `fan-helper --serve`，不会将构建产物同时放进 `/usr/bin`。
许可放在 `/usr/share/licenses/mechrevo-console/`，
包含项目 GPL 许可声明和两份内嵌字体的 OFL；控制台说明放在对应的 `/usr/share/doc/` 目录。

`prepare()`、`check()`、`package()` 均核对固定提交、干净工作树及必要文件的 Git 跟踪和普通文件类型，
包括新 helper 源码及两份策略。`check()` 验证 desktop 文件及两份策略 XML，不联网读取 DTD，
逐个核对唯一 action、固定 helper 路径及唯一的授权默认值。
风扇 action 另外要求 `org.freedesktop.policykit.exec.argv1=--serve`；
OEM action 与既有权限保持独立，不接受其他授权注解。
`package()` 在写入前重新执行这些检查，并要求四个真实可执行产物为非符号链接普通文件，
将两个 helper 改名放到上述固定路径，再核对各 helper／策略的字节、普通文件类型与 0755／0644 权限。

OEM 枚举与预览不提权。只有显式应用时，普通用户通过 `pkexec` 请求受限 helper
执行类型化计划；活动会话策略为 `auth_admin_keep`，管理员认证可短时缓存，非活动
会话为 `auth_admin`，`allow_any` 为 `no`。不赋予任意路径写入权限，不放宽 sysfs
权限，也不让整个 GUI 以 root 运行。授权拒绝、缺少代理或 helper 不可用均须作为
失败报告；不能自动重放旧计划。

风扇预览和普通遥测不启动提权 helper。只有用户点“应用”时才通过 `pkexec` 启动
固定路径的 `fan-helper --serve`，在当前 GUI 会话中保留受限命令通道；
后续应用仍需校验每份类型化计划，不能因通道已授权而接受任意寄存器、路径或命令。
它使用独立的 `org.mechrevo.console.fan` action：活动会话 `auth_admin_keep`，
非活动会话 `auth_admin`，其他情况 `no`；不会放宽 OEM action 或让 GUI 以 root 运行。
缺少 helper、授权失败、协议错误和进程退出应分别报告。安装不会自动启动此进程，
不会添加 systemd 服务或自动应用风扇曲线；实际关闭和风扇恢复行为须另做运行及硬件验收。

安装后应核对 `pacman -Qkk mechrevo-console`、包内与安装后二进制的摘要、CLI
只读状态及原生 GUI 启动结果。还要核对 helper／策略的 root 归属、0755／0644 模式、
固定执行路径，以及取消授权时没有硬件写入。实际 OEM 写入、回读和恢复另作硬件
验收；LED 请求缓存不证明物理颜色，充电档位不证明固定百分比停充。
安装包成功不代表 GUI 或硬件控制已验收；运行中的
内核模块也不会因安装控制台而更新。回滚使用保留的上一个本地包执行 `pacman -U`；
首次安装可用 `pacman -R mechrevo-console` 卸载，用户 GUI 偏好不会由该包删除。

离线打包回归通过真实 Bash `prepare()`／`check()`／`package()` 运行到临时 staging 目录，
覆盖两份策略与两个 helper 的实际安装内容，以及缺失文件、错误路径／授权、重复 action、
符号链接和安装后字节／权限漂移的拒绝。它不执行 `pkexec`、真实 helper 或硬件控制：

```sh
mkdir -p .local-tests
python -B -m unittest discover -s tests -p test_console_fan_packaging.py
```
