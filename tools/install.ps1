# Git Tracer 的安装 / 卸载脚本（只用 .NET 与 COM，不碰 %TEMP%）
#
#   安装：powershell -NoProfile -ExecutionPolicy Bypass -File install.ps1
#   卸载：powershell -NoProfile -ExecutionPolicy Bypass -File uninstall.ps1 -Uninstall
#
# 为什么不用 NSIS / MSI 安装包：实测 NSIS 安装器启动时固定要往 %TEMP% 里解压自己的
# 数据（换 4 种配置都一样），TEMP 不可写就弹 "Error writing temporary file."。
# 这个脚本把 exe 复制到 %LOCALAPPDATA%\Programs\GitTracer、建快捷方式、写卸载项，
# 全程只用文件复制 / WScript.Shell / 注册表，因此在 TEMP 被拒的环境里也能装。

param(
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'

$App        = 'Git Tracer'
$Version    = '0.9.0'
$ExeName    = 'GitTracer.exe'
$AuditName  = 'gittracer-audit.exe'
$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\GitTracer'
$RegKey     = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\GitTracer'
$RegState   = 'HKCU:\Software\GitTracer'
$StartMenu  = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$Desktop    = [Environment]::GetFolderPath('Desktop')
$LnkName    = 'Git Tracer.lnk'
$ReadmeName = '使用说明.txt'
$SelfName   = 'uninstall.ps1'

function Write-Step([string]$text) { Write-Host "  $text" }

function New-Shortcut([string]$lnkPath, [string]$target) {
    if (-not (Test-Path (Split-Path -Parent $lnkPath))) { return }
    $sh = New-Object -ComObject WScript.Shell
    $lnk = $sh.CreateShortcut($lnkPath)
    $lnk.TargetPath = $target
    $lnk.WorkingDirectory = $InstallDir
    $lnk.IconLocation = $target
    $lnk.Description = '查看目录下所有 Git 仓库的待提交内容'
    $lnk.Save()
}

function Remove-Shortcuts {
    foreach ($p in @((Join-Path $StartMenu $LnkName), (Join-Path $Desktop $LnkName))) {
        if (Test-Path -LiteralPath $p) { Remove-Item -LiteralPath $p -Force; Write-Step "删除快捷方式 $p" }
    }
}

# ---------------------------------------------------------------- 卸载
if ($Uninstall) {
    Write-Host "卸载 $App ..."
    Get-Process -Name 'GitTracer' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 300
    Remove-Shortcuts
    if (Test-Path -LiteralPath $InstallDir) {
        Remove-Item -LiteralPath $InstallDir -Recurse -Force
        Write-Step "删除目录 $InstallDir"
    }
    foreach ($k in @($RegKey, $RegState)) {
        if (Test-Path $k) { Remove-Item -Path $k -Recurse -Force; Write-Step "清理注册表 $k" }
    }
    Write-Host "已卸载。"
    exit 0
}

# ---------------------------------------------------------------- 安装
$srcDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
$srcExe = Join-Path $srcDir $ExeName
if (-not (Test-Path -LiteralPath $srcExe)) {
    throw "找不到 $ExeName。请把本脚本和 $ExeName 放在同一个目录里再运行。"
}

Write-Host "安装 $App $Version 到 $InstallDir"

# 更新安装 = 全量覆盖：先停掉正在运行的旧版本，再把整个安装目录清空重建。
# 不这么做的话，旧版本里已经不存在的文件（改过名的资源、旧的 uninstall.ps1、
# 残留的 dll）会一直躺在目录里，升级后的行为可能和全新安装对不上。
Get-Process -Name 'GitTracer' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 300

if (Test-Path -LiteralPath $InstallDir) {
    $cleared = $false
    for ($i = 0; $i -lt 6; $i++) {
        try {
            Remove-Item -LiteralPath $InstallDir -Recurse -Force -ErrorAction Stop
            $cleared = $true
            break
        } catch {
            # 进程刚退出 / 资源管理器预览 / 杀软扫描时句柄可能还在，等一会儿再试
            Start-Sleep -Milliseconds 400
            Get-Process -Name 'GitTracer' -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
        }
    }
    if (-not $cleared) {
        throw "无法清空旧版本目录：$InstallDir（文件被占用，请先关掉 Git Tracer 再装）"
    }
    Write-Step "清空旧版本目录（全量覆盖）"
}
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null

Copy-Item -LiteralPath $srcExe -Destination (Join-Path $InstallDir $ExeName) -Force
Write-Step "复制 $ExeName"

# 巡检命令行程序：老版本包里没有就跳过，别让安装失败
$srcAudit = Join-Path $srcDir $AuditName
if (Test-Path -LiteralPath $srcAudit) {
    Copy-Item -LiteralPath $srcAudit -Destination (Join-Path $InstallDir $AuditName) -Force
    Write-Step "复制 $AuditName"
}

$srcReadme = Join-Path $srcDir $ReadmeName
if (Test-Path -LiteralPath $srcReadme) {
    Copy-Item -LiteralPath $srcReadme -Destination (Join-Path $InstallDir $ReadmeName) -Force
    Write-Step "复制 $ReadmeName"
}

# 卸载脚本随程序一起放进去，卸载时不再依赖原始 zip
$selfPath = if ($PSCommandPath) { $PSCommandPath } else { $MyInvocation.MyCommand.Path }
if ($selfPath -and (Test-Path -LiteralPath $selfPath)) {
    Copy-Item -LiteralPath $selfPath -Destination (Join-Path $InstallDir $SelfName) -Force
    Write-Step "复制 $SelfName"
}

$installedExe = Join-Path $InstallDir $ExeName
# 旧快捷方式先删再建：指向别处的旧 lnk 不会因为全量覆盖而自动消失
Remove-Shortcuts
New-Shortcut (Join-Path $StartMenu $LnkName) $installedExe
New-Shortcut (Join-Path $Desktop $LnkName) $installedExe
Write-Step '创建开始菜单 / 桌面快捷方式'

$uninstallCmd = 'powershell -NoProfile -ExecutionPolicy Bypass -File "{0}" -Uninstall' -f (Join-Path $InstallDir $SelfName)
if (-not (Test-Path $RegKey)) { New-Item -Path $RegKey -Force | Out-Null }
Set-ItemProperty -Path $RegKey -Name 'DisplayName'          -Value $App
Set-ItemProperty -Path $RegKey -Name 'DisplayVersion'       -Value $Version
Set-ItemProperty -Path $RegKey -Name 'Publisher'            -Value 'Git Tracer'
Set-ItemProperty -Path $RegKey -Name 'DisplayIcon'          -Value $installedExe
Set-ItemProperty -Path $RegKey -Name 'InstallLocation'      -Value $InstallDir
Set-ItemProperty -Path $RegKey -Name 'UninstallString'      -Value $uninstallCmd
Set-ItemProperty -Path $RegKey -Name 'QuietUninstallString' -Value $uninstallCmd
Set-ItemProperty -Path $RegKey -Name 'NoModify'             -Value 1 -Type DWord
Set-ItemProperty -Path $RegKey -Name 'NoRepair'             -Value 1 -Type DWord
if (-not (Test-Path $RegState)) { New-Item -Path $RegState -Force | Out-Null }
Set-ItemProperty -Path $RegState -Name 'InstallDir' -Value $InstallDir
Set-ItemProperty -Path $RegState -Name 'Version'    -Value $Version
Set-ItemProperty -Path $RegState -Name 'InstallDate' -Value (Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
Write-Step '写入卸载注册表项'

Write-Host ''
Write-Host "装好了。开始菜单里搜「Git Tracer」就能启动，也可以直接跑："
Write-Host "  $installedExe"
