# Arch Linux / AUR 打包

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

后续已在旷世 16 Pro `GM6PX0X` 安装并加载 `4.24.0.r3.g6598274-1`，
完成背光/Fn 锁接口切换恢复、传感器读取和九项不支持 TDP 查询的验证。
本地测试已增加到 34 项，两个内核各编译并安装全部 27 个模块。
功能边界、现存 NVIDIA 日志及未测项目见[本机验证记录](hardware/GM6PX0X.md)。
Git 包构建时会读取 `main` 的最新提交；AUR 网页版本与实际构建出的 `pkgver()` 可能不同。
