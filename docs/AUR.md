# Arch Linux / AUR 打包

2026-10-16 最新本地磁盘安装为 `4.24.0.r106.gaaf6b36-1`，两套内核各 28 个签名模块核验通过。
本地另有固定源码 `e389631` 的 `4.24.0.r112.ge389631-1` 候选：803项完整回归及两内核隔离DKMS验证通过，尚未安装。
本轮尚未更新 AUR；主启动条目已同步到经核验的 r106 签名 UKI，旧镜像保留。
当前运行模块保持旧版，尚未重启，启动与硬件验收仍待完成。
具体版本和证据见[本机记录](hardware/GM6PX0X.md)。下文发布记录按各节时间理解。


- 包名：[`mechrevo-drivers-dkms-git`](https://aur.archlinux.org/packages/mechrevo-drivers-dkms-git)
- 源码：本仓库的 `main`，不是 GitLab 开发分支，也不是第三方包下载后再打补丁。
- 打包定义：`packaging/aur/mechrevo-drivers-dkms-git/PKGBUILD` 与同目录的 `dkms.conf`。
- AUR 维护账号：`lightjunction`。

现有 `mechrevo-drivers-dkms` 由另一位维护者维护，与本包独立。
本包声明 `conflicts=('mechrevo-drivers-dkms' ...)`，同时提供对应的非 git 包名以满足依赖，
**不使用 `replaces`，不接管、不修改对方的包**。
由于内核模块及 udev/modprobe 文件名保持上游名称，本包也与原版 TUXEDO 驱动、旧版
`tuxedo-keyboard` DKMS 包及其 git 变体互斥，不能同时安装。

## 安装

先确认[支持范围与风险](MECHREVO.md)。品牌检查放行不代表实机兼容认证；
本包不会额外调整风扇曲线、功耗或 EC 寄存器。

选择与已安装内核对应的 headers。以标准 Arch 内核为例：

```sh
sudo pacman -S --needed base-devel linux-headers
# 使用已安装的 AUR helper，二选一。
yay -S mechrevo-drivers-dkms-git
# paru -S mechrevo-drivers-dkms-git
```

LTS / Zen / Hardened 内核分别使用 `linux-lts-headers` / `linux-zen-headers` /
`linux-hardened-headers`；自定义内核需要自己的匹配 headers。
headers 被列为可选依赖是为了避免强制安装某一种内核，不代表可以省略。

AUR 只发布构建配方，不提供预编译内核模块。安装后的源代码位于
`/usr/src/mechrevo-drivers-<版本>/`，由 Arch 的 DKMS hooks 为安装了 headers 的内核编译。
构建配方本身不执行 `sudo`、`make install` 或 `modprobe`，也不强制加载模块。
安装后可以通过 `dkms status` 检查状态；替换已加载的旧驱动后应重启，不能把磁盘上的包替换
当作运行中的模块已经切换。

卸载使用 `sudo pacman -R mechrevo-drivers-dkms-git`，由 DKMS hooks 移除对应模块。
不要手工删除 `/usr/src` 或 `/var/lib/dkms` 中的目录。

## 版本与发布

版本形如 `4.24.0.rN.g<commit>`：以 `main` 历史中最新的正式 `vX.Y.Z` 标签为基线，
`N` 是标签后的提交数。RC、beta 和不属于当前分支历史的标签不作为版本基线。
没有正式标签时回退为 `r<总提交数>.g<commit>`。

`pkgver()` 在构建时读取 Git 自动更新版本。本仓库保留的是配方模板；
AUR checkout 中的 `PKGBUILD` 版本字段与 `.SRCINFO` 应按发布时的源码提交重新生成。
`.SRCINFO` 是给 AUR 读取的包元数据，只发布到 AUR 仓库，不在源码仓库重复维护。
修改 `dkms.conf` 时必须同时更新配方中的 SHA-256，只有 Git 源使用 `SKIP`。

维护流程：

1. 在本机运行 `python3 -m unittest discover -s tests -v`，检查 ShellCheck、配方和模块列表。
2. 提交并推送已验证的源码/配方改动到 GitHub，核对 `origin/main`。
3. 在独立 AUR checkout 中复制本仓库的 `PKGBUILD`、`dkms.conf`。
4. 运行 `makepkg --cleanbuild`，不加 `--install`；核对生成的包、版本、依赖和源文件权限，
   使用 `namcap` 检查配方与包，并从包内源代码进行隔离的 DKMS 构建验证。
5. 运行 `makepkg --printsrcinfo > .SRCINFO`，审核三份文件后提交并推送到
   `ssh://aur@aur.archlinux.org/mechrevo-drivers-dkms-git.git`。
6. 核对 AUR 远端提交、网页/RPC 中的版本与维护者。只推送源码到 GitHub 不等于 AUR 已发布。

AUR 仓库仅提交 `PKGBUILD`、`dkms.conf`、`.SRCINFO`，不提交源码镜像、`src/`、`pkg/`、
日志或 `.pkg.tar.zst`。不为日常打包验收触发远程 CI 或自动审查。

## 首次配方验证

- 本地测试共 24 项通过（其中 5 项检查 AUR 配方）；Bash 语法、ShellCheck 与 `git diff --check` 通过。
- `makepkg` 成功生成包；从包内源代码进行隔离 DKMS 构建，在 `7.2.9-arch1-1` headers 下
  生成全部 27 个模块。测试使用独立的 source/DKMS/install 目录，关闭模块签名，未安装或加载模块。
- `namcap` 无错误，保留两个预期警告：包只包含源文件而声明 `x86_64`（驱动针对 x86 笔记本）；
  静态分析未识别 `dkms` 依赖的使用（实际由发行版 DKMS hooks 使用）。不能因此改成支持任意架构
  或删除 `dkms` 依赖。

以上首次配方测试不安装、不加载驱动，不访问 EC；本地编译成功不等于硬件功能验证。

后续已在旷世 16 Pro `GM6PX0X` 安装并加载 `4.24.0.r10.gc79a268-1`，
完成背光/Fn 锁接口切换恢复、传感器读取、九项不支持 TDP 查询、拒绝无效指针及不支持的写入、
重载电源值保持等验证。上一轮另通过探针确认同值设置不写 EC。
本地测试已增加到 146 项，两个内核各编译并安装全部 27 个模块。
另实测两只风扇的超范围输入均被拒绝，两次恢复自动模式成功；
37 项风扇顺序/故障及 12 项固定交错并发回归使用模拟 EC；
实机另通过两线程四次自动模式调用，真实睡眠及手动转速仍未验证。
配方递归安装 Markdown 文档，包含 `docs/hardware/`，保持路径及文件权限。
功能边界、现存 NVIDIA 日志及未测项目见[本机验证记录](hardware/GM6PX0X.md)。

2026-10-15 后续已发布并安装 `4.24.0.r16.gb549e06-1`，源码 `b549e06`，完整本地测试增至 201 项。
包内隔离 DKMS 及系统安装 hooks 均在两套内核各完成 27 模块构建；系统模块已签名安装。
仅 `tuxedo_io` 切换到新版本并实测模式能力拒绝路径，另外四个运行模块保持 r10。
绑定、按键任务、半截序列和 custom-profile 修复已写入磁盘，尚待下次启动激活与实机验收。
未进行睡眠或重启。AUR Git、未命中缓存的网页及 RPC 已核对；旧缓存一度仍返回 r10。
本轮安装前 r10 包和 54 个模块的恢复材料已独立解包校验，保存于统一 Backups 目录。

Git 包构建时会读取 `main` 的最新提交；AUR 网页版本与实际构建出的 `pkgver()` 可能不同。

2026-10-15 本轮发布并安装 `4.24.0.r23.gf1aa652-1`，包源码 `f1aa652`；
功能源码 `2c749ac` 的 281 项测试及两套内核各 27 模块全新构建通过。
包内独立 DKMS 和系统安装均为两个内核各 27 模块，系统模块已签名，包完整性及启动文件签名检查通过。
仅新 IO 运行，另外四个模块保持 r10；新的 probe、PM、背光和重绑逻辑等待后续加载及硬件验收。
AUR Git 和未命中缓存网页已核对；发布初期 RPC 仍返回 r16，具体观测保留在本地发布记录中。
本轮恢复材料为经校验的 r16 包、54 个模块及安装前 TCC 配置，位于统一 Backups 目录。
没有睡眠或重启。

2026-10-15 后续事务修复已合入维护源码 `85bf136`，381 项完整测试及两个内核各 27 模块全新构建通过；
`4.24.0.r30.g85bf136-1` 已完成包内双内核隔离 DKMS 验证、磁盘安装及 AUR 发布。
系统安装 hooks 在两套内核各安装 27 个模块；安装源码及 DKMS 配方与 `85bf136` 核对一致。
包完整性、模块 vermagic/签名元数据及启动文件签名检查通过；Secure Boot 关闭，未验证开启后的信任或实际启动。
五个运行模块的 build ID 均保持：IO 仍为 r23，另外四个仍为 r10，新事务修复尚未运行。
新 IO 依赖新 keyboard 导出的 `uw_write_tdp`，不能只热重载 IO 配合旧 keyboard。
本轮只更新磁盘上的配套模块，保留五个运行模块，TCC 配置字节、背光/RGB/Fn 锁、cTGP=25 及三个服务状态保持；
没有睡眠、重启或重启 TCC 服务。
新的事务修复需要以后统一加载新模块后才能开始硬件验收。
安装前 r23 包、两个内核的 54 个签名模块及 TCC 配置已独立校验保留在
`/home/lightjunction/Backups/projects/mechrevo-drivers/light/20261015-transactions-r23`。
AUR 提交 `5cde00b` 与远端一致，新网页的包版本及维护者已核对；发布初期 RPC 仍缓存 r23，
没有将它当作 r30 元数据已更新的证据。包冻结于 `85bf136`，发布后的状态说明仅更新文档；
后续从 `main` 构建的 git 包可能包含这些文档提交并得到更高的提交计数。
