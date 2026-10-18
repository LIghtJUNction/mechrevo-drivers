# Windows x64 构建与安装核验

这里提供 Windows MSI 的源码构建配方。`build.ps1` 只构建、检查并记录产物，不安装 MSI，也不启动控制助手或写入硬件。

目前尚未完成 Windows 上的 PowerShell 解析、MSVC 构建、WiX 5.0.2 打包及 MSI 安装、升级、卸载和修复验收。维护守卫的 C 源码仅在 Linux 上用 MinGW 完成编译与链接检查；这不等于 MSVC、WiX 或 Windows 运行通过。

## 构建环境

在原生 Windows x64 上准备以下工具。脚本不会下载或安装依赖。

- 原生 x64 PowerShell 7.4 或更新版本，即 `pwsh.exe`，不是 Windows PowerShell 5.1。
- Git、Rust MSVC 工具链及已安装的 `x86_64-pc-windows-msvc` target。
- Visual Studio Build Tools 的 x64 C/C++ 开发环境和 Windows SDK。需要可用的 `cl.exe`、头文件、导入库和 `mt.exe`；签名时还需要 `signtool.exe`。
- 固定版本 [WiX 5.0.2](https://github.com/wixtoolset/wix/releases/tag/v5.0.2)，`wix.exe --version` 必须匹配该版本。
- .NET 8 或更新运行时。WiX 5.0.2 的工具目标为 .NET 6，并设置了 Major roll-forward；配方会检查 .NET 8+ 运行时是否存在。
- `console/Cargo.lock` 锁定的、适用于 Windows MSVC 构建的 Cargo 依赖缓存。实际构建使用 `--locked --offline`，缺少缓存会失败，不会联网补齐。

先载入 Visual Studio 的 x64 开发环境，再在继承该环境的 x64 `pwsh` 中运行脚本。仅把 `cl.exe` 复制到 `PATH` 不够。依赖准备和缓存下载应提前完成。

源码必须已提交，且 `git status --porcelain --untracked-files=normal` 为空；Windows 资源和编译内嵌的支持目录也必须已被 Git 跟踪。构建结束会再次检查提交和工作区。不要在构建期间修改源码。

`OutputDirectory` 必须尚不存在。建议使用仓库外的新绝对路径，避免产物使源码工作区变脏。保留失败构建的日志，后续重试换一个新目录。调用环境不能设置 `RUSTFLAGS` 或 `CARGO_ENCODED_RUSTFLAGS`；配方自行选择静态 MSVC CRT。

## 构建开发候选包

在仓库根目录运行，例如：

```powershell
pwsh -NoProfile -File .\packaging\windows\build.ps1 `
  -MsiVersion 0.1.172 `
  -ReleaseId console-0.1.172-candidate `
  -OutputDirectory C:\Build\mechrevo-console-0.1.172-candidate `
  -Candidate
```

示例版本仅演示参数，不表示该版本已经发布。

| 参数 | 实际含义 |
| --- | --- |
| `-MsiVersion` | 必填的三段整数版本，无前导零，范围不超过 `255.255.65535`。重大升级需要提高这个版本；只修改 Release ID 不构成升级。 |
| `-ReleaseId` | 必填，1–128 个 ASCII 字母、数字、点、下划线或连字符，写入二进制构建身份和发布清单。 |
| `-OutputDirectory` | 必填的新输出目录，不覆盖已有目录。 |
| `-Candidate` | 可选，给 CLI 和 GUI 启用 `windows-fnlock-candidate` 编译特性。省略时不允许仅凭 candidate 资格进入真实控制路径。 |
| `-Trust` | 默认 `development_candidate`；需要签名时显式指定 `authenticode`。 |

`-Candidate` 不修改机型支持目录，不提供运行时路径或硬件白名单覆盖，也不证明 Fn 控制已在 Windows 实机上验收。默认 `development_candidate` 产物未签名，不能作为已签名的正式发布包对外说明。

签名构建必须另外传入：

- `-Trust authenticode`。
- `-SigningCertificateThumbprint`：当前用户 `My` 证书库中可用于签名的证书指纹，必须为 **40 位十六进制 SHA-1 证书指纹**。它是证书选择器，文件摘要仍使用 SHA-256。
- `-TimestampUrl`：明确的 HTTPS RFC 3161 时间戳服务地址，不能包含用户信息或 URL fragment。

只有选择 `authenticode` 时才接受签名参数。配方先嵌入并检查三个 EXE 的权限 manifest，再签名最终 EXE、维护守卫 DLL 和 MSI；发布清单记录签名后 EXE 的 SHA-256。签名有效只证明相应签名检查通过，不代替安装、权限提示或硬件功能验收。

## 产物与回执

输出目录包含：

- `mechrevo-console-<MsiVersion>-x64.msi`：MSI 安装包。
- `payload/`：三个最终 EXE、`console-release.json` 和许可证。
- `build-receipt.json`：构建身份、各步骤退出码、信任模式，以及成功时的 MSI 和发布清单摘要。
- `logs/`：各构建、manifest、签名和打包步骤的输出。
- `intermediate/`、`cargo-target/`、`tmp/`：本次构建的中间产物、Cargo 输出和临时目录。

保留交付包、回执和排错所需日志；中间目录也位于本次独立输出目录内，不会自动清理。`build_complete: true` 只表示脚本中的构建步骤完成。回执仍明确记录 `installed: false`、`windows_runtime_verified: false` 和 `hardware_setter_executed: false`。

## 安装后只读核验

MSI 采用每台计算机安装，目标固定为系统 **ProgramFilesX64 Known Folder 下的 `Mechrevo\Console`**，通常是 `C:\Program Files\Mechrevo\Console`。没有便携安装或自定义安装根目录选项；核验脚本也不接受安装路径覆盖。MSI 安装、升级、卸载、修复及实际 UAC/ACL 行为需要在 Windows 验收环境中单独验证。

完成实际安装后，用原构建回执提供期望身份，在一个全新的进程中运行核验，不要把脚本点加载到已经使用过的 PowerShell 会话：

```powershell
$receipt = Get-Content -LiteralPath `
  C:\Build\mechrevo-console-0.1.172-candidate\build-receipt.json -Raw |
  ConvertFrom-Json

pwsh -NoProfile -File .\packaging\windows\verify-installed.ps1 `
  -ExpectedReleaseId $receipt.release_id `
  -ExpectedSourceCommit $receipt.source_commit `
  -ExpectedMsiVersion $receipt.msi_version
```

三个 `Expected*` 参数都可选，但验收指定构建时应全部提供。`ExpectedSourceCommit` 必须为 40 位小写十六进制提交号，应来自构建回执，而不是安装后任意工作区的当前 HEAD。`ExpectedMsiVersion` 同样要求规范的三段 MSI 版本。

核验脚本输出一行 JSON，成功退出码为 `0`，失败为 `1`。它只读检查固定路径、所有者与权限、发布清单、三个 EXE 的摘要和 AMD64/权限 manifest，以及唯一关联的机器级 MSI 注册记录。`authenticode` 模式还要求三个 EXE 有效的内嵌签名且证书一致；这仍不是独立的发布者证书固定验证。它不验证 MSI 组件归属或缓存 MSI，不枚举每个 manifest 语言变体，不启动应用，不修改驱动、服务或硬件。`status: verified` 不能写成 Fn、风扇或其他 Windows 实机功能已通过。

## 维护被阻止时

安装维护使用与 Fn 写入相同的互斥锁，并在系统 ProgramData 下留下受保护的持久维护标记。标记在事务之间阻止新的 EC 写入；进程退出或经过一段时间不会使它自动失效。

崩溃、部分写入或未知标记会保持拒绝写入。管理员需要先检查安装日志和标记归属。只有同一 MSI `ProductCode` 的 Repair 才能接管合法残留标记；不同 ProductCode 的残留不会由新版安装自动清除。重大升级中的旧版卸载只借用外层维护标记，不负责清除它。

不要手工删除维护标记、强杀正在写入的助手、重置 Fn 状态或关闭回滚来绕过阻塞。若合法 Repair 也被拒绝，应保留现场和日志，检查原因，不能把绕过保护当成修复。
