//! Git Tracer —— 扫描一个目录下所有 Git 仓库的待提交内容。
//!
//! 后端只做三件事：遍历找仓库、跑 `git status` / `git diff`、把结果整理成
//! 前端友好的结构。所有命令都是 `async` + `spawn_blocking`：
//! 同步命令会被内联在 IPC 线程（Windows 上就是 UI 主线程）执行，扫描几百个
//! 目录会直接把窗口卡死。

pub mod audit;
pub mod config;
mod dialog;
mod git;
mod scan;

use std::process::Command;

use tauri::Manager;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 让子进程（git / powershell / explorer）不要弹控制台窗口。
/// GUI 进程启动控制台程序时 Windows 默认会给它新建一个黑窗口。
#[cfg(windows)]
pub(crate) fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
pub(crate) fn hide_console(_cmd: &mut Command) {}

/// 把阻塞活儿丢到线程池，避免堵住异步运行时
async fn blocking<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| format!("后台任务异常：{e}"))?
}

#[tauri::command]
async fn git_info() -> git::GitInfo {
    tauri::async_runtime::spawn_blocking(git::info)
        .await
        .unwrap_or(git::GitInfo {
            available: false,
            version: None,
            exe: None,
            hint: Some("读取 git 信息时出错".to_string()),
        })
}

/// 扫描根目录下的所有仓库（只找 `.git`，不跑 git 命令，所以很快）
#[tauri::command]
async fn scan_repos(root: String, depth: u32, limit: Option<usize>) -> Result<scan::ScanResult, String> {
    let depth = depth.clamp(1, 16);
    let limit = limit.unwrap_or(800).clamp(1, 5000);
    let root_buf = std::path::PathBuf::from(root.trim());
    if root_buf.as_os_str().is_empty() {
        return Err("请先选择一个目录".to_string());
    }
    blocking(move || scan::scan(&root_buf, depth, limit)).await
}

/// 单个仓库的待提交内容
#[tauri::command]
async fn repo_status(path: String) -> Result<git::RepoStatus, String> {
    let p = std::path::PathBuf::from(path);
    blocking(move || git::status(&p)).await
}

/// 单文件差异
#[tauri::command]
async fn file_diff(
    path: String,
    file: String,
    staged: bool,
    untracked: bool,
) -> Result<git::FileDiff, String> {
    let repo = std::path::PathBuf::from(path);
    blocking(move || git::file_diff(&repo, &file, staged, untracked)).await
}

/// 分支树（所有分支的提交 + 父提交），布局在前端算
#[tauri::command]
async fn repo_graph(path: String, limit: Option<usize>) -> Result<git::RepoGraph, String> {
    let repo = std::path::PathBuf::from(path);
    let limit = limit.unwrap_or(300).clamp(1, 2000);
    blocking(move || git::graph(&repo, limit)).await
}

/// 单个提交的补丁
#[tauri::command]
async fn commit_diff(path: String, hash: String) -> Result<git::FileDiff, String> {
    let repo = std::path::PathBuf::from(path);
    blocking(move || git::commit_diff(&repo, &hash)).await
}

/// 提交选中的文件（**写操作**：只提交传入的路径）
#[tauri::command]
async fn commit_files(
    path: String,
    files: Vec<String>,
    message: String,
) -> Result<git::CommitResult, String> {
    let repo = std::path::PathBuf::from(path);
    blocking(move || git::commit(&repo, &files, &message)).await
}

/// 推送当前分支（**写操作**）
#[tauri::command]
async fn push_repo(path: String) -> Result<git::PushResult, String> {
    let repo = std::path::PathBuf::from(path);
    blocking(move || git::push(&repo)).await
}

/// 拉取当前分支（**写操作**：会更新工作区）
#[tauri::command]
async fn pull_repo(path: String) -> Result<git::PullResult, String> {
    let repo = std::path::PathBuf::from(path);
    blocking(move || git::pull(&repo)).await
}

/// 回滚前的体检（只读）
#[tauri::command]
async fn reset_plan(path: String, hash: String) -> Result<git::ResetPlan, String> {
    let repo = std::path::PathBuf::from(path);
    blocking(move || git::reset_plan(&repo, &hash)).await
}

/// 把当前分支回滚到某个提交（**写操作**）
#[tauri::command]
async fn reset_repo(path: String, hash: String, mode: String) -> Result<git::ResetResult, String> {
    let repo = std::path::PathBuf::from(path);
    blocking(move || git::reset_to(&repo, &hash, &mode)).await
}

/// 巡检结果包：报告本体 + 可直接复制的 Markdown + 可贴进 CI 的命令行。
#[derive(serde::Serialize)]
struct AuditEnvelope {
    report: audit::Report,
    markdown: String,
    command: String,
}

/// 发版 / 交付前的多仓库巡检（只读）
///
/// `depth` / `branch` 传空表示「没指定」，这时会去读目录里的 `.gittracer.toml`；
/// `use_config = false` 则完全不看配置文件。`strict` 决定未提交改动算不算阻断，
/// 传 `None` 表示交给配置文件，再没有就是默认（只提示）。
#[tauri::command]
async fn audit_repos(
    roots: Vec<String>,
    depth: Option<u32>,
    branch: Vec<String>,
    base: Option<String>,
    skip: Vec<String>,
    use_config: Option<bool>,
    strict: Option<bool>,
) -> Result<AuditEnvelope, String> {
    let opts = audit::Options {
        roots: roots
            .into_iter()
            .filter(|r| !r.trim().is_empty())
            .map(std::path::PathBuf::from)
            .collect(),
        depth: depth.map(|d| d.clamp(1, 16)),
        branch_allow: if branch.is_empty() {
            None
        } else {
            Some(
                branch
                    .into_iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect(),
            )
        },
        base: base
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        skip,
        strict,
        use_config: use_config.unwrap_or(true),
        ..audit::Options::default()
    };
    let command = audit::command_line(&opts);
    let report = blocking(move || audit::run(&opts)).await?;
    let markdown = audit::render_markdown(&report);
    Ok(AuditEnvelope {
        report,
        markdown,
        command,
    })
}

/// 系统「选择文件夹」对话框
#[tauri::command]
async fn pick_folder(initial: Option<String>) -> Result<Option<String>, String> {
    let initial = initial.unwrap_or_default();
    blocking(move || dialog::pick_folder("选择要扫描的项目根目录", &initial)).await
}

/// 在资源管理器中定位文件 / 打开目录
#[tauri::command]
async fn reveal_path(path: String) -> Result<(), String> {
    blocking(move || dialog::reveal(&path)).await
}

/// WebView2 的用户数据目录（cookie / 缓存都放这儿）。
///
/// 不能依赖 WebView2 的默认值：默认会往 `%LOCALAPPDATA%\<exe>.exe.WebView2` 里建目录，
/// 在受限环境（只读 AppData、漫游配置、审计策略、某些沙箱）下 `CreateCoreWebView2Environment`
/// 会直接返回 E_ACCESSDENIED，窗口根本建不出来。这里显式给一个可写目录并逐级降级。
fn webview_data_dir(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    let mut candidates = Vec::new();
    // 1. 显式覆盖（便携模式 / 把缓存放到别的盘 / 自动化测试）
    if let Some(v) = std::env::var_os("GIT_TRACER_DATA_DIR").filter(|v| !v.is_empty()) {
        candidates.push(std::path::PathBuf::from(v));
    }
    // 2. 标准位置：%LOCALAPPDATA%\com.git-tracer.app\webview2
    if let Ok(base) = app.path().app_local_data_dir() {
        candidates.push(base.join("webview2"));
    }
    // 3. 兜底：临时目录
    candidates.push(std::env::temp_dir().join("git-tracer-webview2"));
    // 4. 再兜底：exe 旁边的隐藏目录（便携版 / U 盘 / 前两条都不可写的极端环境）
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(".git-tracer-webview2"));
        }
    }
    // 5. 最后：当前工作目录
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(".git-tracer-webview2"));
    }

    for dir in candidates {
        if std::fs::create_dir_all(&dir).is_ok() {
            return Some(dir);
        }
    }
    None
}

/// 手动建主窗口：只有这样才能挂上自定义的 WebView2 数据目录
fn create_main_window(app: &tauri::AppHandle) -> tauri::Result<tauri::WebviewWindow> {
    let mut builder = tauri::WebviewWindowBuilder::new(
        app,
        "main",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("Git Tracer · 待提交内容扫描")
    .inner_size(1360.0, 860.0)
    .min_inner_size(1000.0, 620.0)
    .center()
    .resizable(true)
    .visible(true);

    if let Some(dir) = webview_data_dir(app) {
        builder = builder.data_directory(dir);
    }
    builder.build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            git_info,
            scan_repos,
            repo_status,
            file_diff,
            repo_graph,
            commit_diff,
            commit_files,
            push_repo,
            pull_repo,
            reset_plan,
            reset_repo,
            audit_repos,
            pick_folder,
            reveal_path
        ])
        .setup(|app| {
            // 窗口在代码里建（见 create_main_window），所以 tauri.conf.json 里 windows 是空数组
            create_main_window(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Git Tracer 启动失败");
}
