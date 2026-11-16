# 生成一份用于自测 / 截图的演示数据：多个状态各异的 git 仓库
$ErrorActionPreference = 'Stop'
$root = 'D:\Projects\_git-tracer-demo'
if (Test-Path $root) { Remove-Item $root -Recurse -Force }
New-Item -ItemType Directory -Path $root | Out-Null

function New-DemoRepo([string]$path) {
  New-Item -ItemType Directory -Path $path -Force | Out-Null
  Push-Location $path
  git init -q -b main
  git config user.name 'Demo User'
  git config user.email 'demo@example.com'
  git config commit.gpgsign false
  git config core.autocrlf false
  Pop-Location
}

function Write-Text([string]$path, [string]$text) {
  $dir = Split-Path $path -Parent
  if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
  [System.IO.File]::WriteAllText($path, $text)
}

# ---------- 1. alpha-api：干净仓库 ----------
$p = Join-Path $root 'alpha-api'
New-DemoRepo $p
Write-Text (Join-Path $p 'README.md') "# alpha-api`n`nInternal REST service."
Write-Text (Join-Path $p 'src/app.py') "from fastapi import FastAPI`n`napp = FastAPI()`n`n@app.get('/health')`ndef health():`n    return {'ok': True}`n"
Write-Text (Join-Path $p 'src/db.py') "def connect(dsn):`n    return dsn`n"
Write-Text (Join-Path $p 'tests/test_app.py') "def test_health():`n    assert True`n"
Push-Location $p; git add -A; git commit -qm "feat: bootstrap alpha-api"; Pop-Location

# 给它一个真正的远程并推上去：这样它才是「干净 + 已推送 + 有上游」的仓库，
# 巡检报告里才会出现一个真正的绿色「通过」。否则每个演示仓库都停在
# 「有活没提交」的注意状态，截图说明不了三种结论的区别。
$remotes = 'D://Projects//_git-tracer-demo-remotes'
New-Item -ItemType Directory -Path $remotes -Force | Out-Null
$bare = Join-Path $remotes 'alpha-api.git'
if (Test-Path $bare) { Remove-Item $bare -Recurse -Force }
git init -q --bare $bare
Push-Location $p
git remote add origin $bare
git push -q -u origin main
Pop-Location

# ---------- 2. beta-web：已暂存 + 未暂存 + 未跟踪 + 中文文件名 ----------
$p = Join-Path $root 'beta-web'
New-DemoRepo $p
Write-Text (Join-Path $p 'index.html') "<!doctype html>`n<html><body><div id=app></div></body></html>`n"
Write-Text (Join-Path $p 'src/main.js') "import { render } from './render.js';`n`nconst el = document.querySelector('#app');`nrender(el, { title: 'beta-web' });`n"
Write-Text (Join-Path $p 'src/render.js') "export function render(el, state) {`n  el.textContent = state.title;`n}`n"
Write-Text (Join-Path $p 'src/util.ts') "export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));`n"
Write-Text (Join-Path $p 'styles.css') "body { margin: 0; font-family: system-ui; }`n"
Write-Text (Join-Path $p 'README.md') "# beta-web`n`nFrontend playground."
Push-Location $p; git add -A; git commit -qm "feat: initial web shell"; Pop-Location

# 未暂存：改两个文件
Write-Text (Join-Path $p 'src/main.js') "import { render } from './render.js';`nimport { clamp } from './util.ts';`n`nconst el = document.querySelector('#app');`nrender(el, { title: 'beta-web' });`n`nwindow.addEventListener('resize', () => {`n  render(el, { title: 'beta-web', width: clamp(window.innerWidth, 320, 1920) });`n});`n"
Write-Text (Join-Path $p 'styles.css') "body { margin: 0; font-family: system-ui; }`n.app-shell { display: grid; gap: 12px; padding: 24px; }`n.app-shell > * { min-width: 0; }`n"
# 已暂存：改 util.ts 并 add
Write-Text (Join-Path $p 'src/util.ts') "export const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));`nexport const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));`nexport const noop = () => {};`n"
Push-Location $p; git add src/util.ts; Pop-Location
# 未跟踪：新文件 + 中文文件名
Write-Text (Join-Path $p 'src/new-feature.ts') "export function featureFlag(name: string): boolean {`n  return name.startsWith('beta.');`n}`n"
Write-Text (Join-Path $p 'docs/接口说明.md') "# 接口说明`n`n- GET /health`n- POST /render`n`n> 该文件用于验证中文路径的显示与暂存区读取。`n"
Write-Text (Join-Path $p 'notes.md') "TODO:`n- [ ] 接入路由`n- [ ] 拆分包体积`n"

# ---------- 3. gamma-service：合并冲突 ----------
$p = Join-Path $root 'gamma-service'
New-DemoRepo $p
Write-Text (Join-Path $p 'app.go') "package main`n`nfunc main() {`n`tprintln(`"gamma`")`n}`n"
Write-Text (Join-Path $p 'config.yaml') "port: 8080`nlog: info`n"
Push-Location $p; git add -A; git commit -qm "feat: gamma skeleton"; Pop-Location
# 两边在**同一行位置**各加一句不同的日志，才会真的冲突；
# 改不同位置 git 会自己合掉，那样演示数据里就没有冲突仓库了。
Push-Location $p
git checkout -q -b feature/rate-limit
Write-Text (Join-Path $p 'app.go') "package main`n`nfunc main() {`n`ttprintln(`"gamma`")`n`ttprintln(`"rate limited`")`n}`n"
git commit -qam "feat: add rate limiter"
git checkout -q main
Write-Text (Join-Path $p 'app.go') "package main`n`nfunc main() {`n`ttprintln(`"gamma`")`n`ttprintln(`"boot ok`")`n}`n"
git commit -qam "fix: log boot ok"
git merge feature/rate-limit 2>&1 | Out-Null
Pop-Location

# ---------- 4. delta-tools：重命名 + 删除 + 二进制 ----------
$p = Join-Path $root 'delta-tools'
New-DemoRepo $p
Write-Text (Join-Path $p 'README.md') "# delta-tools`n`nCLI helpers."
Write-Text (Join-Path $p 'src/old-name.rs') "pub fn run() {`n    println!(`"run`");`n}`n"
Write-Text (Join-Path $p 'src/legacy.rs') "pub fn legacy() {}`n"
New-Item -ItemType Directory -Path (Join-Path $p 'assets') -Force | Out-Null
$bytes = [byte[]](0..255)
[System.IO.File]::WriteAllBytes((Join-Path $p 'assets/logo.bin'), $bytes)
Push-Location $p; git add -A; git commit -qm "feat: delta cli"; Pop-Location
Push-Location $p
git mv src/old-name.rs src/new-name.rs
Remove-Item (Join-Path $p 'src/legacy.rs')
Pop-Location
$bytes2 = [byte[]](0..255 | ForEach-Object { ($_ * 7) % 256 })
[System.IO.File]::WriteAllBytes((Join-Path $p 'assets/logo.bin'), $bytes2)
Write-Text (Join-Path $p 'README.md') "# delta-tools`n`nCLI helpers for migrations.`n`n## Usage`n`n    delta migrate --dry-run`n`n## Notes`n`nBinary assets live under assets/."

# ---------- 5. 嵌套仓库 packages/ui-kit（只有暂存区） ----------
$p = Join-Path $root 'packages/ui-kit'
New-DemoRepo $p
Write-Text (Join-Path $p 'src/Button.tsx') "export const Button = () => null;`n"
Write-Text (Join-Path $p 'package.json') "{ `"name`": `"ui-kit`", `"version`": `"0.1.0`" }`n"
Push-Location $p; git add -A; git commit -qm "feat: ui kit"; Pop-Location
Write-Text (Join-Path $p 'src/Button.tsx') "export const Button = () => null;`nexport const IconButton = () => null;`n"
Write-Text (Join-Path $p 'src/Input.tsx') "export const Input = () => null;`n"
Push-Location $p; git add -A; Pop-Location

# ---------- 6. 依赖目录里的仓库（应被扫描器跳过） ----------
$p = Join-Path $root 'deps/node_modules/@acme/widgets'
New-DemoRepo $p
Write-Text (Join-Path $p 'index.js') "module.exports = {};`n"
Push-Location $p; git add -A; git commit -qm "vendor"; Pop-Location

# ---------- 7. 非仓库目录 ----------
New-Item -ItemType Directory -Path (Join-Path $root 'scratch/docs') -Force | Out-Null
Write-Text (Join-Path $root 'scratch/docs/todo.md') "- 随便写点东西，不是仓库`n"

# ---------- 8. 配置文件示例：根目录一份 + 仓库一份 ----------
# 根目录这份管下面所有仓库；packages/ui-kit 自己那份只放宽它自己，
# 界面上会给它挂「本地配置」和「跳过 1」，报告里也写得出来路。
Write-Text (Join-Path $root '.gittracer.toml') "# 根目录这份：管下面所有仓库`nbranch = [`"main`"]`nbase = `"main`"`nskip = [`"stash`"]`n"
Write-Text (Join-Path $root 'packages/ui-kit/.gittracer.toml') "# 这个仓库自己放宽一条：它的改动都在暂存区，发版时只当提示`nskip = [`"dirty`"]`n"

Write-Host '演示数据已生成：' $root
Get-ChildItem $root -Directory | Select-Object -ExpandProperty Name
