<div align="center">

# Sluice（水闸）

**为 AI 编码代理打造的 IDEA 级 Git 工作台。**

AI 负责写代码，你在闸口审查、逐行暂存、放行。

[![CI](https://github.com/kverona-ai/sluice/actions/workflows/ci.yml/badge.svg)](https://github.com/kverona-ai/sluice/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/License-Apache--2.0-3b5bdb.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.97-dea584?logo=rust&logoColor=white)](rust-toolchain.toml)
[![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows-2f9e44)](#安装)
[![Status](https://img.shields.io/badge/status-M3%20%2F%20M4%20in%20progress-e8590c)](#路线图)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-d6006c.svg)](#参与贡献)

[English](./README.md) · **简体中文**

</div>

---

## Sluice 是什么？

终端 AI 编码代理——Claude Code、Codex CLI、Grok Build、DeepSeek Harness、Gemini CLI、Kimi Code 等——正在以人类跟不上的速度产出仓库里的大部分提交，而 `git log` 和 `git add -p` 早已不堪审查之用。

**Sluice 是这些代理与你的提交历史之间的那道水闸。** 它是一个独立的、GPU 渲染的 Git 图形工作台，把 JetBrains IDEA 级别的 Git 审查体验按「代理时代」重造：

- **一眼看全** —— 虚拟化提交图、稳定的泳道配色、实时过滤，每条提交都带 *agent 徽章*：这是人写的，还是 Claude / Codex / Grok / 其他代理写的。
- **精确放行** —— 双栏 diff 带词级高亮；按文件、按块、甚至按单行勾选暂存（可视化的 `git add -p`）。
- **始终可控** —— 所有写操作都经由你自己的 `git` 执行，hooks、提交签名、凭据助手与终端行为完全一致；Sluice 执行的每条命令都回显在 Console 页。
- **自带 AI** —— 提交信息由你已在用的 AI CLI 起草，用你自己的账号。零 API key、零配置。
- **不被打断、不被惊吓** —— 文件监听让任何代理/终端动作约 200ms 内反映到界面；代理只能「提议」，放行永远由人点下（提议-确认队列开发中）。

> **当前状态：早期预览。** 历史浏览、过滤、diff、暂存（文件/块/行）、提交、拉取/推送、AI 提交信息起草今天即可用。分支操作、merge/rebase 界面、MCP 桥接与移动端伴侣正在开发中——见[路线图](#路线图)。

## 功能特性

| | |
|---|---|
| **提交图** | 万级提交流畅滚动、泳道颜色稳定、`origin & main` 合并徽章、HEAD 高亮、时间序 / 拓扑序 |
| **过滤** | 文本 / 正则 / 大小写搜索、作者选择、日期预设、「仅看 AI 提交」——全部实时 |
| **Diff 查看器** | 双栏与统一视图、词级高亮、忽略空白、上下文 0/3/8/20 行、差异导航（`F7`/`⇧F7`） |
| **行级暂存** | 每个块、每一行都有复选框；补丁经 `git apply --cached` 应用并回显到 Console |
| **提交面板** | Amend / Sign-off / Author 覆盖 / 跳过 hooks、Commit & Push |
| **AI 提交信息** | 由本机已装 CLI 起草——Claude Code、Codex CLI、Grok Build、DeepSeek Harness、Gemini CLI、Kimi Code、Z Code、Qwen Code、opencode、GitHub Copilot CLI——自动探测、零密钥 |
| **一键接入 AI 工具** | `⌘⇧I` 或 `sluice ai connect`：把 Sluice 的只读 MCP server 注册进每个已装 AI CLI（优先用工具自己的 `mcp add`，没有命令的工具则带备份地写配置文件） |
| **文件历史与 Blame** | 逐文件 `git log --follow` 可跳转到提交；blame 按提交分带的槽、悬停高亮整个提交（`⌘⇧H` / `⌥⌘B`，或右键任意文件） |
| **分支 / Stash / 时光机** | 分支面板 checkout · merge · rebase · 删除 · 新建；stash push/apply/pop/drop；丢弃 / reset --hard 前自动安全快照 |
| **代理溯源** | 每条提交归因到产生它的代理（当前基于 trailer 分析，确定性会话溯源在路线图上） |
| **实时刷新** | 工作区 + `.git` 监听；代理的任何动作约 200ms 内可见 |
| **Console** | 每条 git 命令的耗时、退出码、stderr；详细模式连读路径也可见 |
| **键盘优先** | IDEA 风格键位：`⌘9` 日志 · `⌘0` 变更 · `⌘B` 分支 · `⌘5` stash · `⌘K` 提交面板 · `⌘↩` 提交 · `Space` 暂存；完整列表见设置（`⌘,`） |

## 技术栈

| 层 | 技术 | 为什么 |
|---|---|---|
| 语言 | [Rust](https://www.rust-lang.org/)（`rust-toolchain.toml` 锁定版本） | 从 git 内部到像素同一门语言；核心可复用到移动端 |
| UI 框架 | [GPUI](https://www.gpui.rs/) + [gpui-component](https://github.com/longbridge/gpui-component) | GPU 渲染（Zed 同款框架）；虚拟化列表、Dock、输入组件 |
| Git 读路径 | [gitoxide (`gix`)](https://github.com/GitoxideLabs/gitoxide) | 纯 Rust 高速读取——refs、日志、状态、blob |
| Git 写路径 | 用户自己的 `git` CLI | 行为 100% 保真：hooks、GPG/SSH 签名、凭据助手 |
| Diff | [imara-diff](https://github.com/pascalkuthe/imara-diff) + 自研词级二次 diff | Histogram 行级 diff、token 级高亮、部分补丁生成 |
| 文件监听 | [notify](https://github.com/notify-rs/notify) | 跨平台工作区 + `.git` 事件 |
| 异步 | [tokio](https://tokio.rs/) + GPUI 执行器 | git 工作永不阻塞渲染帧 |
| 字体 / 图标 | Source Serif 4（OFL）· Phosphor Icons（MIT） | 全部内嵌，运行时零网络 |

## 架构与模块

```
crates/
├── core          VCS 中立领域类型 · GitReader trait · 后端能力声明
│                 diff 引擎（分块、词级、部分补丁）· Console 日志
├── backend-gix   读路径 —— refs / 日志 / 提交详情 / 状态 / blob（gitoxide）
├── backend-cli   写路径 —— 暂存、提交、推拉……全部经用户自己的 `git`
│                 登录 shell PATH 解析 · porcelain-v2 状态解析
├── graph         提交图泳道布局：泳道分配、边路由、稳定配色
├── watch         工作区 + .git 监听，事件流可去抖
├── domain        Repo 句柄 · 后台快照（日志/详情/变更/diff）· 日志过滤
│                 UI 无关的业务大脑，禁止出现 gpui 类型
├── bridge        MCP server · 代理 hooks · 会话溯源            （开发中）
├── ui            GPUI 视图：工作台外壳、日志、diff、变更、Console
└── app           `sluice` 可执行文件 —— 桌面入口 + CLI 子命令
```

两条纪律保证架构不腐化：`core`/`domain` 永不出现 UI 类型；`git2` 被 `cargo-deny` 禁止——读是纯 Rust（`gix`），写是你自己的 `git`。

## 安装

### 前置条件

- **git ≥ 2.35** 在 `PATH` 上
- **Rust** —— 装好 [rustup](https://rustup.rs/) 即可，首次构建自动安装锁定的工具链
- **macOS 12+**（Apple Silicon 或 Intel；无需 Xcode / Metal 工具链），或
  **Windows 10/11** + Visual Studio Build Tools（C++ 工作负载）+ Git for Windows

### 从源码运行

```bash
git clone https://github.com/kverona-ai/sluice.git
cd sluice
cargo run --release -p sluice -- open /path/to/any/repo
```

### 安装 / 部署

```bash
# macOS —— 打成可双击的 .app
cargo build --release -p sluice
scripts/bundle-macos.sh release          # → target/release/Sluice.app，拖进 /Applications

# Windows —— 构建可执行文件
cargo build --release -p sluice          # → target\release\sluice.exe

# 或把 `sluice` CLI 装到 PATH（两个平台通用）
cargo install --path crates/app
sluice open .                            # 打开当前仓库
sluice log .                             # 提交图文本导出（SSH 场景好用）
sluice ai status                         # 哪些 AI CLI 已安装 / 已接入
sluice ai connect                        # 把 Sluice 注册为它们的 MCP server
sluice mcp serve                         # stdio MCP server 本体（各工具实际启动的就是它）
```

打了 `v*` 标签的版本由 [Release 工作流](.github/workflows/release.yml) 在 macOS arm64 / x86_64 与 Windows x86_64 上构建并附到 GitHub Releases（含 `SHA256SUMS`）——目前是未签名预览（Gatekeeper / SmartScreen 会提示）；签名与公证随首个 public beta 提供，届时同步上 Homebrew / winget。想立即体验，可以用生成的演示仓库：

```bash
fixtures/make-sample.sh && cargo run --release -p sluice -- open target/fixtures/sample
```

### 快捷键

IDEA 风格预设（Windows/Linux 用 `Ctrl` 代替 `⌘`）：`↑/↓/PgUp/PgDn/Home/End` 移动 · `Space` 暂存/取消 ·
`⌘9` 日志 · `⌘0` 本地变更 · `⌘6` Console · `⌘F` 搜索 · `⌘K` 提交面板 · `⌘↩` 提交 ·
`⌥⌘A`/`⌥⌘U` 全部暂存/取消 · `F7`/`⇧F7` 差异导航 · `Esc` 关闭 diff · `⌥⌘Y` 刷新。

## 路线图

- ✅ **M1 —— 审查**：提交图、refs、过滤、diff、详情、实时监听
- ✅ **M2 —— 提交**：行级暂存、提交面板、AI 起草、推拉
- 🔨 **M3 —— 分支**（当前）：分支面板、merge / rebase、stash、快照、文件历史与 blame 已完成；交互式 rebase 界面与三方合并器接下来
- 🔨 **M4 —— AI 桥接**（大部分完成）：MCP server（读工具 + propose_* 工具）、一键接入含 hooks、提议-确认队列、会话溯源、askpass；接下来是向导打磨与 public beta 打包
- ⏭ **M5 —— 移动伴侣**：iOS / Android 上审查与放行，复用同一 Rust 核心
- ⏭ **M6 —— 扩展**：GitHub / GitLab PR 评审、jujutsu (jj) 后端

## 参与贡献

非常欢迎 PR——最短路径：

```bash
# 1. 在 GitHub 上 Fork，然后
git clone https://github.com/<你>/sluice.git && cd sluice
# 2. 验证环境（工具链自动安装）
cargo test --workspace
# 3. 开分支、写代码、保持检查全绿
git switch -c feat/my-change
cargo fmt --all && cargo clippy --workspace --all-targets && cargo test --workspace
# 4. 用 Conventional Commits（feat:/fix:/docs:/refactor: …）提交，向 main 发 PR
```

CI 会在 macOS **和** Windows 上跑 fmt、clippy `-D warnings`、测试与 `cargo-deny`——PR 只需要一个绿色的 CI。完整指南（项目布局、fixture 仓库、如何跑测试数据）见 [CONTRIBUTING.md](CONTRIBUTING.md)；比小修复更大的改动请先开 [issue](https://github.com/kverona-ai/sluice/issues) 讨论。

## Star 趋势

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=kverona-ai/sluice&type=Date&theme=dark" />
  <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=kverona-ai/sluice&type=Date" />
  <img alt="Star History Chart" src="https://api.star-history.com/svg?repos=kverona-ai/sluice&type=Date" />
</picture>

## 许可

[Apache-2.0](LICENSE) © kverona-ai 与贡献者。
内嵌资源：Source Serif 4（SIL OFL 1.1）· Phosphor Icons（MIT）。
