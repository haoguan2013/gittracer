# Git Tracer · 开发与排障笔记

> 这是面向 GitHub 重写 `README.md` 之前的**原文归档**，内容偏内部：本机构建环境、NSIS / `%TEMP%` 排查结论、CDP 调试方式、完整命令接口表和逐版本更新记录。
> 只想了解怎么用、怎么装，请看 [README.md](../README.md)。

---

一个用 **Tauri 2 + Rust** 写的 Windows 桌面小工具：选一个目录，递归找出里面所有 Git 仓库，
把它们**待提交的改动**（已暂存 / 未暂存 / 未跟踪 / 冲突）汇总到一张表里，点一下就能看 diff，
勾几个文件就能提交、之后一键推送。

默认只读 —— 扫描、看状态、看 diff 全程不碰仓库（连 `index.lock` 都不会留，所有查询命令都带
`--no-optional-locks`）。只有你点「提交选中」或「推送」时才会真正写仓库。

## 界面

![Git Tracer 界面](docs/screenshot.png)

三栏联动：

```
┌──────────────────────────────────────────────────────────────┐
│ 目录输入  [浏览] [扫描]   深度▾  只看有变更↩  [刷新]           │
│ 📁 dir-a 6 ×   📁 dir-b 3 ×                                    │
│ 12 个仓库 · 7 个有变更 · 34 处变更 (待推送 2 个仓库)           │
├────────────┬───────────────┬─────────────────────────────────┤
│ 仓库列表    │ 文件列表       │ [差异][分支树]                  │
│ (分支/计数) │ (勾选 + 分组)  │ (行号 + 增删着色 / 提交图)      │
├────────────┴───────────────┴─────────────────────────────────┤
│ 提交 已选 2/7·main [提交信息___][⤢] [全选][取消全选][提交选中] │
│                                          [拉取↓1] [推送↑1]    │
└──────────────────────────────────────────────────────────────┘
```

没有待提交项的仓库也默认列出来（灰一点、标「干净」），勾上「只看有变更」才会隐藏。

界面走的是仪表盘/终端那一挂：深蓝黑底、细青色线、全大写小标签、蓝色选中条、琥珀色警示，信息密度优先。

分支树：

![分支树：车道图 + ref 徽标 + 提交列表](docs/graph.png)

## 功能

- **递归扫描**：从任意根目录向下找 Git 仓库（下拉里可选 1~12 层，默认 4），自动跳过 `node_modules`、`target`、
  `dist`、`.venv`、`vendor` 等噪音目录，以及 junction / 符号链接，避免爬进盘符级目录。
- **同时绑多个目录**：顶部输入框回车就能往里加一个根目录，也可以点「浏览…」一个个加；
  已绑定的根目录做成一行标签（带该目录下的仓库数），点标签上的 `×` 就移除。多个根目录的仓库合并到一个列表里
  （同一个仓库被两个根目录覆盖时只出现一次），行上用小标签标明它属于哪个根，状态栏写「N 个根目录 · 扫描 M 个目录」。
- **分支内嵌套也能找到**：发现仓库后仍在其中继续下钻，所以子模块、monorepo 里的多仓库不会漏。
- **暂存区感知**：一个文件如果既暂存又未暂存，会分别出现在「已暂存」「未暂存」两组里。
- **冲突优先**：`git status` 报冲突的文件排在最前，标 `U`。
- **Diff 预览**：行号、`+/-` 着色、hunk 头；未跟踪文件直接读内容全绿显示；二进制 / 超大文件给出提示。
- **合并冲突**：冲突文件用 `git diff` 的 combined diff（`@@@ … @@@` + 两列前缀）原样渲染，
  `<<<<<<< ===  >>>>>>>` 标记保留在正文里。
- **中文路径**：所有 git 命令带 `core.quotepath=false`，`docs/接口说明.md` 原样显示。
- **增量加载**：扫描完先出仓库列表，status 由 6 个并发 worker 逐个回填，状态栏显示
  「正在读取 git status… 已读/总数」，仓库再多也不会卡住界面。
- **多选提交**：文件列表每行左边有勾选框（点方框、或按空格勾当前行），列表顶上还有一颗三态的「全选」开关
  （全选 / 半选 / 未选），旁边是「取消全选」；底部提交栏也有一组「全选」「取消全选」，两处效果一样。
  勾几个就只提交这几个文件。确认框会列出提交信息和文件清单；提交走 `git commit <信息> -- <勾选的路径>`，
  **别处已经暂存的内容不会被带进来，没勾的文件也不会被动**。
- **提交信息随便长**：底部那条输入框固定高度（不占屏幕、不能拖大），要写多行就点右边的 `⤢` 开弹窗，
  里面是大文本框（`Ctrl+Shift+E` 也能开）；弹窗里 `Ctrl+Enter` 用这条信息直接提交。
- **一键拉取**：底部「拉取」按钮拉当前分支；有上游直接 `git pull --no-edit`，没上游显式 `git pull --no-edit <remote> <分支>`。
  `behind` 不为 0 时按钮上显示 `↓N`（仓库行也会标 `↓N`）。没配远程、detached HEAD、还没任何提交时按钮禁用。
  拉完弹一条结果：「已快进 / 已是最新 / 已合并远程提交 / 已变基到远程」，失败按 git 的原话给出原因
  （凭据 / 有冲突 / 先提交或 stash / 先设上游）。
- **一键推送**：底部「推送」按钮推当前分支；有上游直接 `git push`，没上游自动 `git push -u <remote> <分支>`
  （按钮上会显示 `↑N` 表示有几个待推提交）。没配远程的仓库按钮直接禁用，不会让你白点一次。
- **待推送面板**：统计条上会出现「待推送 N 个仓库」，点开列出**分别是哪几个仓库**要推、各差几个提交、
  推到哪个上游，可以逐个推，也可以一次全推（失败的那个留在原地显示 git 的原话，其余照常继续）。

![待推送面板：哪些仓库还有提交没推到远程](docs/push-panel.png)

- **分支树**：右侧面板顶部有「差异 / 分支树」两个标签。分支树用 `git log --all --date-order` 取最近 300 个提交，
  自己算车道（lane）布局，用 SVG 画车道线、分叉 / 合并曲线和节点；行上显示短哈希、ref 徽标
  （`HEAD`→绿、本地分支→青、`origin/…`→紫、tag→琥珀）、提交标题、作者与日期。
  点任意一行会在「差异」标签里显示这个提交的补丁（`git show --diff-merges=first-parent`）。
- **分支树模糊搜索**：分支树顶上有个搜索框（`Ctrl+F` 或直接点它），边打边筛。模糊匹配是「子序列 + 打分」，
  连续命中、词首命中、开头命中加分，中间跳过太多扣分，所以 `fgsr` 能命中 `feature/graph-search`，
  `fix api` 能同时限定标题里的两个词。搜的范围包括提交标题、作者、分支 / 标签名、哈希和日期，
  命中的字用琥珀色高亮；搜到之后平铺成结果列表（车道线在子集里没有意义），按得分从高到低排，
  点一行照样打开它的差异，回车直接打开第一个结果，`Esc` 清空；清空后自动回到带车道线的分支树。

![分支树模糊搜索：搜「fix」的命中结果与高亮](docs/search.png)
- **回滚到任意提交**：打开某个提交的差异后，顶部有「回滚至此处」。点开会先算出后果——当前分支从哪个提交退到
  哪个提交、撤销几个提交、有几处未提交改动——再让你选方式：**混合**（撤销提交，改动留在工作区，默认）、
  **软**（改动留在暂存区，可以直接重新提交）、**硬**（撤销提交并丢弃已跟踪文件的未提交改动，需要额外勾选确认）。
  回滚完自动刷新仓库状态并跳回分支树。撤销掉的提交只留在 reflog 里，本程序不提供撤销；未跟踪的新文件不会被删除。

![回滚弹窗：先说清后果，再选软 / 混合 / 硬](docs/reset.png)
- **记忆**：上次的根目录列表、扫描深度、过滤开关、当前标签页都存在 localStorage。
- **默认全量显示**：没有待提交项的仓库也列在左边（标「干净」），只有主动勾「只看有变更」才隐藏；
  有本地未推送提交的仓库，行上会多一个 `↑N` 徽标，落后远程会多一个 `↓N` 徽标。

## 使用

1. 启动后在顶部输入框里填目录按 Enter（或点「浏览…」），可以连续加多个根目录，标签行上点 `×` 移除。
2. 左侧点仓库 → 中间看文件 → 点文件看 diff。
3. 要提交：在文件列表左边勾上文件（也可以用底部的「全选」/「取消全选」），底部写提交信息，
   点「提交选中」，确认框里核对一遍信息和文件清单 → 确定。要写多行信息就点输入框右边的 `⤢`。
4. 要推送：底部「推送」推当前仓库；想看**哪些仓库还没推**，点统计条上的「待推送 N 个仓库」。
5. 要拉取：底部「拉取」把远程的提交拉下来，落后几个会在按钮上显示 `↓N`。

| 操作 | 说明 |
| --- | --- |
| `Enter`（在目录框） | 把输入的目录加进列表并扫描 |
| `F5` / `Ctrl+R` | 重新扫描 |
| `↑` / `↓` | 焦点在列表上时切换仓库 / 文件 |
| `空格` | 焦点在文件列表时，勾选 / 取消当前行 |
| `Ctrl+Enter`（在提交信息框） | 提交选中的文件 |
| `Ctrl+Shift+E` | 打开弹窗编辑提交信息（大文本框） |
| `Ctrl+F`（分支树） | 聚焦提交搜索框 |
| `Enter`（在搜索框） | 打开第一个匹配的提交 |
| `Esc`（在搜索框） | 清空搜索，回到带车道线的分支树 |
| `/` | 聚焦仓库过滤框 |
| 文件名右侧 `+12 -3` | 该文件的增删行数（取自 `git diff --numstat`） |

## 开发 / 构建

需要：Node 18+、pnpm、Rust（MSVC toolchain）、WebView2（Win10/11 自带）、Git 在 PATH。

```powershell
pnpm install                 # 只装 @tauri-apps/cli
cargo build                  # 后端 debug 构建（前端资源会编进 exe）
pnpm tauri build             # 出 NSIS 安装包 + release exe
```

> **在这台机器上 `pnpm tauri build` 会报 `error: unrecognized subcommand '<...>\DSH Desktop.exe'`。**
> 因为 DSH 的 `node` 是个 shim（`.desktop-bin\node.cmd` 转调 `DSH Desktop.exe`），
> `@tauri-apps/cli` 的 `tauri.js` 会把 `process.argv[0]` 当子命令塞给 clap。用真 node 绕过：
>
> ```powershell
> & 'C:\nvm4w\nodejs\node.exe' node_modules\@tauri-apps\cli\tauri.js build
> ```
>
> 另外本机 crates.io 拉不动，构建前需要 `$env:CARGO_NET_OFFLINE='true'`（依赖已缓存），
> 并把 `$env:TMP` / `$env:TEMP` 指到工作区内（否则 `vswhom-sys` 的 `cl.exe` 会因写 `%TEMP%` 被拒而失败）。

产物：

- `src-tauri/target/release/git-tracer.exe` —— 免安装，复制走就能跑
- `GitTracer-0.3.0-portable.zip` —— 发布包（`tools\make-package.ps1` 生成），里面是：
  `GitTracer.exe`、`install.ps1`、`安装.cmd`、`卸载.cmd`、`使用说明.txt`
- `src-tauri/target/release/bundle/nsis/GitTracer_0.3.0_x64-setup.exe` —— `tauri build` 自带的 NSIS 安装包（可选，见下）

打发布包：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File tools\make-package.ps1
```

### 为什么发布包不用 NSIS 安装包

`tauri build` 生成的 NSIS 安装包在 `%TEMP%` 不可写时会弹
**「Error writing temporary file. Make sure your temp folder is valid.」**。
排查结论：这是 **NSIS 的固有行为**——安装器启动阶段就要往 `%TEMP%\nsuXXXX.tmp` 里解压自己的数据。
实测四种配置（MUI2 / 经典页面 × solid lzma / zlib 压缩、有无 `nsis_tauri_utils` 插件）
在 TEMP 被拒时**全部**失败，所以换脚本、去插件都救不了它。

因此发布包改成两条都不碰临时目录的路子：

| 方式 | 做法 | 结果 |
| --- | --- | --- |
| 便携 | 解压后直接双击 `GitTracer.exe` | 不写系统任何位置（窗口数据除外） |
| 安装 | 双击 `安装.cmd`（内部 `install.ps1`） | 复制到 `%LOCALAPPDATA%\Programs\GitTracer`，建开始菜单/桌面快捷方式，写 `HKCU` 卸载项；卸载走 `卸载.cmd` 或「设置 → 应用」 |

**更新安装是全量覆盖**：`install.ps1` 会先 `Stop-Process GitTracer` 停掉正在跑的旧版本，
把安装目录整个删掉再重建（失败会重试 6 次；仍失败就报「文件被占用，请先关掉 Git Tracer 再装」），
然后才复制新文件、重建快捷方式。所以旧版本遗留的文件（改名/删除过的资源、旧 `uninstall.ps1`）
不会留在目录里，升级后的行为跟全新安装一致。

`install.ps1` 只用文件复制 + `WScript.Shell` + 注册表，实测在 **TEMP 完全不可写**（ACL 拒绝写）
的环境下，用 Windows PowerShell 5.1 装 / 卸都正常，装完程序也能照常启动。

如果确实需要 exe 安装包，两个选择：在**正常桌面环境**（资源管理器里双击）跑 NSIS 包，或者
先设好可写的 TEMP 再启动：

```powershell
$env:TEMP = $env:TMP = 'D:\tmp'; .\GitTracer_0.3.0_x64-setup.exe
```

自测数据（会重建 `D:\Projects\_git-tracer-demo`，含干净仓库、暂存/未暂存/未跟踪混合、
真实合并冲突、重命名 + 删除 + 二进制改动、子目录仓库、以及一个位于 `node_modules` 里应被跳过的仓库）：

```powershell
pwsh -File tools\make-demo-data.ps1
```

图标是脚本生成的（无 Pillow 依赖）：

```powershell
python tools\make-icons.py
```

### 环境变量

| 变量 | 作用 |
| --- | --- |
| `GIT_TRACER_DATA_DIR` | 指定 WebView2 的用户数据目录（cookie / 缓存）。不给就按序试：`%LOCALAPPDATA%\com.git-tracer.app\webview2` → `%TEMP%\git-tracer-webview2` → exe 同级 `.git-tracer-webview2` → 当前目录 `.git-tracer-webview2`，取第一个能建出来的。受限环境（只读 AppData、沙箱、便携版）里最后两档兜底很关键，否则 WebView2 会在 `CreateCoreWebView2Environment` 处直接 `os error 5` 起不来窗口。 |
| `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` | 透传给 WebView2 的参数，调试时可用 `--remote-debugging-port=9223` 配合 `tools\cdp.mjs` 驱动界面。 |

用 CDP 自动截图 / 读界面状态：

```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS='--remote-debugging-port=9223'
Start-Process src-tauri\target\debug\git-tracer.exe
Start-Sleep 4
node tools\cdp.mjs 9223 tools\steps-example.json   # steps 里支持 eval / wait / shot，截图写到 out.png
```

## 结构

```
dist/                      前端（无构建步骤，直接是 Tauri 的 frontendDist）
  index.html
  styles.css
  app.js                   所有交互；通过 window.__TAURI_INTERNALS__.invoke 调后端
src-tauri/
  src/main.rs              入口
  src/lib.rs               Builder、hide_console、async 命令封装
  src/git.rs               git 调用与 porcelain v2 / numstat / diff 解析
  src/scan.rs              目录遍历找 .git
  src/dialog.rs            PowerShell 文件夹选择 + 资源管理器定位
  Cargo.toml
  tauri.conf.json
  capabilities/default.json
tools/
  make-icons.py             生成 icons/（纯标准库写 PNG/ICO/ICNS）
  make-demo-data.ps1        生成自测用的 6 个仓库
  make-package.ps1          打发布包（便携 zip + 安装脚本，不碰 %TEMP%）
  install.ps1               安装 / 卸载脚本本体（全量覆盖；`-Uninstall` 卸载）
  cdp.mjs                   极简 CDP 驱动（截图 / eval / wait），配合上面的调试参数用
docs/
  screenshot.png             主界面（干净仓库 + 待推送 + 提交栏）
  graph.png                  分支树（车道图 + ref 徽标 + 提交列表）
  push-panel.png             待推送面板（哪几个仓库还有提交没推）
```

### 命令接口

| 命令 | 参数 | 返回 |
| --- | --- | --- |
| `git_info` | — | `{available, version, exe, hint}` |
| `scan_repos` | `root, depth, limit?` | `{root, repos[], dirs, errors[], truncated, elapsed_ms}` |
| `repo_status` | `path` | `{path, name, branch, detached, initial, upstream, remotes[], ahead, behind, stashes, head_subject, staged, unstaged, untracked, conflicted, entries[]}` |
| `file_diff` | `path, file, staged, untracked` | `{kind: unified\|text\|binary\|empty\|too_large, text, message}` |
| `repo_graph` | `path, limit?` | `{branch, commits[{hash, short, parents[], author, date, refs, subject}], truncated}` |
| `commit_diff` | `path, hash` | 同 `file_diff`（该提交的补丁） |
| `commit_files` | `path, files[], message` | `{hash, subject, branch, files}` |
| `push_repo` | `path` | `{branch, remote, upstream, set_upstream, output}` |
| `pull_repo` | `path` | `{branch, remote, upstream, summary, output}` |
| `reset_plan` | `path, hash` | `{hash, short, subject, branch, detached, head, drop_commits, gain_commits, dirty}` |
| `reset_repo` | `path, hash, mode` | `{hash, short, subject, branch, mode, drop_commits, dirty_dropped}` |
| `pick_folder` | `initial?` | 选中路径或 `null` |
| `reveal_path` | `path` | — |

`commit_files` 的三条安全限制：只接受仓库内相对路径（拒绝绝对路径、`..`、盘符）、
提交一律带 pathspec（`git commit -m <信息> -- <路径>`）、提交信息为空直接拒绝。
`push_repo` 会拦掉 detached HEAD、没配远程的仓库，并把 git 的凭据 / non-fast-forward 报错翻成中文提示。

## 已知限制

- 仅 Windows（文件夹选择走 PowerShell + WinForms；`hide_console` 也是 Windows 专用）。
- 单仓库 status 超时 60 s，单文件 diff 超时 30 s，diff 预览上限 2 MiB；`git push` 超时 5 分钟。
- 不做自动刷新，需要手动 `F5`；不监听文件系统事件。
- **提交只支持当前仓库、按文件多选**：一次 commit 只带勾选的那些路径，不支持跨仓库一次性提交，
  也不做 `--amend`、不做交互式选择 hunk。
- **推送只推当前分支**（待推送面板也是逐个仓库推各自的当前分支，不同时推 tag / 其他分支）：
  不 force push；凭据弹窗由 git 自己处理
  （`GIT_TERMINAL_PROMPT=0`，需要交互式输入时会直接报错，请在命令行里先配好凭据 / SSH key）。
- 不提供暂存区编辑（`git add -p`）、丢弃单个文件的改动、`clone`、分支切换；分支树是只读的，
  最多 300 个提交，不能在树上做 checkout / merge / tag；「回滚至此处」只做 `git reset`，
  不做 revert（不会生成反向提交），撤销掉的提交只能自己去 reflog 里捞。
- 安装器不是 Windows Installer（MSI）：本机离线环境没有 WiX 工具链，MSI 需要联网才能打；
  `install.ps1` 走的是「复制 + 快捷方式 + HKCU 卸载项」，够用但不进 MSI 的集中部署体系。

## 更新记录

**0.7.0**

- 新增 **`.gittracer.toml` 规则文件**：规则不再只活在命令行参数和界面输入里，写成文件后跟着仓库
  进版本库，本地和 CI 读同一份。两处位置——`<根目录>/.gittracer.toml` 管整批仓库（还能定 `depth`），
  `<仓库>/.gittracer.toml` 只覆盖一个仓库；优先级是「命令行 / 界面显式给的 > 仓库文件 > 根目录文件 >
  内置默认」。`branch = []` 与不写 `branch` 是两件事（明确不检查 vs 继承），`skip` 只做并集。
- 新增 `src-tauri/src/config.rs`（含 `check_ref_like`，`audit::validate` 改为复用它）；依赖 `toml`
  0.9，关掉 `display` 一侧，只留 `std` + `parse` + `serde`。
- **解析失败一律硬失败**（退出码 2，报文件名和位置）：键名拼错、规则名拼错、`depth` 越界、
  值以 `-` 开头或含引号，都不做「读不懂就忽略」。被静默忽略的规则就是假通过。
- 报告里补审计线索：`Report.config_files` 列出本次读到的全部配置文件，`RepoAudit.config` 标出
  仓库自带的配置，`RepoAudit.effective_skip` 记下该仓库实际生效的跳过项；text / md 里对「靠本地
  配置放宽过」的仓库显式标注（md 的已通过列表也会写「跳过 xxx」），json 里是结构化字段。
- `Options` 改成分层：`depth: Option<u32>`、`branch_allow: Option<Vec<String>>`（`None` = 没指定，
  `Some(vec![])` = 明确不检查）、新增 `use_config`。CLI 加 `--no-config`，界面加「使用目录里的
  `.gittracer.toml`」勾选框和「扫描深度」输入（留空 = 配置 / 4）。
- 32 个单测通过（新增 11 个：配置解析、未知键 / 未知规则、`--` 前缀与引号、depth 越界、
  三档优先级、空数组语义、`same_skip`、`command_line` 的 `--no-config`）。
- **使用注意（写进 README）**：配置文件本身没提交时会被 `dirty` 数进去，第一次用会看到
  「未提交改动（.gittracer.toml）」，这是提醒把它一起提交，不是 bug。

**0.6.0**

- 新增**发版前巡检**：界面右上角「巡检」，对扫描到的所有仓库跑一套固定规则（冲突 / 半途的
  merge·rebase / 未提交改动 / detached HEAD / 未推送 / 落后远程 / 无上游 / stash / 分支白名单 /
  遗留分支 / 无远程），按「未通过 / 已通过」出结论，可一键复制 Markdown 报告或等价命令行。
- 新增**命令行程序 `gittracer-audit.exe`**：与 GUI 同一份规则（`audit.rs`），console 子系统，
  三档输出（text / md / json），退出码 `0` 通过 / `1` 有阻断项 / `2` 参数或环境错误，可以直接当
  CI 门禁。Windows 下临时切控制台代码页到 65001 输出中文，退出时还原，不把父控制台改坏。
- 为巡检新增 `git::status_lite`（跳过 `--numstat`，只要计数）和 `git::git_lines`（按行取查询结果）。
- **修掉一个去重缺陷**：同一个仓库经「根目录本身」拿到的是用户输入的分隔符（可能是 `/`），
  遍历出来的子目录是 `\`，只做 `toLowerCase` 时字符串不相等，多根目录下会重复计一个仓库。
  巡检与界面扫描都改成先统一分隔符再比（Rust `audit::path_key` / JS `pathKey`）。
- 打包脚本带上 `gittracer-audit.exe`，安装脚本存在则一并复制。
- 新增 **GitHub Actions 工作流** `.github/workflows/audit.yml`：打 `v*` 标签自动跑、也能在 Actions
  页面填参数手动跑，别的仓库可以 `uses:` 复用；报告写进 job summary 并留 artifact，返回码非 0 就失败。
  必须 `fetch-depth: 0`——浅克隆里没有远程跟踪分支，`ahead` / `behind` / `branch --no-merged` 全算 0，
  检查会静默变成「全部通过」。
- **复查时修掉的四处**：`file_diff` 补上 `check_rel_path`（未跟踪文件预览原先能靠 `../` 读到仓库外，
  提交那条路径本来就有这层校验，diff 漏了）；`--base` / `--branch` 拒绝以 `-` 开头和含引号 / 控制字符的
  取值（`--base --all` 会被 git 当成选项，命令失败后被跳过，结论照样「通过」）；显式给的基准分支在某个
  仓库里不存在时出 WARN 而不是静默跳过；`path_key` 归一末尾分隔符（`D:\work\repo\` 和 `D:\work\repo`
  会被数成两个仓库）。
- 报告渲染：Markdown 转义控制字符（文件名里可以有换行，会把表格拆坏）；文本报告按显示宽度对齐中文仓库名。
- `git` 子进程不再继承 `GIT_DIR` / `GIT_WORK_TREE` / `GIT_INDEX_FILE`——shell 里 export 过这三个变量的话，
  所有命令都会指向别的仓库，对 `reset` / `commit` 就是真事故。
- WebView2 侧开了 CSP（原先 `security.csp` 是 `null`）：`script-src 'self'`、`connect-src ipc:
  http://ipc.localhost`，只给 style 放行内联（界面里大量 `style="margin-left:…"`）。仓库内容渲染本来就
  全部走 `esc()`，这层是防以后某处漏转义。

**0.5.0**

- **界面整套重做**：配色与元素按 HUD 风格来——深蓝黑底、1px 青色细线框、超大字距的全大写小标签、
  蓝色渐变选中条、琥珀色警示、面板与弹窗的四角刻度。信息层级没变，还是同样的三栏，只是把原来偏
  「网页后台」的观感换成了仪表盘。
- 差异区的增删行改成左侧细色条 + 淡底色，冲突文件的两列前缀（`++` / `+ ` / `  `）不再挤在一起。
- 修掉合并冲突文件差异里 `diff --cc` 头被吃掉两个字符的问题（`META_PREFIXES` 现在认所有 `diff --` 开头）。

**0.4.3**

- 分支树新增**模糊搜索**：顶上搜索框边打边筛，匹配提交标题 / 作者 / 分支与标签 / 哈希 / 日期；
  子序列打分（连续、词首、开头加分，长距离跳过扣分），`fgsr` 这种缩写也能命中 `feature/graph-search`，
  多个词用空格分开且都要命中。结果平铺按得分排序、命中的字高亮，点行或回车即可打开该提交的差异。
  `Ctrl+F` 聚焦、`Esc` 清空。

**0.4.2**

- 新增**回滚到任意提交**：在分支树里点某个提交打开差异后，顶部出现「回滚至此处」。
  点开先做一次只读体检（`reset_plan`），把「分支从 A 退到 B / 撤销 N 个提交 / 还有 M 处未提交改动」摊开，
  再选方式——混合（改动留在工作区，默认）、软（改动留在暂存区）、硬（丢弃已跟踪文件的未提交改动，
  需要额外勾选一句确认才放行）。回滚后自动刷新状态并跳回分支树。
- 新增命令 `reset_plan`（只读体检）与 `reset_repo`（`git reset --soft|--mixed|--hard`）。

**0.4.1**

- **文件列表顶上加了「全选」开关**：原先全选 / 取消全选只在底部提交栏，视线要往下跑；现在文件列表的过滤框左侧
  多了一颗三态方框（全选 / 半选 / 未选），右侧是「取消全选」，勾选状态两边同步。
- 提交栏原有的「全选」「取消全选」保留，两处行为一致。

**0.4.0**

- 新增**多根目录绑定**：输入框回车即加一个根目录，标签行列出所有已绑定的根（带仓库数、可点 `×` 移除）；
  多个根目录的扫描结果合并成一个列表，同一个仓库被不同根覆盖时只出现一次，行上用小组签标明归属，
  状态栏写「N 个根目录 · 扫描 M 个目录」。根目录列表存 localStorage，旧的单目录设置自动迁移。
- 新增**拉取**：底部「拉取」按钮（`↓N` 表示落后几个提交），走 `git pull --no-edit`，
  没上游时显式指定 remote 与分支；仓库行新增 `↓N` 徽标；无远程 / detached HEAD / 无提交时按钮禁用。
- 新增命令 `pull_repo`。

**0.3.0**

- 新增**分支树**（右侧「差异 / 分支树」标签）：`git log --all --date-order` 取最近 300 个提交，
  前端自算车道布局、用 SVG 画车道线与分叉/合并曲线，行上显示短哈希、ref 徽标、提交标题、作者与日期；
  点一行就在「差异」标签里看这个提交的补丁。实测 241 个提交的仓库：取数 159 ms、布局 <1 ms。
- **精简界面文案**：空状态只留标题、弹窗去掉解释段、toast 缩短、`git version 2.49.0.windows.1`
  只显示成 `git 2.49.0`；同时修掉提交信息按钮里一个多出来的 `>`。
- 新增命令 `repo_graph`、`commit_diff`。

**0.2.1**

- **没有待提交项的仓库也显示**：仓库列表默认列出全部找到的仓库（干净的灰一点、标「干净」），
  「只看有变更」改为需要主动勾选才生效（旧版本存在 localStorage 里的开关状态不再沿用）。
- **待推送面板**：统计条上多一个「待推送 N 个仓库」，点开能看到分别是哪几个仓库有没推的提交、
  各差几个、推到哪个上游，支持逐个推和「全部推送（N 个）」；仓库行上也加了 `↑N` 徽标。
- **全选 / 取消全选拆成两个按钮**（原来是一个按钮来回切），并把「已勾选数 / 总数」做成按钮的 tooltip。
- **提交信息固定高度、不可拖拽**：底部输入框固定 32px，右边加 `⤢` 按钮开弹窗用大文本框写多行
  （`Ctrl+Shift+E`）；弹窗里 `Ctrl+Enter` 直接走提交流程。
- 顺手修掉模态框里 `[hidden]` 被组件 `display` 规则盖掉的 bug（「知道了」弹窗原来会多出一个「取消」）；
  仓库路径统一显示成反斜杠。

**0.2.0**

- 新增**多选提交**：文件列表带勾选框（点方框 / 按空格勾当前行 / 「全选」），
  底部填提交信息后点「提交选中」，确认框里核对信息与文件清单，后端用
  `git add -A -- <勾选路径>` + `git commit -m <信息> -- <勾选路径>` 提交，
  只带这几个路径，别处已暂存的内容不会被裹进来。快捷键 `Ctrl+Enter`。
- 新增**推送**：底部「推送」按钮，按钮上带 `↑N` 显示待推提交数；有上游直接推，没上游自动
  `git push -u`；detached HEAD、没配远程的仓库会被拦下并给出中文原因。
- `repo_status` 多返回 `remotes[]`，界面据此在没配远程时直接禁用推送按钮。
- 安装改为**全量覆盖**：先停掉运行中的旧版本、整个删除安装目录再重建（失败重试 6 次），
  快捷方式也先删后建，旧版本遗留文件不会残留。

**0.1.1**

- 修「浏览」按钮报 `MissingEndParenthesisInMethodCall`：PowerShell 脚本里
  `(New-Object System.Text.UTF8Encoding($false))` 这种写在方法实参里的语法在 5.1 不合法；
  改成 `-ArgumentList $false`，并把脚本按 UTF-8 BOM 落盘、用 `-File` 执行，
  顺带修掉中文乱码（原来 GBK 的 stderr 被当 UTF-8 读）。
- 文件夹对话框现在会停在当前输入的目录（`FolderBrowserDialog.SelectedPath` 只认反斜杠且要求目录存在，
  先把 `/` 归一化再用 `Directory.Exists` 守卫）。
- WebView2 数据目录改成多级兜底，`%TEMP%` / AppData 不可写时也能起窗口。
- 发布包从 NSIS 换成「便携 zip + `install.ps1`」，不再依赖 `%TEMP%`。

