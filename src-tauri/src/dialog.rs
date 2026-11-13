//! 系统「选择文件夹」对话框。
//!
//! 零依赖实现：借 Windows PowerShell 的 WinForms `FolderBrowserDialog`。
//!
//! 两个坑（0.1.0 就栽在这上面，报 `MissingEndParenthesisInMethodCall`）：
//!
//! 1. `New-Object System.Text.UTF8Encoding($false)` 这种写法**在方法调用的实参里不是合法语法**，
//!    必须写成 `-ArgumentList $false`；
//! 2. 中文脚本不能靠 `-Command` 传字面量（编码/引号都容易翻车），改成把脚本写成
//!    **带 BOM 的 UTF-8 .ps1** 再用 `-File` 执行，PowerShell 5.1 才会按 UTF-8 读。
//!
//! 选中结果经 UTF-8 临时文件回传（走管道会被 OEM 代码页弄乱中文路径），
//! 出错也写进同一个文件（`ERR:` 前缀），不再依赖 stderr 的代码页。

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::hide_console;

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 生成脚本正文（不含 BOM）。`out` 是结果文件路径，`desc` 是对话框说明，`init` 是初始目录。
fn script_text(out: &Path, desc: &str, init: &str) -> String {
    let out = ps_quote(&out.display().to_string());
    let init_raw = init.trim();
    let init_check = if init_raw.is_empty() {
        // 没给初始目录就不写这句，省得 Test-Path '' 报警告
        String::new()
    } else {
        // FolderBrowserDialog.SelectedPath 很挑：路径必须是正斜杠换成反斜杠、
        // 且真实存在，否则它悄悄忽略，对话框就停在「桌面」。
        format!(
            "$init = {0}\n  $init = $init.Replace('/', '\\')\n  \
             if ([System.IO.Directory]::Exists($init)) {{ $d.SelectedPath = $init }}\n",
            ps_quote(init_raw)
        )
    };
    let enc = "New-Object System.Text.UTF8Encoding -ArgumentList $false";
    format!(
        "$ErrorActionPreference = 'Stop'\n\
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8\n\
$out = {out}\n\
try {{\n\
  Add-Type -AssemblyName System.Windows.Forms | Out-Null\n\
  $d = New-Object System.Windows.Forms.FolderBrowserDialog\n\
  $d.Description = {desc}\n\
  $d.ShowNewFolderButton = $true\n\
  {init_check}  if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK) {{\n\
    [System.IO.File]::WriteAllText($out, $d.SelectedPath, ({enc}))\n\
  }} else {{\n\
    [System.IO.File]::WriteAllText($out, '', ({enc}))\n\
  }}\n\
}} catch {{\n\
  [System.IO.File]::WriteAllText($out, 'ERR:' + $_.Exception.Message, ({enc}))\n\
  exit 1\n\
}}\n",
        out = out,
        desc = ps_quote(desc),
        init_check = init_check,
        enc = enc,
    )
}

/// 找个能写文件的临时位置：系统临时目录 → 当前目录 → exe 所在目录。
///
/// 受限环境里 `%TEMP%` 可能不可写，而 PowerShell 子进程写失败只会给一句
/// 没头没脑的报错，所以先在这儿挑一个能建文件的位置。
fn work_dir() -> PathBuf {
    let mut candidates = vec![std::env::temp_dir()];
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.to_path_buf());
        }
    }
    for dir in &candidates {
        let probe = dir.join(format!("git-tracer-probe-{}.tmp", std::process::id()));
        if std::fs::write(&probe, b"x").is_ok() {
            let _ = std::fs::remove_file(&probe);
            return dir.clone();
        }
    }
    candidates.remove(0)
}

/// 弹出系统文件夹选择框；`Ok(None)` = 用户取消。
pub fn pick_folder(desc: &str, initial_dir: &str) -> Result<Option<String>, String> {
    #[cfg(windows)]
    {
        let dir = work_dir();
        let pid = std::process::id();
        let out_path = dir.join(format!("git-tracer-pick-{pid}.txt"));
        let script_path = dir.join(format!("git-tracer-pick-{pid}.ps1"));
        let _ = std::fs::remove_file(&out_path);

        let text = script_text(&out_path, desc, initial_dir);
        // 带 BOM 的 UTF-8：PowerShell 5.1 见到 BOM 才按 UTF-8 解析中文
        let mut bytes = Vec::with_capacity(text.len() + 3);
        bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        bytes.extend_from_slice(text.as_bytes());
        std::fs::write(&script_path, &bytes)
            .map_err(|e| format!("无法写入临时脚本 {}：{e}", script_path.display()))?;

        let mut cmd = Command::new("powershell");
        cmd.args([
            "-NoProfile",
            "-STA",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ]);
        cmd.arg(&script_path);
        // 不隐藏的话，每点一次「浏览」都会闪一个控制台窗口
        hide_console(&mut cmd);
        let output = cmd.output().map_err(|e| format!("无法打开系统对话框：{e}"))?;

        let raw = std::fs::read_to_string(&out_path).unwrap_or_default();
        let _ = std::fs::remove_file(&out_path);
        let _ = std::fs::remove_file(&script_path);
        let picked = raw.trim().to_string();

        if let Some(msg) = picked.strip_prefix("ERR:") {
            return Err(format!("系统对话框失败：{}", msg.trim()));
        }
        if !picked.is_empty() {
            return Ok(Some(picked));
        }

        // 结果文件没写出来：要么用户取消，要么脚本根本没跑起来
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.is_empty() {
            Ok(None)
        } else {
            Err(format!("系统对话框失败：{stderr}"))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (desc, initial_dir);
        Err("当前平台不支持系统文件夹对话框，请直接输入路径".to_string())
    }
}

/// 在资源管理器中定位到某个文件（或直接打开目录）。
pub fn reveal(path: &str) -> Result<(), String> {
    let p = std::path::Path::new(path);
    let (arg, dir) = if p.is_dir() {
        (p.display().to_string(), p.display().to_string())
    } else {
        (
            format!("/select,{}", p.display()),
            p.parent().unwrap_or(p).display().to_string(),
        )
    };
    let mut cmd = Command::new("explorer");
    if arg.starts_with("/select,") {
        cmd.arg(&arg);
    } else {
        cmd.arg(&dir);
    }
    hide_console(&mut cmd);
    // explorer 成功时也可能返回非 0，这里不等它，直接认为已触发
    cmd.spawn().map(|_| ()).map_err(|e| format!("无法打开资源管理器：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_quotes_single_quote() {
        let out = Path::new(r"C:\tmp\o.txt");
        let s = script_text(out, "it's mine", "D:\\a b");
        assert!(s.contains("'it''s mine'"));
        assert!(s.contains("'D:\\a b'"));
    }

    /// 0.1.0 的 parse error 就是这行写的 `(...($false))`，别再退回去
    #[test]
    fn script_uses_argument_list_not_paren_syntax() {
        let s = script_text(Path::new(r"C:\tmp\o.txt"), "d", "");
        assert!(s.contains("System.Text.UTF8Encoding -ArgumentList $false"));
        assert!(!s.contains("UTF8Encoding($false)"));
        assert!(s.contains(r"'C:\tmp\o.txt'"));
        // 没给初始目录时不该出现任何初始目录处理
        assert!(!s.contains("$init"));
    }

    #[test]
    fn script_includes_initial_dir_when_given() {
        let s = script_text(Path::new(r"C:\tmp\o.txt"), "d", "D:/Projects");
        // 正斜杠要先归一化成反斜杠，否则 FolderBrowserDialog 会悄悄忽略初始目录
        assert!(s.contains(r"$init = $init.Replace('/', '\')"));
        assert!(s.contains(r"if ([System.IO.Directory]::Exists($init)) { $d.SelectedPath = $init }"));
        assert!(s.contains(r"$init = 'D:/Projects'"));
    }
}
