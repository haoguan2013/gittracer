//! 发版 / 交付前的多仓库巡检。
//!
//! 和界面上「扫描」的区别在问题本身：扫描回答「改了什么」，巡检回答「能不能发」。
//! 它把一组固定规则对所有仓库跑一遍，给每个仓库判 PASS / FAIL，最后收敛成一份
//! 可存档的报告和一个退出码，方便接在 CI 或者交付检查清单后面。
//!
//! 全程只读：只跑 `status` / `rev-parse` / `branch --no-merged` 这类查询，
//! 统一带 `--no-optional-locks`，不 fetch、不写任何引用、不碰工作区。
//!
//! 规则刻意做成固定的一套，不做阈值配置：多条「灵活度」只会让 CI 里跑出来的
//! 结论依赖环境，反而没人敢当门禁用。要放宽就明说跳过哪一条（`--skip`）。
//!
//! ## 为什么分 release / workspace 两组
//!
//! 最早 `dirty` 是阻断项，结果在第一台真实的开发机上跑出来是**全红**：几十个仓库
//! 里凡是你最近动过的都判「未通过」，原因小到只改了一个 `package-lock.json`。
//! 那时候「未通过」等价于「这个仓库你摸过」，结论就没了信息量——全红等于没有红。
//!
//! 问题出在把两件不同的事塞进了同一个判定：
//!
//! - **release**：发出去的那个提交本身有问题吗？冲突、半途的 rebase、分离的 HEAD、
//!   没推上去的提交、分支不对。这些不修就是事故。
//! - **workspace**：你手上还有没提交的活。这是写代码的常态，不是发布缺陷。
//!
//! 所以前者算阻断，后者算提示，各自带着自己的 [`Scope`] 进报告。真要从工作区直接
//! 打包发版的团队，用 `--strict`（或配置里 `strict = true`）把 `dirty` 提回阻断——
//! 那是他们的实际风险，不该由默认值替他们决定。
//!
//! 规则本身可以从 `.gittracer.toml` 里来（见 [`crate::config`]）：命令行 / 界面
//! 显式给的优先，其次是仓库自带的文件，再次是根目录那份。这样同一套判定既能
//! 跟仓库一起进版本库，也能在 CI 里一模一样地跑一遍。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::config::{self, Rules};
use crate::git;
use crate::scan;

/// 巡检选项。界面命令与 CLI 共用同一份结构，默认值集中在 `Default` 里。
///
/// 几个「没指定」和「设成空」要分开的地方都用 `Option` 表达：
/// 空数组的意思是「明确不检查」，`None` 才是「没写，去问配置文件」。
#[derive(Clone, Debug)]
pub struct Options {
    pub roots: Vec<PathBuf>,
    /// `None` = 用配置文件里的 depth，再没有就用 4
    pub depth: Option<u32>,
    /// 允许的分支模式（`*` 通配）。`None` = 未指定，`Some(vec![])` = 明确不检查
    pub branch_allow: Option<Vec<String>>,
    /// 未合并分支检查的基准；`None` 表示按仓库自动探测 main / master / develop
    pub base: Option<String>,
    /// 跳过的规则 id（不区分大小写）。`skip` 只累加，文件里的不会覆盖命令行的
    pub skip: Vec<String>,
    /// 严格模式：把工作区的未提交改动也算阻断。`None` = 没指定，去问配置文件。
    /// 要从工作区直接打包发版时开它——脏树打出来的产物对不上任何提交。
    pub strict: Option<bool>,
    /// 单个根目录最多收多少个仓库，防止误指到盘符
    pub max_repos: usize,
    /// 是否读取目录 / 仓库里的 `.gittracer.toml`
    pub use_config: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            depth: None,
            branch_allow: None,
            base: None,
            skip: Vec::new(),
            strict: None,
            max_repos: 2000,
            use_config: true,
        }
    }
}

/// 全部规则 id。CLI 用它校验 `--skip` 的拼写：拼错了要当场报错，
/// 否则「以为跳过了、其实还在检查」会让人对着 CI 排查半天。
pub const RULES: &[&str] = &[
    "conflict",
    "operation",
    "dirty",
    "detached",
    "unpushed",
    "behind",
    "no-upstream",
    "stash",
    "branch",
    "unmerged",
    "no-remote",
];

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// 阻断项：这一条不过，就不该发版
    Error,
    /// 提示项：不影响结论，但值得看一眼
    Warn,
    /// 说明项：只是记录，不影响结论
    Info,
}

impl Severity {
    pub fn word(self) -> &'static str {
        match self {
            Severity::Error => "ERROR",
            Severity::Warn => "WARN",
            Severity::Info => "INFO",
        }
    }
}

/// 规则在问哪一类问题。报告按这个分组，「能不能发」只看 [`Scope::Release`]。
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// 发出去的那个提交本身有毛病：冲突、半途的变基、分离 HEAD、没推、分支不对
    Release,
    /// 工作区里还有没提交的活：脏树、stash、落后远程、遗留分支
    Workspace,
}

#[derive(Serialize, Clone, Debug)]
pub struct Finding {
    pub code: String,
    pub severity: Severity,
    pub scope: Scope,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Finding {
    fn new(code: &str, severity: Severity, scope: Scope, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            severity,
            scope,
            message: message.into(),
            detail: None,
        }
    }

    fn with_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct RepoAudit {
    pub name: String,
    pub path: String,
    pub root: String,
    pub branch: Option<String>,
    pub findings: Vec<Finding>,
    /// 这个仓库自带的 `.gittracer.toml`（根目录那份不算，它属于整次运行）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// 本仓库实际生效的跳过项。写进报告，免得「跳过」只发生在内存里、
    /// 事后没人说得清这条是怎么过的
    pub effective_skip: Vec<String>,
    /// 读状态失败时的原始原因（此时 findings 里只有一条 read-error）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl RepoAudit {
    pub fn failed(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    /// 有工作区待办（未提交改动 / stash / 落后远程 / 遗留分支）。不影响结论，
    /// 但要单独汇总——发版单上得写一句「这几个仓库有活没提交」。
    pub fn workspace_pending(&self) -> bool {
        self.findings.iter().any(|f| f.scope == Scope::Workspace)
    }

    /// 工作区那几条的说明拼成一行，进报告的「工作区」小节
    pub fn workspace_note(&self) -> String {
        self.findings
            .iter()
            .filter(|f| f.scope == Scope::Workspace)
            .map(|f| f.message.clone())
            .collect::<Vec<_>>()
            .join("；")
    }
}

/// 三个结论。之前只有「通过 / 未通过」两档，脏树一律算未通过，
/// 于是每台开发机都是全红；现在中间多一档「注意」：能发，但有活没提交。
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// 没有阻断项，也没有提示
    Passed,
    /// 没有阻断项，但有提示（最常见：仓库脏着）
    Attention,
    /// 有阻断项，不该发
    Blocked,
}

impl Verdict {
    pub fn word(self) -> &'static str {
        match self {
            Verdict::Passed => "通过",
            Verdict::Attention => "注意",
            Verdict::Blocked => "未通过",
        }
    }
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Totals {
    pub repos: usize,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    /// 有工作区待办的仓库数（这些仓库可能同时通过）
    pub workspace: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct Report {
    pub generated_at: String,
    pub roots: Vec<String>,
    /// 实际用的扫描深度；多个根目录深浅不一时取最大
    pub depth: u32,
    /// 默认基准分支（各仓库可被自己的配置文件覆盖）
    pub base: Option<String>,
    pub branch_allow: Vec<String>,
    pub skipped: Vec<String>,
    /// 严格模式是否生效（`dirty` 算不算阻断）
    pub strict: bool,
    /// 本次读到的全部配置文件，按路径去重
    pub config_files: Vec<String>,
    /// 给进来但用不了的根目录（不存在 / 不是目录）。不静默跳过：
    /// CI 里把目录名拼错，跳过就等于「一份空报告、结论通过」，那是假通过。
    pub root_errors: Vec<String>,
    pub repos: Vec<RepoAudit>,
    pub totals: Totals,
    pub verdict: Verdict,
    pub elapsed_ms: u64,
}

impl Report {
    /// 退出码用这个：只看阻断项，提示不影响「能不能发」。
    /// 根目录有问题也算不过——它会让整份报告缺东西。
    pub fn passed(&self) -> bool {
        self.totals.errors == 0 && self.root_errors.is_empty()
    }

    pub fn verdict_word(&self) -> &'static str {
        self.verdict.word()
    }
}

/* ---------------------------------------------------------------- 执行 */

pub fn run(opts: &Options) -> Result<Report, String> {
    let started = Instant::now();
    if opts.roots.is_empty() {
        return Err("没有指定要巡检的根目录".to_string());
    }
    validate(opts)?;

    let mut roots: Vec<(PathBuf, Option<config::FileConfig>)> = Vec::new();
    let mut root_errors: Vec<String> = Vec::new();
    for r in &opts.roots {
        if !r.exists() {
            root_errors.push(format!("{}：目录不存在", r.display()));
            continue;
        }
        if !r.is_dir() {
            root_errors.push(format!("{}：不是一个目录", r.display()));
            continue;
        }
        let cfg = if opts.use_config {
            match config::find_in(r) {
                Some(p) => Some(config::load(&p, RULES)?),
                None => None,
            }
        } else {
            None
        };
        roots.push((r.clone(), cfg));
    }
    // 一个能用的都没有就没必要往下跑了：继续跑会生成一份「0 个仓库、结论通过」
    // 的报告，那比直接报错危险得多。
    if roots.is_empty() {
        return Err(root_errors.join("；"));
    }

    let mut config_files: Vec<String> = Vec::new();
    let mut depth_used: u32 = 0;

    let mut seen: HashSet<String> = HashSet::new();
    let mut refs: Vec<scan::RepoRef> = Vec::new();
    let mut root_of: HashMap<String, usize> = HashMap::new();

    for (idx, (root, root_cfg)) in roots.iter().enumerate() {
        // 深度可以来自配置文件；扫描前就得定下来，所以放在循环里算
        let depth = opts
            .depth
            .or_else(|| root_cfg.as_ref().and_then(|c| c.depth))
            .unwrap_or(4)
            .clamp(1, 16);
        depth_used = depth_used.max(depth);
        if let Some(c) = root_cfg {
            note_config(&mut config_files, &c.display());
        }

        let res = scan::scan(root, depth, opts.max_repos)?;
        for r in res.repos {
            // 嵌套的根目录会把同一个仓库扫两遍，按规范路径去重。
            // 不能只 to_lowercase：同一个目录经「根目录本身」拿到的路径保留用户输入的
            // 分隔符（可能是 /），而遍历出来的子目录是 \，字符串不相等。
            let key = path_key(&r.path);
            if seen.insert(key.clone()) {
                root_of.insert(key, idx);
                refs.push(r);
            }
        }
    }
    if depth_used == 0 {
        depth_used = opts.depth.unwrap_or(4).clamp(1, 16);
    }

    // 先把每个仓库的规则算清楚（读配置是几次文件 IO，快），再并行跑 git。
    // 拆成两步是为了让「某个仓库的配置有毛病」只污染它自己那一行，不打断整次巡检。
    let mut plans: Vec<Plan> = Vec::with_capacity(refs.len());
    for r in &refs {
        let idx = root_of.get(&path_key(&r.path)).copied().unwrap_or(0);
        let (root_path, root_cfg) = roots.get(idx).cloned().unwrap_or_default();

        // 仓库自己带的配置。仓库目录正好等于根目录时两份是同一个文件，
        // 读一次就够——这时它按「根目录配置」生效，不再重复算成仓库配置。
        let same_as_root = path_key(&root_path.display().to_string()) == path_key(&r.path);
        let mut broken: Option<String> = None;
        let mut note: Option<String> = None;
        let repo_cfg = if opts.use_config && !same_as_root {
            match config::find_in(Path::new(&r.path)) {
                Some(p) => match config::load(&p, RULES) {
                    Ok(c) => {
                        // depth 决定「从哪里往下找仓库」，只有根目录那份说得通。
                        // 仓库里写了不认它，但也不能装作没看见：这多半是同一个文件
                        // 被当成根目录配置在别处用，说清楚就行，不必让整个仓库不过。
                        if c.depth.is_some() {
                            note = Some(format!(
                                "{} 里的 depth 在这里不生效（它只管从根目录往下找几层），其余设置照常",
                                p.display()
                            ));
                        }
                        note_config(&mut config_files, &c.display());
                        Some(c)
                    }
                    // 真解析不出来就只能按上一层跑，此时结论不可信，标成阻断
                    Err(e) => {
                        broken = Some(e);
                        None
                    }
                },
                None => None,
            }
        } else {
            None
        };

        let rules = Rules::resolve(
            opts.branch_allow.as_deref(),
            opts.base.as_deref(),
            &opts.skip,
            opts.strict,
            repo_cfg.as_ref(),
            root_cfg.as_ref(),
        );
        plans.push(Plan {
            root: root_path.display().to_string(),
            rules,
            cfg: repo_cfg.as_ref().map(|c| c.display()),
            broken,
            note,
        });
    }

    let mut repos = run_repos(&refs, &plans);
    // 未通过的排前面：CI 日志一屏里先看到要修的
    repos.sort_by(|a, b| {
        b.failed()
            .cmp(&a.failed())
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    let mut totals = Totals {
        repos: repos.len(),
        ..Default::default()
    };
    for r in &repos {
        if r.failed() {
            totals.failed += 1;
        } else {
            totals.passed += 1;
        }
        if r.workspace_pending() {
            totals.workspace += 1;
        }
        for f in &r.findings {
            match f.severity {
                Severity::Error => totals.errors += 1,
                Severity::Warn => totals.warnings += 1,
                Severity::Info => totals.infos += 1,
            }
        }
    }

    // 报告头部的这几项是「默认值」：第一个根目录那一层解析出来的结果。
    // 仓库各有覆盖时以 JSON 里每个仓库的 effective_skip / config 为准。
    let head_rules = Rules::resolve(
        opts.branch_allow.as_deref(),
        opts.base.as_deref(),
        &opts.skip,
        opts.strict,
        None,
        roots.first().and_then(|(_, c)| c.as_ref()),
    );

    let verdict = if totals.errors > 0 || !root_errors.is_empty() {
        Verdict::Blocked
    } else if totals.warnings > 0 {
        Verdict::Attention
    } else {
        Verdict::Passed
    };

    Ok(Report {
        generated_at: now_iso8601(),
        roots: roots
            .iter()
            .map(|(p, _)| p.display().to_string())
            .collect(),
        depth: depth_used,
        base: head_rules.base.clone(),
        branch_allow: head_rules.branch_allow.clone(),
        skipped: head_rules.skip.clone(),
        strict: head_rules.strict,
        config_files,
        root_errors,
        repos,
        totals,
        verdict,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

/// 一个仓库要跑的东西：规则已经合并完，配置路径和它的问题也带上了。
struct Plan {
    root: String,
    rules: Rules,
    cfg: Option<String>,
    /// 配置解析失败：按上一层跑，但结论不可信，算阻断
    broken: Option<String>,
    /// 配置里被忽略的项（depth）：说清楚，不阻断
    note: Option<String>,
}

/// 并行跑所有仓库，结果顺序和 `refs` 一致。
///
/// 为什么要并行：`git status` 是一次进程调用，一个带 node_modules 的仓库单次要
/// 一两秒，35 个仓库串行 90 秒。发版前没人愿意等 90 秒，而这个检查的价值恰恰在
/// 「顺手跑一下」。8 路并发就能把这段时间压到十几秒，且全程只读，没有副作用。
fn run_repos(refs: &[scan::RepoRef], plans: &[Plan]) -> Vec<RepoAudit> {
    let n = refs.len();
    if n == 0 {
        return Vec::new();
    }
    let workers = std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(4)
        .clamp(1, 8)
        .min(n);
    let chunk = (n + workers - 1) / workers;
    let mut out: Vec<RepoAudit> = Vec::with_capacity(n);

    std::thread::scope(|s| {
        let handles: Vec<_> = plans
            .chunks(chunk)
            .enumerate()
            .map(|(ci, part)| {
                let start = ci * chunk;
                s.spawn(move || -> Vec<RepoAudit> {
                    part.iter()
                        .enumerate()
                        .map(|(k, p)| {
                            audit_repo(
                                &p.root,
                                &refs[start + k],
                                &p.rules,
                                p.cfg.clone(),
                                p.broken.clone(),
                                p.note.clone(),
                            )
                        })
                        .collect()
                })
            })
            .collect();

        for (ci, h) in handles.into_iter().enumerate() {
            match h.join() {
                Ok(v) => out.extend(v),
                // 线程炸了不能悄悄少几个仓库：门禁少查一个就是假通过
                Err(_) => {
                    let start = ci * chunk;
                    for (k, p) in plans[start..(start + chunk).min(n)].iter().enumerate() {
                        out.push(audit_repo(
                            &p.root,
                            &refs[start + k],
                            &p.rules,
                            p.cfg.clone(),
                            p.broken.clone(),
                            p.note.clone(),
                        ));
                    }
                }
            }
        }
    });

    out
}

fn note_config(list: &mut Vec<String>, path: &str) {
    if !list.iter().any(|p| path_key(p) == path_key(path)) {
        list.push(path.to_string());
    }
}

/// 校验命令行 / 界面直接给的值。配置文件里的值在 `config::load` 里已经校验过。
fn validate(opts: &Options) -> Result<(), String> {
    if let Some(b) = &opts.base {
        config::check_ref_like("--base", b)?;
    }
    if let Some(list) = &opts.branch_allow {
        for p in list {
            config::check_ref_like("--branch", p)?;
        }
    }
    Ok(())
}

fn audit_repo(
    root: &str,
    repo: &scan::RepoRef,
    rules: &Rules,
    repo_cfg: Option<String>,
    cfg_broken: Option<String>,
    cfg_note: Option<String>,
) -> RepoAudit {
    let path = PathBuf::from(&repo.path);
    let mut findings: Vec<Finding> = Vec::new();
    let mut branch: Option<String> = None;
    let mut error: Option<String> = None;

    // 脏树算不算阻断，看严格模式：从工作区直接打包的团队必须当阻断，
    // 其余人只是在写代码——把它当阻断会让每台机器都全红，门禁就废了。
    let dirty_sev = if rules.strict {
        Severity::Error
    } else {
        Severity::Warn
    };

    if let Some(note) = cfg_note {
        findings.push(Finding::new("config-note", Severity::Warn, Scope::Release, note));
    }
    if let Some(broken) = &cfg_broken {
        findings.push(Finding::new(
            "config-error",
            Severity::Error,
            Scope::Release,
            "仓库级配置读不了，这条仓库按上一层规则跑的，结论不可信".to_string(),
        ).with_detail(Some(broken.clone())));
    }

    match git::status_lite(&path) {
        Err(e) => {
            error = Some(e.clone());
            findings.push(Finding::new(
                "read-error",
                Severity::Error,
                Scope::Release,
                format!("读不到仓库状态：{e}"),
            ));
        }
        Ok(st) => {
            branch = st.branch.clone();

            // 1. 冲突：最容易把「发版」变成「事故」的一条，排最前
            if st.conflicted > 0 && !rules.skipped("conflict") {
                findings.push(Finding::new(
                    "conflict",
                    Severity::Error,
                    Scope::Release,
                    format!("有 {} 个文件处于冲突状态", st.conflicted),
                ));
            }

            // 2. 半途而废的合并 / 变基
            if !rules.skipped("operation") {
                if let Some(op) = git_dir(&path).and_then(|d| operation_in_progress(&d)) {
                    findings.push(Finding::new(
                        "operation",
                        Severity::Error,
                        Scope::Release,
                        format!("{op} 还没做完，仓库停在中途状态"),
                    ));
                }
            }

            // 3. 未提交改动。默认只是提示：几乎每台开发机在任何时刻都有未提交的活，
            //    把它算阻断的结果是「全员全红」，而那正是这个工具最难用的地方。
            //    严格模式下提回阻断（从工作区直接打包发版时它就是真风险）。
            let dirty = st.staged as usize + st.unstaged as usize + st.untracked as usize;
            if dirty > 0 && !rules.skipped("dirty") {
                findings.push(
                    Finding::new(
                        "dirty",
                        dirty_sev,
                        Scope::Workspace,
                        format!(
                            "有 {dirty} 处未提交改动（{} 暂存 / {} 未暂存 / {} 未跟踪）",
                            st.staged, st.unstaged, st.untracked
                        ),
                    )
                    .with_detail(sample_paths(&st)),
                );
            }

            // 4. detached HEAD：这种状态下发出去的版本对不上任何分支
            if st.detached && !rules.skipped("detached") {
                findings.push(Finding::new(
                    "detached",
                    Severity::Error,
                    Scope::Release,
                    "HEAD 已分离，不在任何分支上".to_string(),
                ));
            }

            // 5. 未推送
            if st.ahead > 0 && !rules.skipped("unpushed") {
                let up = st.upstream.clone().unwrap_or_else(|| "上游".to_string());
                findings.push(
                    Finding::new(
                        "unpushed",
                        Severity::Error,
                        Scope::Release,
                        format!("有 {} 个提交还没推到 {up}", st.ahead),
                    )
                    .with_detail(Some("别人拿到的代码里没有这些提交".to_string())),
                );
            }

            // 6. 落后远程：只是提示，拉一下就好
            if st.behind > 0 && !rules.skipped("behind") {
                findings.push(Finding::new(
                    "behind",
                    Severity::Warn,
                    Scope::Workspace,
                    format!("落后远程 {} 个提交", st.behind),
                ));
            }

            // 7. 没有上游：推不了，也说明这个分支是纯本地的
            if st.upstream.is_none() && !st.initial && !st.detached && !rules.skipped("no-upstream") {
                findings.push(Finding::new(
                    "no-upstream",
                    Severity::Warn,
                    Scope::Workspace,
                    "当前分支没有上游分支".to_string(),
                ));
            }

            // 8. stash：临时改动忘在栈里，往往就是「发出去的版本少了个补丁」
            if st.stashes > 0 && !rules.skipped("stash") {
                findings.push(Finding::new(
                    "stash",
                    Severity::Warn,
                    Scope::Workspace,
                    format!("有 {} 条 stash 没清理", st.stashes),
                ));
            }

            // 9. 分支是否在允许列表里
            if !rules.branch_allow.is_empty() && !rules.skipped("branch") {
                if let Some(b) = &st.branch {
                    if !rules.branch_allow.iter().any(|p| glob_match(p, b)) {
                        findings.push(Finding::new(
                            "branch",
                            Severity::Error,
                            Scope::Release,
                            format!(
                                "当前分支 {b} 不在允许列表里（允许：{}）",
                                rules.branch_allow.join(" / ")
                            ),
                        ));
                    }
                }
            }

            // 10. 遗留的本地分支
            if !rules.skipped("unmerged") {
                if let Some(f) = unmerged_finding(&path, rules.base.as_deref(), st.branch.as_deref()) {
                    findings.push(f);
                }
            }

            // 11. 没有远程：不阻断（有些仓库本来就只在本地），但要说明
            if st.remotes.is_empty() && !rules.skipped("no-remote") {
                findings.push(Finding::new(
                    "no-remote",
                    Severity::Info,
                    Scope::Release,
                    "没有配置远程仓库".to_string(),
                ));
            }
        }
    }

    // 先按严重级别、再按分组：ERROR 在最前，同级里发版状态排在「工作区还有活」前面
    findings.sort_by_key(|f| (f.severity, f.scope));

    RepoAudit {
        name: repo.name.clone(),
        path: repo.path.clone(),
        root: root.to_string(),
        branch,
        findings,
        config: repo_cfg,
        effective_skip: rules.skip.clone(),
        error,
    }
}

/// 挑几个文件名当改动摘要，别把几百个文件全塞进报告。
fn sample_paths(st: &git::RepoStatus) -> Option<String> {
    if st.entries.is_empty() {
        return None;
    }
    let mut names: Vec<String> = st.entries.iter().take(3).map(|e| e.path.clone()).collect();
    if st.entries.len() > 3 {
        names.push(format!("…等 {} 个", st.entries.len()));
    }
    Some(names.join("、"))
}

/* ---------------------------------------------------------------- 仓库内部状态 */

/// 解析出真正的 git 目录：普通仓库是 `<repo>/.git`，worktree / 子模块是文件里的 `gitdir:`。
fn git_dir(repo: &Path) -> Option<PathBuf> {
    let marker = repo.join(".git");
    let meta = std::fs::metadata(&marker).ok()?;
    if meta.is_dir() {
        return Some(marker);
    }
    let text = std::fs::read_to_string(&marker).ok()?;
    let line = text.lines().next()?;
    let rest = line.strip_prefix("gitdir:")?.trim();
    if rest.is_empty() {
        return None;
    }
    let p = PathBuf::from(rest);
    Some(if p.is_absolute() { p } else { repo.join(p) })
}

/// 有没有停在半途的合并 / 变基 / 拣选。只看标记文件，不跑 git。
fn operation_in_progress(git_dir: &Path) -> Option<&'static str> {
    let has = |name: &str| git_dir.join(name).exists();
    if has("rebase-merge") {
        return Some("rebase");
    }
    if has("rebase-apply") {
        return Some("rebase / am");
    }
    if has("MERGE_HEAD") {
        return Some("merge");
    }
    if has("CHERRY_PICK_HEAD") {
        return Some("cherry-pick");
    }
    if has("REVERT_HEAD") {
        return Some("revert");
    }
    if has("BISECT_LOG") {
        return Some("bisect");
    }
    None
}

/// 基准分支：显式给了就用它，否则按 main → master → develop 的顺序找第一个存在的。
fn resolve_base(repo: &Path, prefer: Option<&str>) -> Option<String> {
    let candidates: Vec<String> = match prefer {
        Some(b) => vec![b.trim().to_string()],
        None => vec!["main".to_string(), "master".to_string(), "develop".to_string()],
    };
    for c in candidates {
        if c.is_empty() {
            continue;
        }
        let spec = format!("{c}^{{commit}}");
        let ok = git::git_lines(repo, &["rev-parse", "--verify", "--quiet", &spec])
            .map(|l| !l.is_empty())
            .unwrap_or(false);
        if ok {
            return Some(c);
        }
    }
    None
}

fn unmerged_finding(repo: &Path, prefer: Option<&str>, current: Option<&str>) -> Option<Finding> {
    let base = match resolve_base(repo, prefer) {
        Some(b) => b,
        // 显式指定了基准分支，但这个仓库里没有它：不能装作查过了。
        // 自动探测失败时（仓库只有一个分支、也没有 main/master/develop）不算问题。
        None => {
            return prefer.map(|b| {
                Finding::new(
                    "unmerged",
                    Severity::Warn,
                    Scope::Workspace,
                    format!("基准分支 {b} 在这个仓库里不存在，本条未生效"),
                )
            })
        }
    };
    let names = match git::git_lines(
        repo,
        &["branch", "--no-merged", &base, "--format=%(refname:short)"],
    ) {
        Ok(n) => n,
        Err(e) => {
            return Some(Finding::new(
                "unmerged",
                Severity::Warn,
                Scope::Workspace,
                format!("查不到未合并分支：{}", clip(&e, 120)),
            ))
        }
    };

    let rest: Vec<String> = names
        .into_iter()
        .filter(|b| Some(b.as_str()) != current && b != &base)
        .collect();
    if rest.is_empty() {
        return None;
    }
    let mut shown: Vec<String> = rest.iter().take(5).cloned().collect();
    if rest.len() > 5 {
        shown.push(format!("…等 {} 个", rest.len()));
    }
    Some(
        Finding::new(
            "unmerged",
            Severity::Warn,
            Scope::Workspace,
            format!("有 {} 个本地分支没合并进 {base}", rest.len()),
        )
        .with_detail(Some(shown.join("、"))),
    )
}

/* ---------------------------------------------------------------- 报告渲染 */

pub fn render_json(report: &Report) -> String {
    let mut s = serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string());
    s.push('\n');
    s
}

pub fn render_text(report: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!("Git Tracer 巡检  {}\n", report.generated_at));
    out.push_str(&format!("根目录  {}\n", report.roots.join("；")));
    out.push_str(&format!("深度    {}\n", report.depth));
    out.push_str(&format!("基准    {}\n", base_text(report)));
    out.push_str(&format!("模式    {}\n", mode_text(report)));
    // 用不了的根目录放在最前面说：它会让整份报告缺东西，不能藏在结尾
    for e in &report.root_errors {
        out.push_str(&format!("不可用  {e}（已跳过，其余照常）\n"));
    }
    if !report.branch_allow.is_empty() {
        out.push_str(&format!("允许分支  {}\n", report.branch_allow.join(" / ")));
    }
    if !report.skipped.is_empty() {
        out.push_str(&format!("跳过规则  {}\n", report.skipped.join(", ")));
    }
    if !report.config_files.is_empty() {
        out.push_str(&format!(
            "配置      {} 个 .gittracer.toml\n",
            report.config_files.len()
        ));
    }
    out.push('\n');

    if report.repos.is_empty() {
        out.push_str("没有找到任何 Git 仓库。\n");
    }

    for repo in &report.repos {
        let mark = if repo.failed() { "FAIL" } else { "PASS" };
        let branch = repo.branch.as_deref().unwrap_or("(detached)");
        out.push_str(&format!(
            "{}  {}  <{}>\n",
            mark,
            pad(&repo.name, 26),
            branch
        ));
        // 只在这个仓库和全局默认不一样时才单独列，否则 200 个仓库全在刷同一行
        if let Some(c) = &repo.config {
            out.push_str(&format!("      配置    {c}\n"));
        }
        if !same_skip(&repo.effective_skip, &report.skipped) {
            out.push_str(&format!(
                "      跳过    {}\n",
                repo.effective_skip.join(", ")
            ));
        }
        for f in &repo.findings {
            out.push_str(&format!(
                "      {:<5}  {:<12} {}\n",
                f.severity.word(),
                f.code,
                f.message
            ));
            if let Some(d) = &f.detail {
                out.push_str(&format!("                        {d}\n"));
            }
        }
        out.push('\n');
    }

    let t = &report.totals;
    out.push_str(&format!(
        "{} 个仓库：{} 未通过 / {} 通过 · {} 阻断 / {} 提示 · 结论：{} · {:.1}s\n",
        t.repos,
        t.failed,
        t.passed,
        t.errors,
        t.warnings + t.infos,
        report.verdict_word(),
        report.elapsed_ms as f64 / 1000.0
    ));
    // 全红问题的另一半：就算都没有阻断项，也要让人一眼看到「有活没提交」，
    // 否则会以为报告漏了东西。
    if t.workspace > 0 {
        out.push_str(&format!(
            "工作区  {} 个仓库有待提交 / stash / 落后远程（只提示，{}）\n",
            t.workspace,
            if report.strict {
                "严格模式下已计入阻断"
            } else {
                "--strict 时算阻断"
            }
        ));
    }
    out
}

pub fn render_markdown(report: &Report) -> String {
    let t = &report.totals;
    let mut out = String::new();
    out.push_str("# Git Tracer 巡检报告\n\n");
    out.push_str("| 项 | 值 |\n| --- | --- |\n");
    out.push_str(&format!("| 时间 | {} |\n", report.generated_at));
    out.push_str(&format!("| 根目录 | {} |\n", code_join(&report.roots)));
    out.push_str(&format!("| 扫描深度 | {} |\n", report.depth));
    out.push_str(&format!("| 基准分支 | {} |\n", base_text(report)));
    out.push_str(&format!("| 检查模式 | {} |\n", mode_text(report)));
    if !report.branch_allow.is_empty() {
        out.push_str(&format!(
            "| 允许分支 | {} |\n",
            code_join(&report.branch_allow)
        ));
    }
    if !report.skipped.is_empty() {
        out.push_str(&format!("| 跳过规则 | {} |\n", report.skipped.join(", ")));
    }
    if !report.config_files.is_empty() {
        out.push_str(&format!(
            "| 配置文件 | {} 个 `.gittracer.toml` |\n",
            report.config_files.len()
        ));
    }
    for e in &report.root_errors {
        out.push_str(&format!("| 跳过的根目录 | {} |\n", md_escape(e)));
    }
    out.push_str(&format!("| 结论 | **{}** |\n", report.verdict_word()));
    out.push('\n');
    out.push_str(&format!(
        "{} 个仓库：{} 未通过 / {} 通过 · {} 阻断 / {} 提示 · {:.1}s\n\n",
        t.repos,
        t.failed,
        t.passed,
        t.errors,
        t.warnings + t.infos,
        report.elapsed_ms as f64 / 1000.0
    ));

    let failed: Vec<&RepoAudit> = report.repos.iter().filter(|r| r.failed()).collect();
    let passed: Vec<&RepoAudit> = report.repos.iter().filter(|r| !r.failed()).collect();

    if failed.is_empty() {
        out.push_str("## 需要处理\n\n没有阻断项。\n\n");
    } else {
        out.push_str(&format!("## 需要处理（{}）\n\n", failed.len()));
        for repo in failed {
            out.push_str(&format!(
                "### {} `{}`\n\n`{}`\n\n",
                md_escape(&repo.name),
                md_escape(repo.branch.as_deref().unwrap_or("detached")),
                md_escape(&repo.path)
            ));
            // 结论是被本地配置放宽过的，必须写出来，不然这份报告不能当证据
            if let Some(c) = &repo.config {
                out.push_str(&format!("依据仓库配置：`{}`\n\n", md_escape(c)));
            }
            if !same_skip(&repo.effective_skip, &report.skipped) {
                out.push_str(&format!(
                    "本仓库跳过：{}\n\n",
                    md_escape(&repo.effective_skip.join(", "))
                ));
            }
            out.push_str("| 级别 | 规则 | 说明 |\n| --- | --- | --- |\n");
            for f in &repo.findings {
                out.push_str(&format!(
                    "| {} | {} | {} |\n",
                    f.severity.word(),
                    md_escape(&f.code),
                    md_escape(&f.message)
                ));
            }
            out.push('\n');
            for f in &repo.findings {
                if let Some(d) = &f.detail {
                    out.push_str(&format!("- {}：{}\n", md_escape(&f.code), md_escape(d)));
                }
            }
            out.push('\n');
        }
    }

    // 工作区单独一节：这些不是阻断项，但发版单上通常要交代一句「谁手上还有活」。
    // 只列通过的仓库——未通过的在上面已经把自己的 dirty 写在表里了。
    let workspace: Vec<&RepoAudit> = passed
        .iter()
        .copied()
        .filter(|r| r.workspace_pending())
        .collect();
    if !workspace.is_empty() {
        out.push_str(&format!(
            "## 工作区（{}，不计入结论）\n\n\
             这些仓库有未提交改动 / stash / 落后远程。不影响「能不能发」，\
             但如果你是从工作区直接打包的，用 `--strict` 再跑一遍。\n\n\
             | 仓库 | 分支 | 待处理 |\n| --- | --- | --- |\n",
            workspace.len()
        ));
        for repo in &workspace {
            out.push_str(&format!(
                "| {} | `{}` | {} |\n",
                md_escape(&repo.name),
                md_escape(repo.branch.as_deref().unwrap_or("detached")),
                md_escape(&repo.workspace_note())
            ));
        }
        out.push('\n');
    }

    if !passed.is_empty() {
        out.push_str(&format!("## 已通过（{}）\n\n", passed.len()));
        for repo in passed {
            let mut tag = String::new();
            if repo.config.is_some() {
                tag.push_str(" · 依据仓库配置");
            }
            // 靠跳过规则才过的仓库，不能只写「通过」——报告要能自证
            if !same_skip(&repo.effective_skip, &report.skipped) {
                tag.push_str(&format!(
                    " · 跳过 {}",
                    md_escape(&repo.effective_skip.join(", "))
                ));
            }
            // 脏着过的仓库要标出来：不然「全部通过」会被读成「什么都不用管」
            if repo.workspace_pending() {
                tag.push_str(" · 工作区有活");
            }
            out.push_str(&format!(
                "- {} `{}`{}\n",
                md_escape(&repo.name),
                md_escape(repo.branch.as_deref().unwrap_or("detached")),
                tag
            ));
        }
        out.push('\n');
    }
    out
}

/// 生成一条可以直接贴进 CI 配置的命令行。
pub fn command_line(opts: &Options) -> String {
    let mut parts: Vec<String> = vec!["gittracer-audit".to_string()];
    for r in &opts.roots {
        parts.push(format!("--root \"{}\"", r.display()));
    }
    if let Some(d) = opts.depth {
        parts.push(format!("--depth {d}"));
    }
    if let Some(list) = &opts.branch_allow {
        for b in list {
            parts.push(format!("--branch \"{b}\""));
        }
    }
    if let Some(b) = &opts.base {
        parts.push(format!("--base \"{b}\""));
    }
    if !opts.skip.is_empty() {
        parts.push(format!("--skip \"{}\"", opts.skip.join(",")));
    }
    if opts.strict == Some(true) {
        parts.push("--strict".to_string());
    }
    if !opts.use_config {
        parts.push("--no-config".to_string());
    }
    parts.push("--fail-on error".to_string());
    parts.join(" ")
}

/// 两组跳过项是否等价（顺序和大小写都不算差异）。
fn same_skip(a: &[String], b: &[String]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let norm = |list: &[String]| {
        let mut v: Vec<String> = list.iter().map(|s| s.to_lowercase()).collect();
        v.sort();
        v
    };
    norm(a) == norm(b)
}

fn base_text(report: &Report) -> String {
    match &report.base {
        Some(b) => b.clone(),
        None => "自动探测（main / master / develop）".to_string(),
    }
}

/// 两种模式的区别就一句话：脏树算不算阻断。
fn mode_text(report: &Report) -> String {
    if report.strict {
        "严格（未提交改动算阻断）".to_string()
    } else {
        "默认（未提交改动只提示）".to_string()
    }
}

fn code_join(list: &[String]) -> String {
    list.iter()
        .map(|s| format!("`{}`", md_escape(s)))
        .collect::<Vec<_>>()
        .join("、")
}

/// Markdown 转义：竖线会把单元格切开，反引号会把后续文字带进代码块，
/// 控制字符（文件名里可以有换行）会把表格和列表整段拆坏。
fn md_escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '|' => "\\|".to_string(),
            '`' => "'".to_string(),
            c if c.is_control() => " ".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// 终端里中日韩字符占两格，只按字符数补空格会让中文仓库名那一列对不齐。
fn cell_width(c: char) -> usize {
    match c as u32 {
        0x1100..=0x115F
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F
        | 0x20000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// 按显示宽度补空格，让报告里的仓库名对齐。
fn pad(s: &str, width: usize) -> String {
    let w: usize = s.chars().map(cell_width).sum();
    if w >= width {
        s.to_string()
    } else {
        format!("{}{}", s, " ".repeat(width - w))
    }
}

/* ---------------------------------------------------------------- 小工具 */

/// 截断长错误信息：git 的报错可能带上整个命令行，塞进报告会糊成一团。
fn clip(s: &str, max: usize) -> String {
    let t = s.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let head: String = t.chars().take(max).collect();
    format!("{head}…")
}

/// 路径去重用的归一化键：统一分隔符 + 不分大小写（Windows）+ 去掉末尾分隔符。
///
/// 只做字符串层面的事，不碰文件系统：`canonicalize` 对每个仓库都是一次
/// 系统调用，在几十个仓库上不划算，而且会把网络盘路径拉成 UNC。
pub fn path_key(path: &str) -> String {
    let mut s = path.replace('/', "\\").to_lowercase();
    // 同一目录写成 `D:\work\repo\` 和 `D:\work\repo` 时字符串不相等，会重复计数；
    // 最短的根（`C:\`）留着，别把盘符修剪成 `C:`。
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    s
}

/// 只支持 `*`（任意长度）和 `?`（单个字符）的通配匹配，够描述 `release/*` 这类规则。
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while j < t.len() {
        if i < p.len() && (p[i] == '?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

/// UTC 时间戳（ISO 8601）。不引 chrono：这里只需要一个能进报告的字符串。
fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 天数（1970-01-01 起）→ 年月日，Howard Hinnant 的 civil_from_days。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_prefix_wildcards() {
        assert!(glob_match("release/*", "release/1.4"));
        assert!(glob_match("release/*", "release/2026/11/hotfix"));
        assert!(!glob_match("release/*", "feature/x"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("main", "main"));
        assert!(!glob_match("main", "main-dev"));
        assert!(glob_match("v?", "v1"));
        assert!(!glob_match("v?", "v12"));
        assert!(glob_match("feature/*/fix", "feature/login/fix"));
    }

    #[test]
    fn civil_dates_are_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(18_262), (2020, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
    }

    #[test]
    fn path_key_normalizes_separators_and_case() {
        // 同一个目录可能被写成 / 或 \，也被写成不同大小写
        assert_eq!(path_key("D:/Projects/GitRepo"), path_key("D:\\Projects\\gitrepo"));
    }

    #[test]
    fn path_key_ignores_trailing_separator() {
        // 末尾多一个分隔符是最常见的重复来源：同一个仓库会被数成两个
        assert_eq!(path_key("D:/Projects/repo/"), path_key("D:\\Projects\\repo"));
        // 但盘符根要留着，不能修成 `C:`
        assert_eq!(path_key("C:\\"), "c:\\");
    }

    #[test]
    fn ref_like_values_are_checked() {
        let mut opts = Options::default();
        opts.base = Some("--all".to_string());
        assert!(validate(&opts).is_err());

        opts.base = Some("main".to_string());
        assert!(validate(&opts).is_ok());

        opts.base = None;
        opts.branch_allow = Some(vec!["release/*".to_string()]);
        assert!(validate(&opts).is_ok());

        opts.branch_allow = Some(vec!["-x".to_string()]);
        assert!(validate(&opts).is_err());

        opts.branch_allow = Some(vec!["a\"b".to_string()]);
        assert!(validate(&opts).is_err());
    }

    #[test]
    fn clip_keeps_short_text_and_truncates_long() {
        assert_eq!(clip("  short  ", 20), "short");
        assert_eq!(clip(&"字".repeat(10), 3), "字字字…");
    }

    #[test]
    fn severity_orders_errors_first() {
        let mut v = vec![Severity::Info, Severity::Error, Severity::Warn];
        v.sort();
        assert_eq!(v, vec![Severity::Error, Severity::Warn, Severity::Info]);
    }

    #[test]
    fn skip_is_case_insensitive() {
        let rules = Rules {
            skip: vec!["Stash".to_string(), "unmerged".to_string()],
            ..Default::default()
        };
        assert!(rules.skipped("stash"));
        assert!(rules.skipped("UNMERGED"));
        assert!(!rules.skipped("dirty"));
    }

    #[test]
    fn command_line_quotes_paths_with_spaces() {
        let opts = Options {
            roots: vec![PathBuf::from("D:/Projects/my work")],
            branch_allow: Some(vec!["main".to_string(), "release/*".to_string()]),
            base: Some("main".to_string()),
            skip: vec!["stash".to_string()],
            depth: Some(6),
            ..Default::default()
        };
        let cmd = command_line(&opts);
        assert!(cmd.contains("--root \"D:/Projects/my work\""), "{cmd}");
        assert!(cmd.contains("--depth 6"), "{cmd}");
        assert!(cmd.contains("--branch \"release/*\""), "{cmd}");
        assert!(cmd.contains("--skip \"stash\""), "{cmd}");
        assert!(cmd.ends_with("--fail-on error"), "{cmd}");
    }

    #[test]
    fn markdown_escapes_pipes() {
        assert_eq!(md_escape("a|b"), "a\\|b");
        assert_eq!(md_escape("x`y"), "x'y");
        // 文件名里可以有换行，不能让它把表格拆开
        assert_eq!(md_escape("a\nb\tc"), "a b c");
    }

    #[test]
    fn pad_pads_by_display_width() {
        assert_eq!(pad("api", 6), "api   ");
        // 中文占两格，补的空格要少一半
        assert_eq!(pad("仓库", 6), "仓库  ");
        assert_eq!(pad("一二三四五六七", 3), "一二三四五六七");
    }

    #[test]
    fn same_skip_ignores_order_and_case() {
        let a = vec!["stash".to_string(), "Behind".to_string()];
        let b = vec!["behind".to_string(), "STASH".to_string()];
        assert!(same_skip(&a, &b));
        assert!(!same_skip(&a, &["stash".to_string()]));
    }

    #[test]
    fn command_line_marks_no_config() {
        let opts = Options {
            roots: vec![PathBuf::from(".")],
            use_config: false,
            ..Default::default()
        };
        let cmd = command_line(&opts);
        assert!(cmd.contains("--no-config"), "{cmd}");
        // 没指定 depth 就别写死一个，否则会把配置文件里的值压掉
        assert!(!cmd.contains("--depth"), "{cmd}");
        assert!(!cmd.contains("--branch"), "{cmd}");
    }

    /// 造一个「只有 dirty 一条」的仓库结果，用来验证结论分档
    fn dirty_only(strict: bool) -> RepoAudit {
        let sev = if strict {
            Severity::Error
        } else {
            Severity::Warn
        };
        RepoAudit {
            name: "api".into(),
            path: "D:/w/api".into(),
            root: "D:/w".into(),
            branch: Some("main".into()),
            findings: vec![Finding::new("dirty", sev, Scope::Workspace, "有 1 处未提交改动")],
            config: None,
            effective_skip: vec![],
            error: None,
        }
    }

    #[test]
    fn dirty_alone_does_not_block_by_default() {
        // 这就是「全亮红」的根因：以前 dirty 是阻断，于是每台开发机都全红。
        let r = dirty_only(false);
        assert!(!r.failed());
        assert!(r.workspace_pending());
        assert!(r.workspace_note().contains("未提交"));
    }

    #[test]
    fn strict_mode_promotes_dirty_to_blocker() {
        let r = dirty_only(true);
        assert!(r.failed());
    }

    #[test]
    fn verdict_has_three_states() {
        // 没有 finding：通过
        let mut repos = vec![dirty_only(false)];
        repos[0].findings.clear();
        let base = Report {
            generated_at: String::new(),
            roots: vec![],
            depth: 1,
            base: None,
            branch_allow: vec![],
            skipped: vec![],
            strict: false,
            config_files: vec![],
            root_errors: vec![],
            repos,
            totals: Totals {
                repos: 1,
                passed: 1,
                ..Default::default()
            },
            verdict: Verdict::Passed,
            elapsed_ms: 0,
        };
        assert_eq!(base.verdict.word(), "通过");

        // 有提示、没阻断：注意（能发，但报告里要交代一句）
        assert_eq!(Verdict::Attention.word(), "注意");
        assert_eq!(Verdict::Blocked.word(), "未通过");

        // 根目录不可用：没有 error，但报告缺东西，不能算通过
        let mut broken_root = base;
        broken_root.root_errors = vec!["D:/nope：目录不存在".into()];
        assert!(!broken_root.passed(), "根目录不可用时不能算通过");
        assert_eq!(broken_root.totals.errors, 0);
        // 三种结论的字面不能重复，否则报告上分不出来
        let words = [Verdict::Passed.word(), Verdict::Attention.word(), Verdict::Blocked.word()];
        assert_eq!(
            words.iter().collect::<HashSet<_>>().len(),
            3,
            "三个结论的词要互不相同"
        );
    }

    #[test]
    fn strict_flag_lands_in_the_command_line() {
        let opts = Options {
            roots: vec![PathBuf::from(".")],
            strict: Some(true),
            ..Default::default()
        };
        assert!(command_line(&opts).contains("--strict"));
        // 没显式给就不写：写了会盖掉配置文件里的 strict
        let opts = Options {
            roots: vec![PathBuf::from(".")],
            ..Default::default()
        };
        assert!(!command_line(&opts).contains("--strict"));
    }
}
