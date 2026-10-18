# 本地 Arch 控制台包

`mechrevo-console` 独立安装 Rust CLI 和原生 GUI，不依赖 DKMS 包，也不替换
`mechrevo-control`、TCC 或已有热键守护进程。安装不新增服务、不修改风扇曲线，
GUI 从应用菜单中的“机械革命控制台”启动。

当前配方固定源码提交 `a1904557bbea55f39aadab03dbc31813e64c3f9b`，包版本为
`0.1.0.r158.ga190455-1`。这是本地部署配方，不表示已经发布到 AUR。
后续更新应一并修改提交、版本和部署验证记录，不能只替换安装目录里的二进制。

构建需要系统已有 Rust 1.95 以上、Git、pkgconf、desktop-file-utils，以及锁文件
对应的 Cargo 缓存。Cargo 使用 `--release --workspace --all-features --locked --offline`，
缺少依赖时直接失败，不安装依赖或修改锁文件。内嵌字体和型号目录随二进制分发；
中文字体通过 fontconfig 使用系统回退字体，可选安装 `noto-fonts-cjk`。

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
pacman -Qlp mechrevo-console-0.1.0.r158.ga190455-1-x86_64.pkg.tar.zst
sudo pacman -U ./mechrevo-console-0.1.0.r158.ga190455-1-x86_64.pkg.tar.zst
mechrevo-console --version
mechrevo-console status --json
```

安装目标为 `/usr/bin/mechrevo-console`、`/usr/bin/mechrevo-console-gui` 和
`/usr/share/applications/best.lmm.mechrevo-console.desktop`；desktop ID 与 GUI
的 Wayland app ID 一致。许可放在 `/usr/share/licenses/mechrevo-console/`，
包含项目 GPL 许可声明和两份内嵌字体的 OFL；控制台说明放在对应的 `/usr/share/doc/` 目录。

安装后应核对 `pacman -Qkk mechrevo-console`、包内与安装后二进制的摘要、CLI
只读状态及原生 GUI 启动结果。安装包成功不代表 GUI 或硬件控制已验收；运行中的
内核模块也不会因安装控制台而更新。回滚使用保留的上一个本地包执行 `pacman -U`；
首次安装可用 `pacman -R mechrevo-console` 卸载，用户 GUI 偏好不会由该包删除。
