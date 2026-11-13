//! Git 命令行封装。
//!
//! 设计取舍：不用 libgit2（`git2`），而是直接调用用户机器上的 `git.exe`。
//! 原因是本次要展示的正是「工作区 / 索引 / HEAD 三者」的差异，交给 git 自己算，
//! 行为天然和用户命令行里 `git status` 看到的一致（含 .gitignore、CRLF 转换、
//! attribute 过滤等），也免掉了在 Windows 上解析索引文件的坑。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::hide_console;

/// 单个文件的变更条目（对应 porcelain v2 的一行）
#[derive(Serialize, Clone, Debug)]
pub struct Entry {
    /// 仓库相对路径（正斜杠由前端统一转换，这里保留 git 原样）
    pub path: String,
    /// 重命名 / 复制时的源路径
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orig: Option<String>,
    /// 索引（暂存区）状态码，`.` 表示无变化
    pub x: String,
    /// 工作区状态码，`.` 表示无变化
    pub y: String,
    pub staged: bool,
    pub unstaged: bool,
    pub untracked: bool,
    pub conflicted: bool,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct RepoStatus {
    pub path: String,
    pub name: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub initial: bool,
    pub upstream: Option<String>,
    pub remotes: Vec<String>,
    pub ahead: u32,
    pub behind: u32,
    pub stashes: u32,
    pub head: Option<String>,
    pub head_subject: Option<String>,
    pub head_time: Option<String>,
    pub staged: u32,
    pub unstaged: u32,
    pub untracked: u32,
    pub conflicted: u32,
    pub entries: Vec<Entry>,
    pub warnings: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct GitInfo {
    pub available: bool,
    pub version: Option<String>,
    pub exe: Option<String>,
    pub hint: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct FileDiff {
    /// unified = 标准 diff 文本；text = 未跟踪文件的原文；binary / too_large / empty = 只有提示
    pub kind: String,
    pub text: Option<String>,
    pub truncated: bool,
    pub size: Option<u64>,
    pub message: Option<String>,
}

const MAX_DIFF_BYTES: u64 = 2 * 1024 * 1024;

static GIT_EXE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// 定位 git.exe：先试 PATH，再试常见安装位置。
fn locate_git() -> Option<PathBuf> {
    for cand in ["git.exe", "git"] {
        let mut cmd = Command::new(cand);
        cmd.arg("--version");
        hide_console(&mut cmd);
        if let Ok(out) = cmd.output() {
            if out.status.success() {
                return Some(PathBuf::from(cand));
            }
        }
    }
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Ok(pf) = std::env::var("ProgramFiles") {
        bases.push(PathBuf::from(pf).join("Git").join("cmd").join("git.exe"));
    }
    if let Ok(pf) = std::env::var("ProgramW6432") {
        bases.push(PathBuf::from(pf).join("Git").join("cmd").join("git.exe"));
    }
    if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
        bases.push(PathBuf::from(pf86).join("Git").join("cmd").join("git.exe"));
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        bases.push(PathBuf::from(local).join("Programs").join("Git").join("cmd").join("git.exe"));
    }
    for p in bases {
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

pub fn git_exe() -> Option<PathBuf> {
    GIT_EXE.get_or_init(locate_git).clone()
}

pub fn info() -> GitInfo {
    match git_exe() {
        Some(exe) => {
            let mut cmd = Command::new(&exe);
            cmd.arg("--version");
            hide_console(&mut cmd);
            match cmd.output() {
                Ok(out) if out.status.success() => {
                    let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    GitInfo {
                        available: true,
                        version: Some(version),
                        exe: Some(exe.display().to_string()),
                        hint: None,
                    }
                }
                Ok(out) => GitInfo {
                    available: false,
                    version: None,
                    exe: Some(exe.display().to_string()),
                    hint: Some(format!(
                        "git --version 执行失败：{}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )),
                },
                Err(e) => GitInfo {
                    available: false,
                    version: None,
                    exe: None,
                    hint: Some(format!("无法启动 git：{e}")),
                },
            }
        }
        None => GitInfo {
            available: false,
            version: None,
            exe: None,
            hint: Some(
                "未找到 git.exe。请安装 Git for Windows（https://git-scm.com/download/win），安装后重启本程序。"
                    .to_string(),
            ),
        },
    }
}

/// 统一入口：所有 git 调用都带 `--no-optional-locks`（只读扫描不去抢索引锁）、
/// `core.quotepath=false`（中文路径不被转义成八进制）。
fn run_git(exe: &Path, cwd: &Path, args: &[&str]) -> Result<Output, String> {
    run_git_timeout(exe, cwd, args, Duration::from_secs(60))
}

/// 带超时的 git 调用。
///
/// 必须自己管超时：`git status --untracked-files=all` 在巨型仓库（kibana 这种几万个文件）
/// 或者磁盘很慢的机器上能跑好几分钟，一个卡住的调用会把并发 worker 永久占住，
/// 界面上永远停在「正在读取 git status…」。
///
/// stdout / stderr 必须交给独立线程读走：管道缓冲区只有 64KB，输出一多子进程就会阻塞在写上，
/// 这时候 `try_wait` 永远返回 None，超时逻辑会把一个其实在正常干活的 git 误杀。
fn run_git_timeout(exe: &Path, cwd: &Path, args: &[&str], timeout: Duration) -> Result<Output, String> {
    let mut cmd = Command::new(exe);
    cmd.current_dir(cwd)
        .arg("--no-optional-locks")
        .arg("-c")
        .arg("core.quotepath=false")
        .arg("-c")
        .arg("color.ui=false")
        .args(args)
        // 任何交互式凭据提示都不该出现在扫描流程里
        .env("GIT_TERMINAL_PROMPT", "0")
        // 父进程里若带着这些变量（有人在 shell 里 export 过 GIT_DIR），
        // 命令会指向别的仓库；对 reset / commit 这种写操作就是真事故。
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_console(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("无法执行 {}：{e}", exe.display()))?;

    let mut out_pipe = child.stdout.take();
    let mut err_pipe = child.stderr.take();
    let out_thread = out_pipe.take().map(|mut p| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = p.read_to_end(&mut buf);
            buf
        })
    });
    let err_thread = err_pipe.take().map(|mut p| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = p.read_to_end(&mut buf);
            buf
        })
    });

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if started.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    break None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("等待 git 结束失败：{e}"));
            }
        }
    };

    let stdout = out_thread
        .and_then(|t| t.join().ok())
        .unwrap_or_default();
    let stderr = err_thread
        .and_then(|t| t.join().ok())
        .unwrap_or_default();

    if timed_out {
        let tail = String::from_utf8_lossy(&stderr);
        let tail = tail.trim();
        return Err(if tail.is_empty() {
            format!(
                "git {} 超过 {} 秒没返回，已放弃（仓库太大或磁盘太慢）",
                args.first().copied().unwrap_or("命令"),
                timeout.as_secs()
            )
        } else {
            format!(
                "git {} 超过 {} 秒没返回，已放弃：{tail}",
                args.first().copied().unwrap_or("命令"),
                timeout.as_secs()
            )
        });
    }

    Ok(Output {
        status: status.expect("timeout 已经返回"),
        stdout,
        stderr,
    })
}

fn stdout_text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// 跑一条只读查询命令，按行返回 stdout（已 trim、去掉空行）。
///
/// 只给「拿一份列表」的场景用（巡检要分支名、要某个引用是否存在），
/// 失败一律转成中文错误，不把裸 exit code 抛给调用方。
pub fn git_lines(repo: &Path, args: &[&str]) -> Result<Vec<String>, String> {
    let exe = git_exe().ok_or_else(|| {
        "未找到 git.exe，请先安装 Git for Windows 后重启本程序。".to_string()
    })?;
    let out = run_git(&exe, repo, args)?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let name = args.first().copied().unwrap_or("命令");
        return Err(if err.is_empty() {
            format!("git {name} 执行失败")
        } else {
            format!("git {name} 失败：{err}")
        });
    }
    Ok(stdout_text(&out)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// 解析 git 的 C 风格引号路径（`"a\tb"` / 八进制 UTF-8 字节）。
/// 开了 `core.quotepath=false` 后只有以空格 / 控制字符结尾等少数情况才会加引号。
fn unquote_path(raw: &str) -> String {
    let raw = raw.trim_end_matches('\r');
    if !(raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"')) {
        return raw.to_string();
    }
    let inner = &raw.as_bytes()[1..raw.len() - 1];
    let mut out: Vec<u8> = Vec::with_capacity(inner.len());
    let mut i = 0usize;
    while i < inner.len() {
        let b = inner[i];
        if b == b'\\' && i + 1 < inner.len() {
            let c = inner[i + 1];
            match c {
                b'n' => {
                    out.push(b'\n');
                    i += 2;
                }
                b't' => {
                    out.push(b'\t');
                    i += 2;
                }
                b'r' => {
                    out.push(b'\r');
                    i += 2;
                }
                b'"' => {
                    out.push(b'"');
                    i += 2;
                }
                b'\\' => {
                    out.push(b'\\');
                    i += 2;
                }
                b'a' => {
                    out.push(7);
                    i += 2;
                }
                b'b' => {
                    out.push(8);
                    i += 2;
                }
                b'f' => {
                    out.push(12);
                    i += 2;
                }
                b'v' => {
                    out.push(11);
                    i += 2;
                }
                b'0'..=b'7' => {
                    let mut val: u32 = 0;
                    let mut n = 0usize;
                    while n < 3 && i + 1 + n < inner.len() && (b'0'..=b'7').contains(&inner[i + 1 + n]) {
                        val = val * 8 + (inner[i + 1 + n] - b'0') as u32;
                        n += 1;
                    }
                    out.push((val & 0xFF) as u8);
                    i += 1 + n;
                }
                _ => {
                    out.push(c);
                    i += 2;
                }
            }
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_numstat(text: &str, map: &mut HashMap<String, (Option<u64>, Option<u64>)>) {
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let mut it = line.splitn(3, '\t');
        let add = it.next().unwrap_or("");
        let del = it.next().unwrap_or("");
        let path = match it.next() {
            Some(p) => p,
            None => continue,
        };
        let path = unquote_path(path);
        let e = map.entry(path).or_insert((None, None));
        e.0 = add.parse::<u64>().ok().or(e.0);
        e.1 = del.parse::<u64>().ok().or(e.1);
    }
}

/// 采集「相对 HEAD」的行数统计。空仓库（无 HEAD）退化为「索引 + 工作区」两段相加。
fn numstat_map(exe: &Path, repo: &Path) -> HashMap<String, (Option<u64>, Option<u64>)> {
    let mut map: HashMap<String, (Option<u64>, Option<u64>)> = HashMap::new();
    let base = ["diff", "--numstat", "--no-renames", "--no-ext-diff"];

    let head_args: Vec<&str> = base.iter().copied().chain(["HEAD"]).collect();
    if let Ok(out) = run_git(exe, repo, &head_args) {
        if out.status.success() {
            parse_numstat(&stdout_text(&out), &mut map);
            return map;
        }
    }
    for extra in [vec!["--cached"], vec![]] {
        let mut args: Vec<&str> = base.to_vec();
        args.extend(extra);
        if let Ok(out) = run_git(exe, repo, &args) {
            if out.status.success() {
                parse_numstat(&stdout_text(&out), &mut map);
            }
        }
    }
    map
}

fn head_meta(exe: &Path, repo: &Path) -> (Option<String>, Option<String>, Option<String>) {
    let args = ["log", "-1", "--pretty=format:%h%x09%s%x09%cr"];
    if let Ok(out) = run_git(exe, repo, &args) {
        if out.status.success() {
            let text = stdout_text(&out);
            let line = text.trim_end_matches('\n');
            if !line.is_empty() {
                let mut it = line.splitn(3, '\t');
                let sha = it.next().map(|s| s.to_string());
                let subject = it.next().map(|s| s.to_string());
                let time = it.next().map(|s| s.to_string());
                return (sha, subject, time);
            }
        }
    }
    (None, None, None)
}

/// 界面用的完整状态：带每个文件的行数统计。
pub fn status(repo: &Path) -> Result<RepoStatus, String> {
    status_impl(repo, true)
}

/// 巡检用的轻量状态：只要计数和文件清单，不要 `+N/-N`。
///
/// 行数统计是每个仓库额外一次 `git diff --numstat`，在几十个巨型仓库上
/// 加起来能占到巡检总耗时的一半，而巡检只关心「有没有改动」。
pub fn status_lite(repo: &Path) -> Result<RepoStatus, String> {
    status_impl(repo, false)
}

fn status_impl(repo: &Path, with_numstat: bool) -> Result<RepoStatus, String> {
    let exe = git_exe().ok_or_else(|| {
        "未找到 git.exe，请先安装 Git for Windows 后重启本程序。".to_string()
    })?;
    if !repo.is_dir() {
        return Err(format!("目录不存在：{}", repo.display()));
    }
    if !repo.join(".git").exists() {
        return Err(format!("不是 Git 仓库：{}", repo.display()));
    }

    let out = run_git(
        &exe,
        repo,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=all",
            "--show-stash",
        ],
    )?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "git status 执行失败".to_string()
        } else {
            format!("git status 失败：{err}")
        });
    }

    let text = stdout_text(&out);
    let mut warnings: Vec<String> = Vec::new();
    let mut entries: Vec<Entry> = Vec::new();
    let mut branch: Option<String> = None;
    let mut detached = false;
    let mut initial = false;
    let mut upstream: Option<String> = None;
    let mut ahead = 0u32;
    let mut behind = 0u32;
    let mut stashes = 0u32;
    let mut head: Option<String> = None;

    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("# ") {
            // 头信息：# branch.oid / # branch.head / # branch.upstream / # branch.ab / # stash
            if let Some(v) = rest.strip_prefix("branch.oid ") {
                if v.starts_with('(') {
                    initial = true;
                } else {
                    head = Some(v.to_string());
                }
            } else if let Some(v) = rest.strip_prefix("branch.head ") {
                if v == "(detached)" {
                    detached = true;
                } else {
                    branch = Some(v.to_string());
                }
            } else if let Some(v) = rest.strip_prefix("branch.upstream ") {
                upstream = Some(v.to_string());
            } else if let Some(v) = rest.strip_prefix("branch.ab ") {
                for tok in v.split_whitespace() {
                    if let Some(n) = tok.strip_prefix('+') {
                        ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = tok.strip_prefix('-') {
                        behind = n.parse().unwrap_or(0);
                    }
                }
            } else if let Some(v) = rest.strip_prefix("stash ") {
                stashes = v.trim().parse().unwrap_or(0);
            }
            continue;
        }

        let kind = line.as_bytes()[0];
        match kind {
            b'1' => {
                // 1 XY sub mH mI mW hH hI <path>
                let f: Vec<&str> = line.splitn(9, ' ').collect();
                if f.len() < 9 {
                    warnings.push(format!("无法解析状态行：{line}"));
                    continue;
                }
                let xy = f[1].as_bytes();
                entries.push(Entry {
                    path: unquote_path(f[8]),
                    orig: None,
                    x: (xy[0] as char).to_string(),
                    y: (xy[1] as char).to_string(),
                    staged: xy[0] != b'.',
                    unstaged: xy[1] != b'.',
                    untracked: false,
                    conflicted: false,
                    additions: None,
                    deletions: None,
                });
            }
            b'2' => {
                // 2 XY sub mH mI mW hH hI X<score> <path>\t<origPath>
                let f: Vec<&str> = line.splitn(10, ' ').collect();
                if f.len() < 10 {
                    warnings.push(format!("无法解析重命名行：{line}"));
                    continue;
                }
                let xy = f[1].as_bytes();
                let tail = f[9];
                let (new_p, old_p) = match tail.split_once('\t') {
                    Some((a, b)) => (a, Some(b.to_string())),
                    None => (tail, None),
                };
                entries.push(Entry {
                    path: unquote_path(new_p),
                    orig: old_p.map(|s| unquote_path(&s)),
                    x: (xy[0] as char).to_string(),
                    y: (xy[1] as char).to_string(),
                    staged: xy[0] != b'.',
                    unstaged: xy[1] != b'.',
                    untracked: false,
                    conflicted: false,
                    additions: None,
                    deletions: None,
                });
            }
            b'u' => {
                // u XY sub m1 m2 m3 mW h1 h2 h3 <path>
                let f: Vec<&str> = line.splitn(11, ' ').collect();
                if f.len() < 11 {
                    warnings.push(format!("无法解析冲突行：{line}"));
                    continue;
                }
                let xy = f[1].as_bytes();
                entries.push(Entry {
                    path: unquote_path(f[10]),
                    orig: None,
                    x: (xy[0] as char).to_string(),
                    y: (xy[1] as char).to_string(),
                    staged: false,
                    unstaged: false,
                    untracked: false,
                    conflicted: true,
                    additions: None,
                    deletions: None,
                });
            }
            b'?' => {
                let p = line.strip_prefix("? ").unwrap_or(line);
                entries.push(Entry {
                    path: unquote_path(p),
                    orig: None,
                    x: "?".to_string(),
                    y: "?".to_string(),
                    staged: false,
                    unstaged: false,
                    untracked: true,
                    conflicted: false,
                    additions: None,
                    deletions: None,
                });
            }
            // '!' 被忽略（未加 --ignored），其余未知类型直接跳过
            _ => {}
        }
    }

    // 行数统计：一个额外 git 调用换来列表里的 +N/-N。
    // 巡检走 status_lite，跳过这一段。
    if with_numstat {
        let numstat = numstat_map(&exe, repo);
        for e in entries.iter_mut() {
            if let Some((add, del)) = numstat.get(&e.path) {
                e.additions = *add;
                e.deletions = *del;
            }
        }
    }

    let (log_sha, subject, rel_time) = if initial {
        (None, None, None)
    } else {
        head_meta(&exe, repo)
    };

    // 远程列表：让界面能提前告诉用户「这个仓库没配远程，推不了」
    let remotes = run_git(&exe, repo, &["remote"])
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    entries.sort_by(|a, b| a.path.to_lowercase().cmp(&b.path.to_lowercase()));

    let staged = entries.iter().filter(|e| e.staged).count() as u32;
    let unstaged = entries.iter().filter(|e| e.unstaged).count() as u32;
    let untracked = entries.iter().filter(|e| e.untracked).count() as u32;
    let conflicted = entries.iter().filter(|e| e.conflicted).count() as u32;

    Ok(RepoStatus {
        path: repo.display().to_string(),
        name: repo
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| repo.display().to_string()),
        branch,
        detached,
        initial,
        upstream,
        remotes,
        ahead,
        behind,
        stashes,
        head: head.or(log_sha),
        head_subject: subject,
        head_time: rel_time,
        staged,
        unstaged,
        untracked,
        conflicted,
        entries,
        warnings,
    })
}

fn looks_binary(bytes: &[u8]) -> bool {
    let probe = &bytes[..bytes.len().min(8000)];
    probe.contains(&0u8)
}

/// 取单个文件的差异。
///
/// * `staged = true` → `git diff --cached`（索引 vs HEAD）
/// * `staged = false` → `git diff`（工作区 vs 索引）
/// * `untracked = true` → 直接读文件原文（git 对未跟踪文件没有 diff），由前端渲染成「全绿」
pub fn file_diff(repo: &Path, file: &str, staged: bool, untracked: bool) -> Result<FileDiff, String> {
    let exe = git_exe()
        .ok_or_else(|| "未找到 git.exe，请先安装 Git for Windows 后重启本程序。".to_string())?;
    if !repo.is_dir() {
        return Err(format!("目录不存在：{}", repo.display()));
    }
    // 未跟踪文件是直接 fs::read 的，不校验就能用 `../../x` 读到仓库外的文件；
    // 提交那条路径本来就有这层校验，diff 这边漏了，补齐。
    check_rel_path(file)?;
    let full = repo.join(file);
    if !full.starts_with(repo) {
        return Err("非法路径".to_string());
    }

    if untracked {
        let meta = std::fs::metadata(&full).map_err(|e| format!("读取文件失败：{e}"))?;
        let size = meta.len();
        if !meta.is_file() {
            return Ok(FileDiff {
                kind: "empty".into(),
                text: None,
                truncated: false,
                size: Some(size),
                message: Some("这是目录 / 子模块，没有可预览的文本内容".into()),
            });
        }
        if size > MAX_DIFF_BYTES {
            return Ok(FileDiff {
                kind: "too_large".into(),
                text: None,
                truncated: true,
                size: Some(size),
                message: Some(format!(
                    "文件过大（{}），未加载预览",
                    human_size(size)
                )),
            });
        }
        let bytes = std::fs::read(&full).map_err(|e| format!("读取文件失败：{e}"))?;
        if looks_binary(&bytes) {
            return Ok(FileDiff {
                kind: "binary".into(),
                text: None,
                truncated: false,
                size: Some(size),
                message: Some("这个文件是二进制，没法按文本预览。".into()),
            });
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        return Ok(FileDiff {
            kind: "text".into(),
            text: Some(text),
            truncated: false,
            size: Some(size),
            message: None,
        });
    }

    let mut args: Vec<&str> = vec!["diff", "--no-ext-diff", "--no-color", "--no-renames", "-U3"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    args.push(file);

    let out = run_git_timeout(&exe, repo, &args, Duration::from_secs(30))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "git diff 执行失败".to_string()
        } else {
            format!("git diff 失败：{err}")
        });
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if text.trim().is_empty() {
        return Ok(FileDiff {
            kind: "empty".into(),
            text: None,
            truncated: false,
            size: None,
            message: Some("该侧没有差异（可能只改了文件模式，或内容与另一侧一致）".into()),
        });
    }
    if text.contains("Binary files ") || text.contains("GIT binary patch") {
        return Ok(FileDiff {
            kind: "binary".into(),
            text: Some(text),
            truncated: false,
            size: None,
            message: Some("git 不给二进制文件出文本差异，只能看出它被改过。".into()),
        });
    }
    Ok(FileDiff {
        kind: "unified".into(),
        text: Some(text),
        truncated: false,
        size: None,
        message: None,
    })
}

/* ---------------------------------------------------------------- 分支树 / 提交补丁 */

#[derive(Serialize, Clone, Debug)]
pub struct GraphCommit {
    pub hash: String,
    pub short: String,
    pub parents: Vec<String>,
    pub author: String,
    pub date: String,
    /// `%D` 的原始装饰串，例如 `HEAD -> main, origin/main, tag: v1.0`
    pub refs: String,
    pub subject: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct RepoGraph {
    pub branch: Option<String>,
    pub commits: Vec<GraphCommit>,
    pub truncated: bool,
}

/// 只接受十六进制的提交号，避免把 `--foo` 这种东西喂给 git。
fn is_hex_ref(hash: &str) -> bool {
    let h = hash.trim();
    (4..=64).contains(&h.len()) && h.chars().all(|c| c.is_ascii_hexdigit())
}

/// 分支树的原始数据：给前端父提交 hash，车道布局在前端算（要跟着缩放 / 换行重排）。
///
/// `--all` 才能看到所有分支；`--date-order` 比默认的拓扑序更接近图形化工具的样子。
pub fn graph(repo: &Path, limit: usize) -> Result<RepoGraph, String> {
    let exe = git_exe()
        .ok_or_else(|| "未找到 git.exe，请先安装 Git for Windows 后重启本程序。".to_string())?;
    if !repo.is_dir() {
        return Err(format!("目录不存在：{}", repo.display()));
    }
    let n = limit.clamp(1, 2000);
    let take = (n + 1).to_string();
    // 记录之间用 ASCII 的 0x1e 分隔、字段之间用 0x1f，避免提交信息里的空白串掉字段
    let pretty = "--pretty=format:%H\x1f%h\x1f%P\x1f%an\x1f%ad\x1f%D\x1f%s\x1e".to_string();
    let args: Vec<&str> = vec![
        "log",
        "--all",
        "--date-order",
        "--no-color",
        "--date=short",
        "-n",
        &take,
        &pretty,
    ];
    let out = run_git_timeout(&exe, repo, &args, Duration::from_secs(30))?;
    let branch = current_branch(&exe, repo);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() && stdout.trim().is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("does not have any commits yet") {
            return Ok(RepoGraph {
                branch,
                commits: Vec::new(),
                truncated: false,
            });
        }
        return Err(format!("git log 失败：{}", err.trim()));
    }
    let mut commits = Vec::new();
    for rec in stdout.split('\u{1e}') {
        let rec = rec.trim_matches(|c| c == '\n' || c == '\r');
        if rec.is_empty() {
            continue;
        }
        let f: Vec<&str> = rec.split('\u{1f}').collect();
        if f.len() < 7 {
            continue;
        }
        commits.push(GraphCommit {
            hash: f[0].to_string(),
            short: f[1].to_string(),
            parents: f[2].split_whitespace().map(str::to_string).collect(),
            author: f[3].to_string(),
            date: f[4].to_string(),
            refs: f[5].trim().to_string(),
            subject: f[6].to_string(),
        });
    }
    let truncated = commits.len() > n;
    if truncated {
        commits.truncate(n);
    }
    Ok(RepoGraph {
        branch,
        commits,
        truncated,
    })
}

/// 单个提交的补丁（合并提交按第一父展开；不显示 commit 头，和文件 diff 一个渲染器）。
pub fn commit_diff(repo: &Path, hash: &str) -> Result<FileDiff, String> {
    let exe = git_exe()
        .ok_or_else(|| "未找到 git.exe，请先安装 Git for Windows 后重启本程序。".to_string())?;
    if !repo.is_dir() {
        return Err(format!("目录不存在：{}", repo.display()));
    }
    if !is_hex_ref(hash) {
        return Err("提交号不合法".to_string());
    }
    let h = hash.trim().to_string();
    let args: Vec<&str> = vec![
        "show",
        "--no-color",
        "--no-ext-diff",
        "--no-renames",
        "--format=",
        "--diff-merges=first-parent",
        "-U3",
        &h,
    ];
    let out = run_git_timeout(&exe, repo, &args, Duration::from_secs(30))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "git show 执行失败".to_string()
        } else {
            format!("git show 失败：{err}")
        });
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    if text.trim().is_empty() {
        return Ok(FileDiff {
            kind: "empty".into(),
            text: None,
            truncated: false,
            size: None,
            message: Some("这个提交没有内容变化".into()),
        });
    }
    if text.contains("Binary files ") || text.contains("GIT binary patch") {
        return Ok(FileDiff {
            kind: "binary".into(),
            text: Some(text),
            truncated: false,
            size: None,
            message: Some("这个提交里有二进制文件".into()),
        });
    }
    Ok(FileDiff {
        kind: "unified".into(),
        text: Some(text),
        truncated: false,
        size: None,
        message: None,
    })
}

/* ---------------------------------------------------------------- 写操作：提交 / 推送
 *
 * 这是整个程序里唯一会改动仓库的两条路径。为了不误伤，做了三层限制：
 *   1. 只接受仓库内的相对路径（绝对路径、`..`、盘符一律拒绝）；
 *   2. 提交一律走 pathspec（`git commit -- <paths>`），只提交被选中的那几个路径，
 *      不会顺带把别处已经 staged 的内容一并提交；
 *   3. 提交信息为空直接拒绝，绝不自动生成。
 */

#[derive(Serialize, Clone, Debug)]
pub struct CommitResult {
    pub hash: String,
    pub subject: String,
    pub branch: Option<String>,
    pub files: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct PushResult {
    pub branch: String,
    pub remote: Option<String>,
    pub upstream: Option<String>,
    pub set_upstream: bool,
    pub output: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct PullResult {
    pub branch: String,
    pub remote: Option<String>,
    pub upstream: Option<String>,
    pub summary: String,
    pub output: String,
}

fn stderr_text(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_string()
}

/// 拒绝绝对路径、盘符路径、`..` 上跳、NUL 字节。
fn check_rel_path(file: &str) -> Result<(), String> {
    let f = file.trim();
    if f.is_empty() {
        return Err("文件路径为空".to_string());
    }
    if f.contains('\0') {
        return Err("文件路径含非法字符".to_string());
    }
    if f.starts_with('/') || f.starts_with('\\') {
        return Err(format!("只接受仓库内的相对路径，收到绝对路径：{f}"));
    }
    let bytes = f.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        return Err(format!("只接受仓库内的相对路径，收到盘符路径：{f}"));
    }
    if f.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(format!("路径不能跳出仓库：{f}"));
    }
    Ok(())
}

fn rev_parse_short(exe: &Path, repo: &Path) -> Option<String> {
    let out = run_git(exe, repo, &["rev-parse", "--short", "HEAD"]).ok()?;
    if !out.status.success() {
        return None;
    }
    let s = stdout_text(&out).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn current_branch(exe: &Path, repo: &Path) -> Option<String> {
    let out = run_git(exe, repo, &["symbolic-ref", "--short", "HEAD"]).ok()?;
    if !out.status.success() {
        return None;
    }
    let s = stdout_text(&out).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// 提交选中的若干文件（相对仓库根、正斜杠）。返回新提交的短哈希与标题。
pub fn commit(repo: &Path, files: &[String], message: &str) -> Result<CommitResult, String> {
    let exe = git_exe().ok_or_else(|| {
        "未找到 git.exe，请先安装 Git for Windows 后重启本程序。".to_string()
    })?;
    if !repo.join(".git").exists() {
        return Err(format!("不是 Git 仓库：{}", repo.display()));
    }
    let msg = message.trim();
    if msg.is_empty() {
        return Err("提交信息不能为空".to_string());
    }
    if files.is_empty() {
        return Err("没有选中任何文件".to_string());
    }
    for f in files {
        check_rel_path(f)?;
    }

    // 1) 先把选中的路径放进暂存区（-A 让「删除」也一起进去；未跟踪文件也在这里入库）
    let mut args: Vec<&str> = vec!["add", "-A", "--"];
    args.extend(files.iter().map(|s| s.as_str()));
    let out = run_git_timeout(&exe, repo, &args, Duration::from_secs(120))?;
    if !out.status.success() {
        let err = stderr_text(&out);
        return Err(if err.is_empty() {
            "git add 失败".to_string()
        } else {
            format!("git add 失败：{err}")
        });
    }

    // 2) 只提交这些路径。带 pathspec 的 commit 会为它们单独建临时索引，
    //    所以别人已经 staged 的内容不会被裹进来。
    let mut args: Vec<&str> = vec!["commit", "-m", msg, "--"];
    args.extend(files.iter().map(|s| s.as_str()));
    let out = run_git_timeout(&exe, repo, &args, Duration::from_secs(120))?;
    if !out.status.success() {
        let err = stderr_text(&out);
        return Err(if err.is_empty() {
            "git commit 失败".to_string()
        } else {
            format!("git commit 失败：{err}")
        });
    }

    let hash = rev_parse_short(&exe, repo).unwrap_or_default();
    let subject = run_git(&exe, repo, &["log", "-1", "--pretty=%s"])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| stdout_text(&o).trim().to_string())
        .unwrap_or_else(|| msg.lines().next().unwrap_or("").to_string());

    Ok(CommitResult {
        hash,
        subject,
        branch: current_branch(&exe, repo),
        files: files.len(),
    })
}

/// 推送当前分支。没有 upstream 时自动 `git push -u <remote> <branch>`（origin 优先）。
pub fn push(repo: &Path) -> Result<PushResult, String> {
    let exe = git_exe().ok_or_else(|| {
        "未找到 git.exe，请先安装 Git for Windows 后重启本程序。".to_string()
    })?;
    if !repo.join(".git").exists() {
        return Err(format!("不是 Git 仓库：{}", repo.display()));
    }

    let branch = current_branch(&exe, repo).ok_or_else(|| {
        "当前处于 detached HEAD（不在任何分支上），先在命令行切回分支再推送".to_string()
    })?;

    let remotes_out = run_git(&exe, repo, &["remote"])?;
    let remotes: Vec<String> = stdout_text(&remotes_out)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if remotes.is_empty() {
        return Err("这个仓库没有配置远程仓库（git remote 为空），没法推送".to_string());
    }

    let upstream = run_git(
        &exe,
        repo,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    )
    .ok()
    .filter(|o| o.status.success())
    .map(|o| stdout_text(&o).trim().to_string())
    .filter(|s| !s.is_empty());

    let mut args: Vec<String> = vec!["push".to_string()];
    let (remote_used, set_upstream) = match &upstream {
        Some(_) => (None, false),
        None => {
            let remote = if remotes.iter().any(|r| r == "origin") {
                "origin".to_string()
            } else {
                remotes[0].clone()
            };
            args.push("-u".to_string());
            args.push(remote.clone());
            args.push(branch.clone());
            (Some(remote), true)
        }
    };

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = run_git_timeout(&exe, repo, &arg_refs, Duration::from_secs(300))?;
    if !out.status.success() {
        let err = stderr_text(&out);
        let mut hint = String::new();
        let low = err.to_lowercase();
        if low.contains("authentication") || low.contains("could not read username") || low.contains("permission denied") {
            hint = "\n（看起来是凭据问题：先在命令行里 git push 一次把账号密码存下来，再回来重试）".to_string();
        } else if low.contains("non-fast-forward") || low.contains("fetch first") || low.contains("rejected") {
            hint = "\n（远程有新提交，本地落后了。先 git pull 或 git fetch 再推送）".to_string();
        } else if low.contains("no configured push destination") {
            hint = "\n（这个分支没有配置上游，指定远程后再推：git push -u origin <branch>）".to_string();
        }
        return Err(if err.is_empty() {
            "git push 失败（没有任何输出）".to_string()
        } else {
            format!("git push 失败：{err}{hint}")
        });
    }

    let mut output = stdout_text(&out);
    let err = stderr_text(&out);
    if !err.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&err);
    }

    Ok(PushResult {
        branch,
        remote: remote_used,
        upstream,
        set_upstream,
        output: output.trim().to_string(),
    })
}

/// 拉取的摘要：优先识别几种常见结果，识别不了就给 git 的最后一行。
fn describe_pull(text: &str) -> String {
    let low = text.to_lowercase();
    if low.contains("already up to date") || low.contains("already up-to-date") {
        return "已是最新".to_string();
    }
    if low.contains("fast-forward") {
        return "已快进".to_string();
    }
    if low.contains("merge made by") {
        return "已合并远程提交".to_string();
    }
    if low.contains("successfully rebased") || low.contains("fast-forwarded") {
        return "已变基到远程".to_string();
    }
    text.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .next_back()
        .unwrap_or("完成")
        .to_string()
}

/// 拉取当前分支。有上游直接 `git pull --no-edit`；没设上游时显式指定远程与分支。
///
/// `--no-edit` 是必须的：需要生成合并提交时，`git pull` 默认要拉起编辑器，
/// 而这里起的子进程没有终端，会一直等到超时。
pub fn pull(repo: &Path) -> Result<PullResult, String> {
    let exe = git_exe().ok_or_else(|| {
        "未找到 git.exe，请先安装 Git for Windows 后重启本程序。".to_string()
    })?;
    if !repo.join(".git").exists() {
        return Err(format!("不是 Git 仓库：{}", repo.display()));
    }

    let branch = current_branch(&exe, repo).ok_or_else(|| {
        "当前处于 detached HEAD（不在任何分支上），先在命令行切回分支再拉取".to_string()
    })?;

    let remotes_out = run_git(&exe, repo, &["remote"])?;
    let remotes: Vec<String> = stdout_text(&remotes_out)
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    if remotes.is_empty() {
        return Err("这个仓库没有配置远程仓库（git remote 为空），没法拉取".to_string());
    }

    let upstream = run_git(
        &exe,
        repo,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"],
    )
    .ok()
    .filter(|o| o.status.success())
    .map(|o| stdout_text(&o).trim().to_string())
    .filter(|s| !s.is_empty());

    let mut args: Vec<String> = vec!["pull".to_string(), "--no-edit".to_string()];
    let remote_used = match &upstream {
        Some(_) => None,
        None => {
            let remote = if remotes.iter().any(|r| r == "origin") {
                "origin".to_string()
            } else {
                remotes[0].clone()
            };
            args.push(remote.clone());
            args.push(branch.clone());
            Some(remote)
        }
    };

    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let out = run_git_timeout(&exe, repo, &arg_refs, Duration::from_secs(300))?;

    let mut output = stdout_text(&out);
    let err = stderr_text(&out);
    if !err.is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&err);
    }
    let combined = output.trim().to_string();

    if !out.status.success() {
        let low = combined.to_lowercase();
        let mut hint = String::new();
        if low.contains("authentication")
            || low.contains("could not read username")
            || low.contains("permission denied")
        {
            hint = "\n（看起来是凭据问题：先在命令行里 git pull 一次把账号密码存下来，再回来重试）".to_string();
        } else if low.contains("conflict") || low.contains("automatic merge failed") {
            hint = "\n（有冲突，需要到命令行里解决后再提交）".to_string();
        } else if low.contains("would be overwritten by merge") || low.contains("please commit your changes or stash") {
            hint = "\n（本地有未提交的改动会被覆盖，先提交或 stash 再拉取）".to_string();
        } else if low.contains("no tracking information") {
            hint = "\n（这个分支没有上游，先 git branch --set-upstream-to=<remote>/<branch>）".to_string();
        } else if low.contains("refusing to merge unrelated histories") {
            hint = "\n（本地与远程没有共同祖先，需要用 git pull --allow-unrelated-histories）".to_string();
        }
        return Err(if combined.is_empty() {
            "git pull 失败（没有任何输出）".to_string()
        } else {
            format!("git pull 失败：{combined}{hint}")
        });
    }

    Ok(PullResult {
        branch,
        remote: remote_used,
        upstream,
        summary: describe_pull(&combined),
        output: combined,
    })
}

/* ---------------------------------------------------------------- 回滚（reset） */

/// 回滚前的体检结果，用来在确认框里把后果讲清楚。
#[derive(Serialize, Clone, Debug)]
pub struct ResetPlan {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub branch: Option<String>,
    pub detached: bool,
    pub head: Option<String>,
    /// 从当前 HEAD 退到目标提交会撤销掉的提交数
    pub drop_commits: usize,
    /// 目标提交里有、当前 HEAD 没有的提交数（回滚到别的分支的提交时大于 0）
    pub gain_commits: usize,
    /// 工作区 + 暂存区里未提交的条目数（硬回滚会丢掉其中已跟踪文件的改动）
    pub dirty: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct ResetResult {
    pub hash: String,
    pub short: String,
    pub subject: String,
    pub branch: Option<String>,
    pub mode: String,
    pub drop_commits: usize,
    pub dirty_dropped: usize,
}

fn check_reset_mode(mode: &str) -> Result<&'static str, String> {
    match mode {
        "soft" => Ok("soft"),
        "mixed" => Ok("mixed"),
        "hard" => Ok("hard"),
        other => Err(format!("不认识的回滚方式：{other}（只能是 soft / mixed / hard）")),
    }
}

/// 把任意 commit 表达式解析成完整哈希；解析不出来就是「没有这个提交」。
fn rev_parse_commit(exe: &Path, repo: &Path, rev: &str) -> Option<String> {
    let spec = format!("{rev}^{{commit}}");
    let out = run_git(exe, repo, &["rev-parse", "--verify", "--quiet", &spec]).ok()?;
    if !out.status.success() {
        return None;
    }
    let s = stdout_text(&out).trim().to_string();
    if s.len() == 40 || s.len() == 64 {
        Some(s)
    } else {
        None
    }
}

fn count_commits(exe: &Path, repo: &Path, range: &str) -> usize {
    run_git(exe, repo, &["rev-list", "--count", range])
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| stdout_text(&o).trim().parse::<usize>().ok())
        .unwrap_or(0)
}

fn dirty_count(exe: &Path, repo: &Path) -> usize {
    run_git(
        exe,
        repo,
        &["status", "--porcelain", "--untracked-files=all"],
    )
    .ok()
    .filter(|o| o.status.success())
    .map(|o| {
        stdout_text(&o)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count()
    })
    .unwrap_or(0)
}

/// 只读：算出「回滚到某个提交会发生什么」，不动仓库。
pub fn reset_plan(repo: &Path, hash: &str) -> Result<ResetPlan, String> {
    let exe = git_exe().ok_or_else(|| {
        "未找到 git.exe，请先安装 Git for Windows 后重启本程序。".to_string()
    })?;
    if !repo.join(".git").exists() {
        return Err(format!("不是 Git 仓库：{}", repo.display()));
    }
    let full = rev_parse_commit(&exe, repo, hash.trim())
        .ok_or_else(|| format!("找不到这个提交：{hash}"))?;
    let short = run_git(&exe, repo, &["rev-parse", "--short", &full])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| stdout_text(&o).trim().to_string())
        .unwrap_or_else(|| full[..7.min(full.len())].to_string());
    let subject = run_git(&exe, repo, &["log", "-1", "--pretty=format:%s", &full])
        .ok()
        .filter(|o| o.status.success())
        .map(|o| stdout_text(&o).trim().to_string())
        .unwrap_or_default();
    let branch = current_branch(&exe, repo);
    let detached = branch.is_none();
    let head = rev_parse_short(&exe, repo);

    Ok(ResetPlan {
        hash: full.clone(),
        short,
        subject,
        branch,
        detached,
        head,
        drop_commits: count_commits(&exe, repo, &format!("{full}..HEAD")),
        gain_commits: count_commits(&exe, repo, &format!("HEAD..{full}")),
        dirty: dirty_count(&exe, repo),
    })
}

/// 把当前分支（或 detached HEAD）退到指定提交。
/// mode: soft（改动留在暂存区）/ mixed（改动留在工作区、不暂存）/ hard（丢掉已跟踪文件的未提交改动）。
/// 注意：hard 不会删除未跟踪文件；被撤销的提交只留在 reflog 里，本程序不提供撤销。
pub fn reset_to(repo: &Path, hash: &str, mode: &str) -> Result<ResetResult, String> {
    let mode = check_reset_mode(mode)?;
    let plan = reset_plan(repo, hash)?;
    let exe = git_exe().ok_or_else(|| {
        "未找到 git.exe，请先安装 Git for Windows 后重启本程序。".to_string()
    })?;

    let arg = format!("--{mode}");
    let out = run_git_timeout(
        &exe,
        repo,
        &["reset", &arg, &plan.hash],
        Duration::from_secs(120),
    )?;
    if !out.status.success() {
        let err = stderr_text(&out);
        let msg = if err.is_empty() { "（没有任何输出）".to_string() } else { err };
        return Err(format!("git reset {arg} 失败：{msg}"));
    }

    Ok(ResetResult {
        hash: plan.hash,
        short: plan.short,
        subject: plan.subject,
        branch: plan.branch,
        mode: mode.to_string(),
        drop_commits: plan.drop_commits,
        dirty_dropped: if mode == "hard" { plan.dirty } else { 0 },
    })
}

pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < KB * KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / (KB * KB))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_plain() {
        assert_eq!(unquote_path("src/中文 空格.txt"), "src/中文 空格.txt");
    }

    #[test]
    fn unquote_quoted() {
        assert_eq!(unquote_path("\"a\\tb\""), "a\tb");
        assert_eq!(unquote_path("\"a\\\"b\""), "a\"b");
    }

    #[test]
    fn numstat_parse() {
        let mut m: HashMap<String, (Option<u64>, Option<u64>)> = HashMap::new();
        parse_numstat("3\t1\tsrc/a.rs\n-\t-\tlogo.png\n", &mut m);
        assert_eq!(m["src/a.rs"], (Some(3), Some(1)));
        assert_eq!(m["logo.png"], (None, None));
    }

    #[test]
    fn pull_summary_picks_known_results() {
        assert_eq!(describe_pull("Already up to date.\n"), "已是最新");
        assert_eq!(describe_pull("Updating a1b2c3..d4e5f6\nFast-forward\n app.go | 2 +-"), "已快进");
        assert_eq!(describe_pull("Merge made by the 'ort' strategy."), "已合并远程提交");
        assert_eq!(describe_pull("line one\n  last line  \n\n"), "last line");
    }

    #[test]
    fn rel_path_accepts_normal_paths() {
        assert!(check_rel_path("src/a.rs").is_ok());
        assert!(check_rel_path("docs/接口说明.md").is_ok());
        assert!(check_rel_path("a").is_ok());
    }

    #[test]
    fn rel_path_rejects_escapes() {
        assert!(check_rel_path("").is_err());
        assert!(check_rel_path("/etc/passwd").is_err());
        assert!(check_rel_path("D:/Projects/x").is_err());
        assert!(check_rel_path("../outside.txt").is_err());
        assert!(check_rel_path("src/../../x").is_err());
    }

    #[test]
    fn reset_mode_only_accepts_three_names() {
        assert_eq!(check_reset_mode("soft").unwrap(), "soft");
        assert_eq!(check_reset_mode("mixed").unwrap(), "mixed");
        assert_eq!(check_reset_mode("hard").unwrap(), "hard");
        assert!(check_reset_mode("").is_err());
        assert!(check_reset_mode("keep").is_err());
        assert!(check_reset_mode("HARD").is_err());
    }
}
