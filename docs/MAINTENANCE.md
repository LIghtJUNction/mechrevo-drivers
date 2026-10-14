# 上游同步与维护

## 分支布局

- `main`：机械革命维护分支，初始基于正式版本 `v4.24.0`，包含品牌补丁、文档及同步工具。
- `upstream`：GitLab 开发分支 `main` 的原始历史镜像，不叠加补丁；可能含尚未正式发布的代码。
- `automation/sync-upstream`：机器人拥有的临时更新 PR 分支。每次从最新维护 `main` 重新合并上游正式版本；不要在此分支手工提交。
- 上游标签保留原名；同步时从 GitLab 取回的标签在本地使用 `upstream/v*` 命名空间，避免分支自己的标签影响版本选择。选中的正式标签会推送到 GitHub 原名 `vX.Y.Z`；它指向上游提交，不是机械革命补丁版本。

跨 GitLab → GitHub 导入不能在 GitHub 显示成 GitLab 仓库的原生 fork 关系，但完整 Git 历史与上游来源保留。

## 工作流

`.github/workflows/sync-upstream.yml` 每日 **03:17 UTC / 北京时间 11:17** 运行，亦可从 Actions 手动运行。
GitHub 定时调度可能延迟；不活跃的公共仓库连续 60 天无活动时 GitHub 可能停用定时工作流，届时需手动重新启用。
没有 push/PR 自动测试、CodeQL、自动远程审查，也没有制品发布或部署任务。

同步脚本 `scripts/sync_upstream.py` 的过程：

1. 要求干净工作区，从固定 GitLab URL 获取开发 `main` 和标签，从 GitHub 获取当前分支。
2. 按数值版本号选择严格符合 `vX.Y.Z` 的最新正式标签，排除 `_rc`、beta 等；要求该标签属于上游 `main` 历史。
3. 检查镜像可快进、维护分支仍保留 MECHREVO/TUXEDO 三个独立 DMI 条目。被改写的上游标签会导致 fetch 失败。
4. 使用普通、原子的 push 更新 `upstream` 与选中正式标签；不强推镜像、标签或 `main`。
5. 正式版本已经被维护 `main` 包含时结束，不创建空 PR；这不会阻止开发镜像继续更新。
6. 有新正式版本时，在临时 Git worktree 中从最新 `origin/main` 创建合并提交，检查机械革命条目没有丢失。
7. 确认维护分支没有并发更新，再通过精确 `force-with-lease` 更新机器人 PR 分支。这是唯一允许重写的分支；远端已变更时拒绝覆盖。
8. 创建或更新同一条 PR；相同树及父提交不产生重复提交，标题/正文未变时不重复编辑。

干净合并和源码检查不等于内核构建及硬件验证，因此 **不会自动合并 PR**。
冲突、接口变更、上游历史改写或权限故障会停止更新，保持维护 `main` 不变，并尝试创建一条去重的提醒 issue。
如果已存在提醒 issue，不重复刷评论；每次失败的具体日志在对应 Actions run 中。
镜像同步可能先成功，而后续合并/PR 操作失败；这种情况不代表维护分支已更新。

## GitHub 权限

只使用内置、仓库范围的 `GITHUB_TOKEN`，不需要长期 PAT 或 GitLab secrets。
checkout action 固定到具体 commit；调度和手动运行都只执行维护 `main` 中的工具，
不通过 `pull_request_target` 执行外来 PR 代码。
job 需要 `contents: write`、`pull-requests: write`、`issues: write`。

仓库 **Settings → Actions → General → Workflow permissions** 必须允许
**Allow GitHub Actions to create and approve pull requests**；虽然设置名称包含 approve，本脚本只创建 PR，不提交审核或批准。
如果组织策略禁止该设置，镜像可能成功但 PR 会失败，应调整策略而不是把 PAT 写进脚本。
不要求开通自动合并。若以后保护 `upstream` 或机器人分支，要允许同步 bot 正常更新对应 refs。

## 本机验收

在仓库目录：

```sh
python3 -m unittest discover -s tests -v
# 不向远端推送、不调用 GitHub PR API；会 fetch 并在本地临时 worktree 验证合并。
python3 scripts/sync_upstream.py --dry-run
# 独立验证内核编译（不安装、不加载模块）。
make -j2 KDIR=/usr/lib/modules/$(uname -r)/build
```

测试在 `.local-tests/` 下创建真实本地 Git 仓库，GitHub API 被 mock，测试不联网。
覆盖版本筛选、品牌守卫、无更新、稳定版与开发版分离、幂等更新、保留维护提交、dry-run、
合并冲突、补丁丢失、上游镜像/标签改写、游离标签、并发维护提交、精确分支租约和脏工作区。

初始化时已在本机通过 19 项测试、actionlint 1.7.12 和 ShellCheck 检查，
并成功构建全部 27 个模块（Arch `7.2.9-arch1-1` headers）。没有安装/加载模块，没有做实机验证。
本机 `/tmp` 当时 inode 耗尽，构建改用仓库内 `.cache/tmp`；这是本机资源问题，不是驱动编译错误。
该记录只描述初始化版本，不代表未来 PR 已被验证。
`.sync-worktrees/` 和 `.local-tests/` 不提交到 Git。

AUR 构建配方及独立包发布流程见 [Arch / AUR 安装与打包](AUR.md)。
`tests/test_aur_packaging.py` 会在本机检查包名、互斥、源码地址、校验和、Git 版本选择、
DKMS 模块与 Kbuild 一致性，以及安装布局和权限；不联网、不安装、不加载模块。

驱动回归还会提取真实 C 函数和 ioctl 定义，在模拟 EC/ACPI 下编译运行。
`test_uniwill_runtime.py` 覆盖用户缓冲区和参数边界；
`test_uniwill_fan_runtime.py` 覆盖风扇初始化的逐项失败、回读不匹配、缓存、后台任务、
自动模式和睡眠恢复的组合错误路径。WMI 用例另启用 ASan/UBSan。
模拟失败不能写成实机 EC 故障注入，也不能替代真实睡眠、散热或长期负载验证。

旧式风扇控制的首次请求会排入后台任务；返回成功表示接受请求，不能当作硬件已完成写入。
同步写入失败不得更新成功缓存；后台失败必须可见，并保留下一次请求的恢复能力。
检查睡眠路径时，要同时验证两只风扇的保存值、挂起失败后没有恢复回调的情况，
以及后续显式手动/自动命令会不会重放过时状态。

例行更新先在本机检查 PR diff、跑以上测试和对应内核构建，记录结果后再合并、推送。
不要为日常验收反复触发/等待远程 CI 或远程审查。没有硬件时明确注明未做实机验证。

## 处理冲突

不要用 reset 或强推 `main` 来跟随上游，也不要直接关闭所有兼容性检查。
从维护 `main` 开新分支，手工合并对应上游正式标签，逐项处理冲突和适配、在本地验收后提交 PR。
若需要精确到型号的限制，应在独立补丁中记录理由、DMI 依据和测试范围。
故障修复后关闭提醒 issue，正常定时同步会继续运行。
