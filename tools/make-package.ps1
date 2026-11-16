# 打发布包：便携 zip（解压即用 / 双击安装脚本装到本机），全程不碰 %TEMP%。
#
# 用法（PowerShell 7 或 5.1 都行，本文件带 UTF-8 BOM）：
#   powershell -NoProfile -ExecutionPolicy Bypass -File tools\make-package.ps1
#
# 关于安装器：
#   `tauri build` 自带的 NSIS 安装包会在启动时把 nsis_tauri_utils 插件和自身数据解到
#   %TEMP%\nsuXXXX.tmp。实测四种 NSIS 配置（MUI2/经典页面 × solid/zlib、有无插件）
#   在 %TEMP% 不可写时全部报 "Error writing temporary file."，说明这是 NSIS 的固有行为，
#   不是我们脚本或插件的问题。所以零 TEMP 的安装改用两样东西：
#     1) 便携版：解压出来直接双击 GitTracer.exe；
#     2) install.ps1：复制文件 + 建快捷方式 + 写卸载项，只用 .NET / COM / 注册表。

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
# 必须显式按 UTF-8 读：PowerShell 5.1 的 Get-Content 遇到无 BOM 的文件会按 GBK 解，
# package.json 里的中文 description 会被解成乱码，甚至把结尾的引号吃掉导致 ConvertFrom-Json 报错。
$pkgText = [System.IO.File]::ReadAllText((Join-Path $root 'package.json'), [System.Text.Encoding]::UTF8)
$version = ($pkgText | ConvertFrom-Json).version
$releaseExe = Join-Path $root 'src-tauri\target\release\git-tracer.exe'
if (-not (Test-Path $releaseExe)) { throw "找不到 $releaseExe，先跑 release 构建" }

$stage = Join-Path $root '.tmp\pkg'
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage -Force | Out-Null

$appExe = Join-Path $stage 'GitTracer.exe'
Copy-Item $releaseExe $appExe -Force

# 巡检命令行程序（console 子系统，CI 里直接调，按返回码判定）
$releaseAudit = Join-Path $root 'src-tauri\target\release\gittracer-audit.exe'
if (-not (Test-Path $releaseAudit)) { throw "找不到 $releaseAudit，先跑 release 构建" }
Copy-Item $releaseAudit (Join-Path $stage 'gittracer-audit.exe') -Force

# ---------------------------------------------------------------- 使用说明
$readme = @"
Git Tracer $version
=================================

看到一个目录下所有 Git 仓库的待提交内容。

一、怎么启动
-----------
方式 A（不用安装）：直接双击本目录里的 GitTracer.exe。
方式 B（装到本机，带开始菜单和桌面图标）：
    双击「安装.cmd」，等它跑完。装好以后从开始菜单搜「Git Tracer」启动，
    想卸载就双击「卸载.cmd」，或者去「设置 → 应用」里卸载。

二、怎么用
---------
1. 顶部填一个目录（或者点「浏览」挑一个），回车就把它加进来并开始扫描。
   可以连续加多个目录，它们排成一行标签（带各自的仓库数），点标签上的 × 就移除。
   每个目录下会递归找 Git 仓库（默认 4 层，可下拉改到 12 层）。
2. 左边选仓库，中间选文件，右边看 diff。
   文件按「冲突 / 已暂存 / 未暂存 / 未跟踪」分组，未跟踪文件直接当文本预览。
   右边顶部还能切到「分支树」，看这个仓库的提交历史（车道图 + 分支/标签徽标），
   点某个提交就在「差异」里显示它的改动。
   分支树顶上的搜索框能模糊搜提交：标题、作者、分支/标签、哈希、日期都能搜，
   缩写也行（比如 fgsr 能搜到 feature/graph-search），多个词用空格分开；
   回车打开第一个结果，Esc 清空。
3. 干净仓库默认也会列出来（标「干净」），勾上「只看有变更」才隐藏；
   行上带 ↑N 表示这个仓库有 N 个提交还没推上去，↓N 表示远程有 N 个提交还没拉下来。
   F5 或 Ctrl+R 重新扫描。绑了多个目录时，行上有个小标签说明它属于哪个目录。
4. 要提交：在文件列表左边勾上文件；列表顶上那颗「全选」方框能一次勾完（半选样子表示只勾了一部分），
   旁边是「取消全选」；底部提交栏也有同样的两个按钮，两处同步。
   然后写提交信息（要写多行就点输入框右边的 ⤢ 按钮，在弹窗里写），
   点「提交选中」，确认框里核对完再点「提交」。
   只提交你勾的那几个文件，别处已经暂存的内容不会被带进来。
5. 要推送：底部「推送」按钮推当前仓库，按钮上有 ↑N 就是还有 N 个提交没推上去，
   这个分支还没设上游时会自动 git push -u。
   想知道一共有哪些仓库要推，点统计条上的「待推送 N 个仓库」，可以逐个推，也可以一次全推。
6. 要拉取：底部「拉取」按钮把远程的提交拉下来，落后几个就在按钮上显示 ↓N。
   有上游直接 git pull，没上游会显式指定远程和分支；拉完会告诉你结果
   （已快进 / 已是最新 / 已合并远程提交）。没有远程仓库时按钮是灰的。
7. 要回滚到某个旧提交：切到「分支树」，点那个提交，差异面板顶部有「回滚至此处」。
   弹窗会先说清后果（当前分支从哪个提交退到哪个、撤销几个提交、还有几处未提交改动），
   再选方式：混合（改动留在工作区）、软（改动留在暂存区）、硬（丢弃已跟踪文件的未提交改动，
   要另外勾一句确认才放行）。回滚完自动刷新并跳回分支树。

快捷键：F5 / Ctrl+R 重新扫描，/ 聚焦仓库过滤，空格勾选当前文件，
Ctrl+Enter 提交，Ctrl+Shift+E 打开大文本框写提交信息，
Ctrl+F 在分支树里搜提交。

三、什么时候才会改动你的仓库
--------------------------
扫描、看状态、看 diff 全是只读查询（status / diff / log），所有命令都带 --no-optional-locks，
不会留下 index.lock。只有你点「提交选中」「拉取」「推送」和「回滚至此处」时才会真正改动仓库
（git add / commit / pull / push / reset）。不做 --amend、不 force push、不推其他分支；
回滚只用 reset，不生成反向提交，撤销掉的提交只留在 reflog 里。

四、需要什么
-----------
- Windows 10/11（自带 WebView2 的版本）
- 机器上装了 Git（在 PATH 里，或者默认装在 Program Files）
- 推送需要仓库已配好远程与凭据（HTTPS 密码 / SSH key）。程序不弹凭据输入框，
  需要输密码时请先在命令行里配好（凭据管理器或 SSH key）。

五、更新安装会全量覆盖
--------------------
装新版时会先关掉正在运行的 Git Tracer，把安装目录整个删掉再重建，
所以旧版本遗留的文件不会留在目录里。提示「文件被占用」时关掉 Git Tracer 再装一次。

六、环境变量（一般不用管）
-------------------------
GIT_TRACER_DATA_DIR   指定窗口数据的存放目录。默认依次试
                      %LOCALAPPDATA%\com.git-tracer.app\webview2、%TEMP%\git-tracer-webview2、
                      exe 同级的 .git-tracer-webview2。前两个都不可写时会自动往后退，
                      所以放在只读目录里也能启动。

七、巡检（发版前体检）
---------------------
右上角的「巡检」按钮会把所有仓库按一套固定规则过一遍：有没有未提交改动、冲突、
没推的提交、detached HEAD、没做完的合并 / 变基、落后远程、没上游、stash、
分支是否符合要求、有没有遗留的本地分支。结果按「通过 / 未通过」列出来，
可以复制成 Markdown 贴进发版单。全程只读。

要接进 CI，用同一个目录里的 gittracer-audit.exe：

    gittracer-audit --root D:\work --branch "main,release/*" --base main

返回码：0 通过，1 有阻断项，2 参数或目录有问题。完整参数见：

    gittracer-audit --help

规则可以写进 .gittracer.toml，跟着仓库一起进版本库，本地和 CI 读同一份：

    <根目录>\.gittracer.toml   管这个目录下的所有仓库，还能定扫描深度
    <仓库>\.gittracer.toml     只覆盖这一个仓库

四个键：depth / branch / base / skip。命令行显式给的参数优先于文件；
加 --no-config 可以整个关掉（界面上是那个勾选框）。文件写错了会直接报错退出，
不会「读不懂就忽略」。注意这个文件本身也要提交，否则巡检会把它算进「未提交改动」。

八、为什么没有 exe 安装包
-----------------------
NSIS 安装器启动时固定要往 %TEMP% 里解压自己的数据，%TEMP% 不可写时会直接报
"Error writing temporary file."。这台机器的受限环境里就是这样，所以改用「便携版 +
安装脚本」：两者都不碰临时目录。
"@
$readmePath = Join-Path $stage '使用说明.txt'
# 带 BOM 落盘：记事本 / chcp 936 的控制台都能正确认中文（PowerShell 5.1 没有 utf8BOM 这个 -Encoding 值）
[System.IO.File]::WriteAllText($readmePath, $readme, (New-Object System.Text.UTF8Encoding $true))

# ---------------------------------------------------------------- 安装脚本
$installSrc = Join-Path $PSScriptRoot 'install.ps1'
if (-not (Test-Path $installSrc)) { throw "找不到 $installSrc" }
$installText = Get-Content -Raw -Encoding utf8 $installSrc
$m = [regex]::Match($installText, "(?m)^\`$Version\s*=\s*'[^']*'")
if ($m.Success) {
    $installText = $installText.Replace($m.Value, "`$Version    = '$version'")
} else {
    throw 'install.ps1 里找不到 $Version 那一行，没法同步版本号'
}
[System.IO.File]::WriteAllText((Join-Path $stage 'install.ps1'), $installText, (New-Object System.Text.UTF8Encoding $true))

$cmdInstall = @'
@echo off
setlocal
chcp 65001 >nul
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1"
set rc=%errorlevel%
echo.
if not "%rc%"=="0" echo Failed, exit code %rc%.
pause
exit /b %rc%
'@
Set-Content -Path (Join-Path $stage '安装.cmd') -Value $cmdInstall -Encoding ascii

$cmdUninstall = @'
@echo off
setlocal
chcp 65001 >nul
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Uninstall
set rc=%errorlevel%
echo.
if not "%rc%"=="0" echo Failed, exit code %rc%.
pause
exit /b %rc%
'@
Set-Content -Path (Join-Path $stage '卸载.cmd') -Value $cmdUninstall -Encoding ascii

# ---------------------------------------------------------------- 打包
$zip = Join-Path $root "GitTracer-$version-portable.zip"
if (Test-Path $zip) {
    try { Remove-Item $zip -Force } catch { throw "旧的 $zip 被占用（先关掉正在预览它的窗口）: $($_.Exception.Message)" }
}
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal
"便携版: $zip ($([math]::Round((Get-Item $zip).Length / 1MB, 2)) MB)"
"包内文件: " + ((Get-ChildItem $stage -File | ForEach-Object { $_.Name }) -join ', ')
