# Arch Linux / AUR 打包

`mechrevo-drivers-dkms-git` 从本仓库 `main` 构建平台驱动，AUR 维护账号为
`lightjunction`。配方位于 `packaging/aur/mechrevo-drivers-dkms-git/`，包含
`PKGBUILD` 和 `dkms.conf`。控制台独立打包，见
[控制台安装说明](../packaging/arch/mechrevo-console/README.md)。

## 安装与卸载

先核对[机型与功能范围](MECHREVO.md)，再安装对应内核的 headers。标准 Arch 内核示例：

```sh
sudo pacman -S --needed base-devel linux-headers
yay -S mechrevo-drivers-dkms-git
# 或：paru -S mechrevo-drivers-dkms-git
dkms status
```

LTS、Zen、Hardened 或自定义内核分别需要自己的 headers。配方把 headers 列为
可选依赖，是为了让用户选择内核；DKMS 编译仍需要匹配版本。

安装包把源码放到 `/usr/src/mechrevo-drivers-<版本>/`，Arch 的 DKMS hooks 为装有
headers 的内核构建模块。配方本身不执行 `sudo`、`make install` 或 `modprobe`。
磁盘安装完成后，已加载的旧模块会继续运行；通过重启统一加载新模块，再进行硬件验证。
不要单独热卸载有相互依赖的平台模块。

```sh
sudo pacman -R mechrevo-drivers-dkms-git
```

卸载由 DKMS hooks 处理模块，保留的旧安装包可用于 `pacman -U` 回滚。不要手工删除
`/usr/src` 或 `/var/lib/dkms` 内的模块记录。

本包与另一维护者的 `mechrevo-drivers-dkms`、原版 TUXEDO 驱动、旧
`tuxedo-keyboard` DKMS 包互斥，因为它们安装相同的模块或规则。配方使用 `conflicts`
和 `provides` 描述关系，不使用 `replaces` 接管其他维护者的软件包。

## 版本

版本格式为 `4.24.0.rN.g<commit>`。`pkgver()` 选取当前历史中最新的正式 `vX.Y.Z`
标签，`N` 为标签后的提交数；RC、beta 和其他分支的标签不作为基线。没有正式标签时，
回退为 `r<总提交数>.g<commit>`。

Git 包在每次构建时读取最新源码，因此实际构建版本可能高于 AUR 网页上的版本。
源码仓库保留配方模板；AUR checkout 的 `PKGBUILD` 版本和 `.SRCINFO` 在发布时生成。
`.SRCINFO` 只在 AUR 仓库维护。`dkms.conf` 改动后必须同步其 SHA-256，只有 Git 源使用 `SKIP`。

## 维护与发布

1. 在开发机运行受影响测试、完整本地回归和模块编译，检查 ShellCheck、打包文件及模块清单。
2. 分批提交并推送已验证改动到 GitHub，核对 `origin/main` 与验收提交。
3. 在独立 AUR checkout 中复制 `PKGBUILD`、`dkms.conf`。
4. 运行 `makepkg --cleanbuild`，检查版本、依赖、包内容和权限，并用 `namcap` 检查。
5. 从包内源码进行隔离 DKMS 构建，记录目标内核、模块数量和结果。
6. 运行 `makepkg --printsrcinfo > .SRCINFO`，审核后提交并推送
   `ssh://aur@aur.archlinux.org/mechrevo-drivers-dkms-git.git`。
7. 核对 AUR 远端提交及网页或 RPC 的版本、维护者；缓存尚未更新时保留实际观测值。

AUR 仓库只提交 `PKGBUILD`、`dkms.conf`、`.SRCINFO`。构建源码镜像、`src/`、`pkg/`、
日志和安装包留在本机构建目录。日常验收使用本地测试，不触发远端 CI 或自动审查。

`namcap` 对这个源码包可能提示架构为 x86_64、未静态识别 DKMS 依赖。驱动面向 x86
笔记本，DKMS 由发行版 hooks 调用；这两个提示不作为删除架构限制或 DKMS 依赖的理由。

## 验收记录

每轮记录分别列出源码提交、构建安装包、磁盘安装版本、运行模块和实机操作结果。
软件包安装成功后，继续核对模块签名、vermagic、启动镜像和实际运行版本。
硬件验收只覆盖记录中的型号、固件和操作。

历史及本轮证据集中保存在 [verification](verification/) 和
[GM6PX0X 实机记录](hardware/GM6PX0X.md)，本说明不重复维护按日期累加的版本清单。
