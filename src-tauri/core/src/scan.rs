//! 目录扫描：从用户指定的根目录自上而下找 `.git`。

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;

/// 不进入的目录名（小写比较）。都是"里面几乎不可能再放一个独立仓库"的大目录，
/// 跳过它们能把 node_modules / target / .venv 这类的扫描时间砍掉一个数量级。
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".svn",
    ".hg",
    "node_modules",
    "bower_components",
    "target",
    "dist",
    "build",
    "out",
    "obj",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".venv",
    "venv",
    "site-packages",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".tox",
    ".gradle",
    ".m2",
    ".cargo",
    ".rustup",
    ".idea",
    ".vscode",
    ".cache",
    ".turbo",
    ".parcel-cache",
    "coverage",
    "vendor",
];

const MAX_ERRORS: usize = 20;

#[derive(Serialize, Clone, Debug)]
pub struct RepoRef {
    pub path: String,
    pub name: String,
    /// 相对根目录的路径，根目录本身是仓库时为空串
    pub rel: String,
    /// repo = 普通仓库；worktree = .git 是文件（worktree / submodule / 软链接克隆）
    pub kind: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ScanResult {
    pub root: String,
    pub repos: Vec<RepoRef>,
    pub dirs: usize,
    pub errors: Vec<String>,
    pub truncated: bool,
    pub elapsed_ms: u64,
}

fn is_repo_marker(dir: &Path) -> Option<&'static str> {
    let marker = dir.join(".git");
    match std::fs::metadata(&marker) {
        Ok(m) if m.is_dir() => Some("repo"),
        Ok(m) if m.is_file() => Some("worktree"),
        _ => None,
    }
}

/// 广度优先遍历。`max_depth = 1` 只看根目录的直接子目录。
pub fn scan(root: &Path, max_depth: u32, limit: usize) -> Result<ScanResult, String> {
    let started = Instant::now();
    if !root.exists() {
        return Err(format!("目录不存在：{}", root.display()));
    }
    if !root.is_dir() {
        return Err(format!("不是一个目录：{}", root.display()));
    }

    let mut repos: Vec<RepoRef> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut dirs = 0usize;
    let mut truncated = false;
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    if let Some(kind) = is_repo_marker(root) {
        repos.push(RepoRef {
            path: root.display().to_string(),
            name: root
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string()),
            rel: String::new(),
            kind: kind.to_string(),
        });
    }

    let mut queue: VecDeque<(PathBuf, u32)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));

    while let Some((dir, depth)) = queue.pop_front() {
        if depth >= max_depth {
            continue;
        }
        dirs += 1;
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) => {
                if errors.len() < MAX_ERRORS {
                    errors.push(format!("{}：{}", dir.display(), e));
                }
                continue;
            }
        };
        for entry in rd.flatten() {
            let path = entry.path();
            // 只关心目录；文件不产生仓库
            let ft = match entry.file_type() {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            if !ft.is_dir() {
                continue;
            }
            // 跳过 junction / 符号链接，避免环路与跨盘遍历
            if ft.is_symlink() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.starts_with('.') {
                continue;
            }
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            let canon = match std::fs::canonicalize(&path) {
                Ok(c) => c,
                Err(_) => path.clone(),
            };
            if !seen.insert(canon) {
                continue;
            }

            if let Some(kind) = is_repo_marker(&path) {
                if repos.len() >= limit {
                    truncated = true;
                    continue;
                }
                let rel = path
                    .strip_prefix(root)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                repos.push(RepoRef {
                    path: path.display().to_string(),
                    name: entry.file_name().to_string_lossy().into_owned(),
                    rel,
                    kind: kind.to_string(),
                });
            }

            // 甚至找到了仓库也继续往下走：子模块就是"仓库里套仓库"
            queue.push_back((path, depth + 1));
        }
    }

    repos.sort_by(|a, b| a.rel.to_lowercase().cmp(&b.rel.to_lowercase()));

    Ok(ScanResult {
        root: root.display().to_string(),
        repos,
        dirs,
        errors,
        truncated,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}
