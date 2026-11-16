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

use serde::{Deserialize, Serialize};

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
    /// 基线：上一次报告里的 (仓库, 规则) 集合。这次还出现的不算「新增」，
    /// 不计入结论——第一次接入全红的团队，第二次跑只看到新问题。
    pub baseline: Option<HashSet<String>>,
    /// 基线文件路径，只用来进报告头和拼等价命令行
    pub baseline_file: Option<String>,
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
            baseline: None,
            baseline_file: None,
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

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
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
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// 发出去的那个提交本身有毛病：冲突、半途的变基、分离 HEAD、没推、分支不对
    Release,
    /// 工作区里还有没提交的活：脏树、stash、落后远程、遗留分支
    Workspace,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Finding {
    pub code: String,
    pub severity: Severity,
    pub scope: Scope,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// 只在带基线跑时有值：这条是新问题，还是上一次就有的已知问题。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<BaselineState>,
}

/// 基线里就有 → `Known`（不计入结论）；这次新冒出来的 → `New`。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BaselineState {
    Known,
    New,
}

impl Finding {
    fn new(code: &str, severity: Severity, scope: Scope, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            severity,
            scope,
            message: message.into(),
            detail: None,
            baseline: None,
        }
    }

    fn with_detail(mut self, detail: Option<String>) -> Self {
        self.detail = detail;
        self
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
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
            .join(crate::i18n::t("；", "; "))
    }
}

/// 三个结论。之前只有「通过 / 未通过」两档，脏树一律算未通过，
/// 于是每台开发机都是全红；现在中间多一档「注意」：能发，但有活没提交。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
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
            Verdict::Passed => crate::i18n::t("通过", "Passed"),
            Verdict::Attention => crate::i18n::t("注意", "Attention"),
            Verdict::Blocked => crate::i18n::t("未通过", "Blocked"),
        }
    }
}

// 基线文件可能是老版本写的（没有 known_* / workspace 字段），
// 缺的字段按 0 补——已知项数本来就是「没有基线」的退化情形。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Totals {
    pub repos: usize,
    pub passed: usize,
    pub failed: usize,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    /// 有工作区待办的仓库数（这些仓库可能同时通过）
    pub workspace: usize,
    /// 基线里就有的（已知）阻断 / 提示数。不计入结论，只进报告。
    pub known_errors: usize,
    pub known_warnings: usize,
}

impl Totals {
    /// 本次新冒出来的阻断数：结论和退出码只看它。
    pub fn new_errors(&self) -> usize {
        self.errors.saturating_sub(self.known_errors)
    }
    pub fn new_warnings(&self) -> usize {
        self.warnings.saturating_sub(self.known_warnings)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
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
    /// 基线文件路径；没带基线跑就没有。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline_file: Option<String>,
    pub repos: Vec<RepoAudit>,
    pub totals: Totals,
    pub verdict: Verdict,
    pub elapsed_ms: u64,
}

impl Report {
    /// 退出码用这个：只看**新增**阻断项，已知项不影响「能不能发」。
    /// 根目录的问题基线救不了：它让报告缺东西，永远算不过。
    pub fn passed(&self) -> bool {
        self.totals.new_errors() == 0 && self.root_errors.is_empty()
    }

    pub fn verdict_word(&self) -> &'static str {
        self.verdict.word()
    }
}

/* ---------------------------------------------------------------- 执行 */

pub fn run(opts: &Options) -> Result<Report, String> {
    let started = Instant::now();
    if opts.roots.is_empty() {
        return Err(crate::i18n::t("没有指定要巡检的根目录", "No root directory given").to_string());
    }
    validate(opts)?;

    let mut roots: Vec<(PathBuf, Option<config::FileConfig>)> = Vec::new();
    let mut root_errors: Vec<String> = Vec::new();
    for r in &opts.roots {
        if !r.exists() {
            root_errors.push(crate::tr!("{p}：目录不存在", "{p}: no such directory", p = r.display()));
            continue;
        }
        if !r.is_dir() {
            root_errors.push(crate::tr!("{p}：不是一个目录", "{p}: not a directory", p = r.display()));
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
        return Err(root_errors.join(crate::i18n::t("；", "; ")));
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
                            note = Some(crate::tr!(
                                "{file} 里的 depth 在这里不生效（它只管从根目录往下找几层），其余设置照常",
                                "depth in {file} is ignored here (it only sets how deep to search from a root); everything else still applies",
                                file = p.display()
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

    // 基线：把上一次报告里就有的问题标成 Known，只数这次新冒出来的。
    // 这是「第一次接入全红」的解药：先存一份基线，之后门禁只盯新问题。
    if let Some(set) = &opts.baseline {
        let (ke, kw) = apply_baseline(&mut repos, set);
        totals.known_errors = ke;
        totals.known_warnings = kw;
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

    let verdict = if totals.new_errors() > 0 || !root_errors.is_empty() {
        Verdict::Blocked
    } else if totals.new_warnings() > 0 {
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
        baseline_file: opts.baseline_file.clone(),
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

/* ---------------------------------------------------------------- 基线 */

/// (仓库路径, 规则 code) 的匹配键。不用 message：脏树从 3 处变 5 处还是
/// 同一个问题，没必要算成「新增」吓人一跳。
fn baseline_key(repo_path: &str, code: &str) -> String {
    format!("{}|{}", path_key(repo_path), code.to_lowercase())
}

/// 给每个 finding 打上 Known / New，返回 (已知阻断数, 已知提示数)。
///
/// 标记只影响结论（verdict / 退出码），不会把已知项藏起来——报告里照样列，
/// 只是挂上「已知」的说明。门禁只盯新问题，但审计时历史一项不少。
fn apply_baseline(repos: &mut [RepoAudit], set: &HashSet<String>) -> (usize, usize) {
    let mut known_errors = 0;
    let mut known_warnings = 0;
    for repo in repos.iter_mut() {
        let rk = path_key(&repo.path);
        for f in repo.findings.iter_mut() {
            let known = set.contains(&baseline_key(&rk, &f.code));
            f.baseline = Some(if known {
                match f.severity {
                    Severity::Error => known_errors += 1,
                    Severity::Warn => known_warnings += 1,
                    Severity::Info => {}
                }
                BaselineState::Known
            } else {
                BaselineState::New
            });
        }
    }
    (known_errors, known_warnings)
}

/// 从上一次的 JSON 报告（`--format json --out xxx.json` 的产物）读出基线集合。
///
/// 故意容忍字段缺失：旧版本写的报告没有 `known_*` / `baseline_file`，
/// serde 的 `#[serde(default)]`……不需要——`Totals` 有 `Default` 且字段全有默认，
/// `Option` 字段缺了就是 `None`，天然向前兼容。解析不了就报错退出 2，
/// 不许静默当「没有基线」跑——那等于门禁悄悄变松。
pub fn load_baseline(text: &str) -> Result<HashSet<String>, String> {
    let report: Report = serde_json::from_str(text)
        .map_err(|e| crate::tr!("基线文件不是有效的巡检报告 JSON：{e}", "baseline file is not a valid audit report JSON: {e}", e = e))?;
    let mut set = HashSet::new();
    for repo in &report.repos {
        let rk = path_key(&repo.path);
        for f in &repo.findings {
            set.insert(baseline_key(&rk, &f.code));
        }
    }
    Ok(set)
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
            crate::i18n::t(
                "仓库级配置读不了，这条仓库按上一层规则跑的，结论不可信",
                "The repo-level config could not be read; this repo ran with the inherited rules, so the verdict is unreliable",
            )
            .to_string(),
        ).with_detail(Some(broken.clone())));
    }

    match git::status_lite(&path) {
        Err(e) => {
            error = Some(e.clone());
            findings.push(Finding::new(
                "read-error",
                Severity::Error,
                Scope::Release,
                crate::tr!("读不到仓库状态：{e}", "Cannot read repository status: {e}", e = e),
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
                    crate::tr!("有 {n} 个文件处于冲突状态", "{n} file(s) left in conflict", n = st.conflicted),
                ));
            }

            // 2. 半途而废的合并 / 变基
            if !rules.skipped("operation") {
                if let Some(op) = git_dir(&path).and_then(|d| operation_in_progress(&d)) {
                    findings.push(Finding::new(
                        "operation",
                        Severity::Error,
                        Scope::Release,
                        crate::tr!("{op} 还没做完，仓库停在中途状态", "{op} is unfinished; the repo is stuck mid-operation", op = op),
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
                        crate::tr!(
                            "有 {dirty} 处未提交改动（{staged} 暂存 / {unstaged} 未暂存 / {untracked} 未跟踪）",
                            "{dirty} uncommitted change(s) ({staged} staged / {unstaged} unstaged / {untracked} untracked)",
                            dirty = dirty,
                            staged = st.staged,
                            unstaged = st.unstaged,
                            untracked = st.untracked
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
                    crate::i18n::t("HEAD 已分离，不在任何分支上", "HEAD is detached; not on any branch").to_string(),
                ));
            }

            // 5. 未推送
            if st.ahead > 0 && !rules.skipped("unpushed") {
                let up = st.upstream.clone().unwrap_or_else(|| crate::i18n::t("上游", "upstream").to_string());
                findings.push(
                    Finding::new(
                        "unpushed",
                        Severity::Error,
                        Scope::Release,
                        crate::tr!("有 {n} 个提交还没推到 {up}", "{n} commit(s) not pushed to {up}", n = st.ahead, up = up),
                    )
                    .with_detail(Some(crate::i18n::t(
                        "别人拿到的代码里没有这些提交",
                        "Anyone else's copy is missing these commits",
                    ).to_string())),
                );
            }

            // 6. 落后远程：只是提示，拉一下就好
            if st.behind > 0 && !rules.skipped("behind") {
                findings.push(Finding::new(
                    "behind",
                    Severity::Warn,
                    Scope::Workspace,
                    crate::tr!("落后远程 {n} 个提交", "{n} commit(s) behind the remote", n = st.behind),
                ));
            }

            // 7. 没有上游：推不了，也说明这个分支是纯本地的
            if st.upstream.is_none() && !st.initial && !st.detached && !rules.skipped("no-upstream") {
                findings.push(Finding::new(
                    "no-upstream",
                    Severity::Warn,
                    Scope::Workspace,
                    crate::i18n::t("当前分支没有上游分支", "Current branch has no upstream").to_string(),
                ));
            }

            // 8. stash：临时改动忘在栈里，往往就是「发出去的版本少了个补丁」
            if st.stashes > 0 && !rules.skipped("stash") {
                findings.push(Finding::new(
                    "stash",
                    Severity::Warn,
                    Scope::Workspace,
                    crate::tr!("有 {n} 条 stash 没清理", "{n} stash entries left behind", n = st.stashes),
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
                            crate::tr!(
                                "当前分支 {b} 不在允许列表里（允许：{allow}）",
                                "Current branch {b} is not in the allow list (allowed: {allow})",
                                b = b,
                                allow = rules.branch_allow.join(" / ")
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
                    crate::i18n::t("没有配置远程仓库", "No remote configured").to_string(),
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
        // 已经列了 3 个，剩下的是「还有几个」，别把总数写成剩余数
        names.push(crate::tr!(
            "…等 {n} 个",
            "…and {n} more",
            n = st.entries.len() - 3
        ));
    }
    Some(names.join(crate::i18n::t("、", ", ")))
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

/// 一个分支名的几种可用写法：本地分支优先，其次才是远程跟踪引用。
///
/// CI 里查出来的工作区经常是 detached HEAD（tag 检出、PR 合并检出），仓库里
/// 只有 `origin/main` 而没有本地 `main`。只认本地分支的话，`base = "main"`
/// 会退化成「基准分支不存在，本条未生效」——门禁上一条虚设的检查，
/// 看着有、实际什么都没查。
fn base_refs(c: &str) -> Vec<String> {
    vec![
        c.to_string(),
        format!("origin/{c}"),
        format!("refs/remotes/origin/{c}"),
    ]
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
        for r in base_refs(&c) {
            let spec = format!("{r}^{{commit}}");
            let ok = git::git_lines(repo, &["rev-parse", "--verify", "--quiet", &spec])
                .map(|l| !l.is_empty())
                .unwrap_or(false);
            if ok {
                // 返回真正解析到的那个写法：后面 branch --no-merged 要能用它，
                // 报告里也该写明查的是 origin/main 而不是 main。
                return Some(r);
            }
        }
    }
    None
}

fn unmerged_finding(repo: &Path, prefer: Option<&str>, current: Option<&str>) -> Option<Finding> {
    let base = match resolve_base(repo, prefer) {
        Some(b) => b,
        // 显式指定了基准分支，但本地和 origin 下都没有它：不能装作查过了。
        // 自动探测失败时（仓库只有一个分支、也没有 main/master/develop）不算问题。
        None => {
            return prefer.map(|b| {
                Finding::new(
                    "unmerged",
                    Severity::Warn,
                    Scope::Workspace,
                    crate::tr!("基准分支 {b} 在本地和 origin 下都不存在，本条未生效", "Base branch {b} exists neither locally nor on origin; this rule did not run", b = b),
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
                crate::tr!("查不到未合并分支：{e}", "Cannot list unmerged branches: {e}", e = clip(&e, 120)),
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
        shown.push(crate::tr!(
            "…等 {n} 个",
            "…and {n} more",
            n = rest.len() - 5
        ));
    }
    Some(
        Finding::new(
            "unmerged",
            Severity::Warn,
            Scope::Workspace,
            crate::tr!("有 {n} 个本地分支没合并进 {base}", "{n} local branch(es) not merged into {base}", n = rest.len(), base = base),
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

/* ---------------------------------------------------------------- SARIF */

/// SARIF 2.1.0（GitHub code scanning 用的格式）。规则表：id + 一句话说明，
/// `properties` 里带分组和默认级别，code scanning 的 UI 都能读。
fn sarif_rules() -> Vec<serde_json::Value> {
    const RULES: &[(&str, &str, &str, &str)] = &[
        ("conflict", "Files left in conflict state", "release", "error"),
        ("operation", "A merge / rebase / cherry-pick / revert / bisect is in progress", "release", "error"),
        ("detached", "HEAD is detached from any branch", "release", "error"),
        ("unpushed", "Commits not pushed to upstream", "release", "error"),
        ("branch", "Current branch is not in the allow list", "release", "error"),
        ("config-error", "Repo-level .gittracer.toml failed to parse", "release", "error"),
        ("read-error", "Cannot read repository status", "release", "error"),
        ("dirty", "Uncommitted changes in the working tree", "workspace", "warning"),
        ("behind", "Local branch is behind its remote", "workspace", "warning"),
        ("no-upstream", "Current branch has no upstream", "workspace", "warning"),
        ("stash", "Stash entries left behind", "workspace", "warning"),
        ("unmerged", "Local branches not merged into the base branch", "workspace", "warning"),
        ("config-note", "Repo-level depth key ignored", "release", "note"),
        ("no-remote", "No remote configured", "release", "note"),
    ];
    RULES
        .iter()
        .map(|(id, desc, scope, level)| {
            serde_json::json!({
                "id": id,
                "shortDescription": { "text": desc },
                "properties": { "scope": scope, "defaultLevel": level },
            })
        })
        .collect()
}

/// 渲染成 SARIF 2.1.0。这是把巡检结果送进 GitHub「Code scanning」标签页的
/// 那张门票：`--format sarif --out audit.sarif` 后用
/// `github/codeql-action/upload-sarif` 上传，仓库页上就能看到。
/// 基线里已知的项会带上 suppressions（code scanning 默认折叠已抑制项）。
pub fn render_sarif(report: &Report) -> String {
    let rules = sarif_rules();
    let index_of = |code: &str| rules.iter().position(|r| r["id"] == code);

    let mut results: Vec<serde_json::Value> = Vec::new();
    for repo in &report.repos {
        for f in &repo.findings {
            let Some(rule_index) = index_of(&f.code) else { continue };
            let level = match f.severity {
                Severity::Error => "error",
                Severity::Warn => "warning",
                Severity::Info => "note",
            };
            // message 里带上仓库和分支：code scanning 列表页只显示 message，
            // 不带仓库名的话「3 条 dirty」分不清是谁家的。
            let mut text = crate::tr!(
                "{msg} · 仓库 {repo}@{branch}",
                "{msg} · repo {repo}@{branch}",
                msg = f.message,
                repo = repo.name,
                branch = repo.branch.as_deref().unwrap_or("detached")
            );
            if let Some(d) = &f.detail {
                text.push_str(&format!(" · {d}"));
            }
            let mut result = serde_json::json!({
                "ruleId": f.code,
                "ruleIndex": rule_index,
                "level": level,
                "message": { "text": text },
                "locations": [{
                    "physicalLocation": {
                        "artifactLocation": { "uri": repo.path }
                    }
                }],
                "partialFingerprints": {
                    "gittracer/v1": baseline_key(&repo.path, &f.code)
                },
            });
            if f.baseline == Some(BaselineState::Known) {
                result["suppressions"] = serde_json::json!([{
                    "kind": "external",
                    "status": "accepted",
                    "justification": "Known issue present in the baseline report"
                }]);
            }
            results.push(result);
        }
    }

    let sarif = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "gittracer-audit",
                    "version": env!("CARGO_PKG_VERSION"),
                    "informationUri": "https://github.com/haoguan2013/gittracer",
                    "rules": rules,
                }
            },
            "invocations": [{
                "executionSuccessful": report.passed(),
                "endTimeUtc": report.generated_at,
            }],
            "results": results,
        }]
    });
    let mut s = serde_json::to_string_pretty(&sarif).unwrap_or_else(|_| "{}".to_string());
    s.push('\n');
    s
}

pub fn render_text(report: &Report) -> String {
    let mut out = String::new();
    let sep = crate::i18n::t("；", "; ");
    out.push_str(&crate::tr!(
        "Git Tracer 巡检  {ts}",
        "Git Tracer audit  {ts}",
        ts = report.generated_at
    ));
    out.push('\n');
    out.push_str(&crate::tr!(
        "根目录  {v}",
        "roots   {v}",
        v = report.roots.join(sep)
    ));
    out.push('\n');
    out.push_str(&crate::tr!("深度    {v}", "depth   {v}", v = report.depth));
    out.push('\n');
    out.push_str(&crate::tr!("基准    {v}", "base    {v}", v = base_text(report)));
    out.push('\n');
    out.push_str(&crate::tr!("模式    {v}", "mode    {v}", v = mode_text(report)));
    out.push('\n');
    if let Some(b) = &report.baseline_file {
        out.push_str(&crate::tr!(
            "基线    {b}（已知项不计入结论，只盯新问题）",
            "baseline {b}  (known issues are listed but not counted; the gate watches new ones)",
            b = b
        ));
        out.push('\n');
    }
    // 用不了的根目录放在最前面说：它会让整份报告缺东西，不能藏在结尾
    for e in &report.root_errors {
        out.push_str(&crate::tr!(
            "不可用  {e}（已跳过，其余照常）",
            "unusable {e}  (skipped, everything else still ran)",
            e = e
        ));
        out.push('\n');
    }
    if !report.branch_allow.is_empty() {
        out.push_str(&crate::tr!(
            "允许分支  {v}",
            "branches {v}",
            v = report.branch_allow.join(" / ")
        ));
        out.push('\n');
    }
    if !report.skipped.is_empty() {
        out.push_str(&crate::tr!(
            "跳过规则  {v}",
            "skipped  {v}",
            v = report.skipped.join(", ")
        ));
        out.push('\n');
    }
    if !report.config_files.is_empty() {
        out.push_str(&crate::tr!(
            "配置      {n} 个 .gittracer.toml",
            "config   {n} .gittracer.toml file(s)",
            n = report.config_files.len()
        ));
        out.push('\n');
    }
    out.push('\n');

    if report.repos.is_empty() {
        out.push_str(crate::i18n::t(
            "没有找到任何 Git 仓库。\n",
            "No Git repositories found.\n",
        ));
    }

    for repo in &report.repos {
        let mark = if repo.failed() { "FAIL" } else { "PASS" };
        let branch = repo.branch.as_deref().unwrap_or("(detached)");
        out.push_str(&format!("{}  {}  <{}>\n", mark, pad(&repo.name, 26), branch));
        // 只在这个仓库和全局默认不一样时才单独列，否则 200 个仓库全在刷同一行
        if let Some(c) = &repo.config {
            out.push_str(&crate::tr!("      配置    {c}", "      config  {c}", c = c));
            out.push('\n');
        }
        if !same_skip(&repo.effective_skip, &report.skipped) {
            out.push_str(&crate::tr!(
                "      跳过    {v}",
                "      skip    {v}",
                v = repo.effective_skip.join(", ")
            ));
            out.push('\n');
        }
        for f in &repo.findings {
            // 已知项不藏起来，只是挂上说明：门禁盯新问题，审计要看到全部
            let known = if f.baseline == Some(BaselineState::Known) {
                crate::i18n::t("（已知）", " (known)")
            } else {
                ""
            };
            out.push_str(&format!(
                "      {:<5}  {:<12} {}{known}\n",
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
    out.push_str(&crate::tr!(
        "{repos} 个仓库：{failed} 未通过 / {passed} 通过 · {errors} 阻断 / {warn} 提示 · 结论：{verdict} · {secs}s",
        "{repos} repo(s): {failed} failed / {passed} passed · {errors} blocking / {warn} hint(s) · verdict: {verdict} · {secs}s",
        repos = t.repos,
        failed = t.failed,
        passed = t.passed,
        errors = t.errors,
        warn = t.warnings + t.infos,
        verdict = report.verdict_word(),
        secs = format!("{:.1}", report.elapsed_ms as f64 / 1000.0)
    ));
    out.push('\n');
    // 全红问题的另一半：就算都没有阻断项，也要让人一眼看到「有活没提交」，
    // 否则会以为报告漏了东西。
    if t.workspace > 0 {
        let note = if report.strict {
            crate::i18n::t("严格模式下已计入阻断", "counted as blocking under strict mode")
        } else {
            crate::i18n::t("--strict 时算阻断", "blocking only with --strict")
        };
        out.push_str(&crate::tr!(
            "工作区  {n} 个仓库有待提交 / stash / 落后远程（只提示，{note}）",
            "workspace {n} repo(s) with uncommitted work / stash / behind remote (hints only, {note})",
            n = t.workspace,
            note = note
        ));
        out.push('\n');
    }
    if report.baseline_file.is_some() {
        out.push_str(&crate::tr!(
            "基线    新增 {ne} 阻断 / {nw} 提示 · 已知 {ke} / {kw} 不计入结论",
            "baseline {ne} new blocking / {nw} new hint(s) · {ke} / {kw} known and not counted",
            ne = t.new_errors(),
            nw = t.new_warnings(),
            ke = t.known_errors,
            kw = t.known_warnings
        ));
        out.push('\n');
    }
    out
}

pub fn render_markdown(report: &Report) -> String {
    let t = &report.totals;
    let mut out = String::new();
    out.push_str(crate::i18n::t(
        "# Git Tracer 巡检报告\n\n",
        "# Git Tracer audit report\n\n",
    ));
    out.push_str(crate::i18n::t(
        "| 项 | 值 |\n| --- | --- |\n",
        "| Item | Value |\n| --- | --- |\n",
    ));
    out.push_str(&crate::tr!(
        "| 时间 | {v} |\n",
        "| Generated | {v} |\n",
        v = report.generated_at
    ));
    out.push_str(&crate::tr!(
        "| 根目录 | {v} |\n",
        "| Roots | {v} |\n",
        v = code_join(&report.roots)
    ));
    out.push_str(&crate::tr!(
        "| 扫描深度 | {v} |\n",
        "| Scan depth | {v} |\n",
        v = report.depth
    ));
    out.push_str(&crate::tr!(
        "| 基准分支 | {v} |\n",
        "| Base branch | {v} |\n",
        v = base_text(report)
    ));
    out.push_str(&crate::tr!(
        "| 检查模式 | {v} |\n",
        "| Mode | {v} |\n",
        v = mode_text(report)
    ));
    if let Some(b) = &report.baseline_file {
        out.push_str(&crate::tr!(
            "| 基线 | `{b}`（已知项不计入结论） |\n",
            "| Baseline | `{b}` (known issues are not counted) |\n",
            b = md_escape(b)
        ));
    }
    if !report.branch_allow.is_empty() {
        out.push_str(&crate::tr!(
            "| 允许分支 | {v} |\n",
            "| Allowed branches | {v} |\n",
            v = code_join(&report.branch_allow)
        ));
    }
    if !report.skipped.is_empty() {
        out.push_str(&crate::tr!(
            "| 跳过规则 | {v} |\n",
            "| Skipped rules | {v} |\n",
            v = report.skipped.join(", ")
        ));
    }
    if !report.config_files.is_empty() {
        out.push_str(&crate::tr!(
            "| 配置文件 | {n} 个 `.gittracer.toml` |\n",
            "| Config files | {n} `.gittracer.toml` |\n",
            n = report.config_files.len()
        ));
    }
    for e in &report.root_errors {
        out.push_str(&crate::tr!(
            "| 跳过的根目录 | {e} |\n",
            "| Skipped roots | {e} |\n",
            e = md_escape(e)
        ));
    }
    out.push_str(&crate::tr!(
        "| 结论 | **{v}** |\n",
        "| Verdict | **{v}** |\n",
        v = report.verdict_word()
    ));
    out.push('\n');
    out.push_str(&crate::tr!(
        "{repos} 个仓库：{failed} 未通过 / {passed} 通过 · {errors} 阻断 / {warn} 提示 · {secs}s\n\n",
        "{repos} repo(s): {failed} failed / {passed} passed · {errors} blocking / {warn} hint(s) · {secs}s\n\n",
        repos = t.repos,
        failed = t.failed,
        passed = t.passed,
        errors = t.errors,
        warn = t.warnings + t.infos,
        secs = format!("{:.1}", report.elapsed_ms as f64 / 1000.0)
    ));
    if report.baseline_file.is_some() {
        out.push_str(&crate::tr!(
            "结论只看新增：**{ne} 新增阻断 / {nw} 新增提示**；另有已知 {ke} / {kw} 项在基线里，不计入。\n\n",
            "Only new findings count: **{ne} new blocking / {nw} new hint(s)**; another {ke} / {kw} known item(s) sit in the baseline and are not counted.\n\n",
            ne = t.new_errors(),
            nw = t.new_warnings(),
            ke = t.known_errors,
            kw = t.known_warnings
        ));
    }

    let failed: Vec<&RepoAudit> = report.repos.iter().filter(|r| r.failed()).collect();
    let passed: Vec<&RepoAudit> = report.repos.iter().filter(|r| !r.failed()).collect();

    if failed.is_empty() {
        out.push_str(crate::i18n::t(
            "## 需要处理\n\n没有阻断项。\n\n",
            "## Needs attention\n\nNo blocking findings.\n\n",
        ));
    } else {
        out.push_str(&crate::tr!(
            "## 需要处理（{n}）\n\n",
            "## Needs attention ({n})\n\n",
            n = failed.len()
        ));
        for repo in failed {
            out.push_str(&crate::tr!(
                "### {name} `{branch}`\n\n`{path}`\n\n",
                "### {name} `{branch}`\n\n`{path}`\n\n",
                name = md_escape(&repo.name),
                branch = md_escape(repo.branch.as_deref().unwrap_or("detached")),
                path = md_escape(&repo.path)
            ));
            // 结论是被本地配置放宽过的，必须写出来，不然这份报告不能当证据
            if let Some(c) = &repo.config {
                out.push_str(&crate::tr!(
                    "依据仓库配置：`{c}`\n\n",
                    "Repo config in effect: `{c}`\n\n",
                    c = md_escape(c)
                ));
            }
            if !same_skip(&repo.effective_skip, &report.skipped) {
                out.push_str(&crate::tr!(
                    "本仓库跳过：{v}\n\n",
                    "Skipped in this repo: {v}\n\n",
                    v = md_escape(&repo.effective_skip.join(", "))
                ));
            }
            out.push_str(crate::i18n::t(
                "| 级别 | 规则 | 说明 |\n| --- | --- | --- |\n",
                "| Level | Rule | Message |\n| --- | --- | --- |\n",
            ));
            for f in &repo.findings {
                let known = if f.baseline == Some(BaselineState::Known) {
                    crate::i18n::t("（已知，不计入结论）", " (known, not counted)")
                } else {
                    ""
                };
                out.push_str(&format!(
                    "| {} | {} | {}{known} |\n",
                    f.severity.word(),
                    md_escape(&f.code),
                    md_escape(&f.message)
                ));
            }
            out.push('\n');
            for f in &repo.findings {
                if let Some(d) = &f.detail {
                    out.push_str(&crate::tr!(
                        "- {code}：{detail}\n",
                        "- {code}: {detail}\n",
                        code = md_escape(&f.code),
                        detail = md_escape(d)
                    ));
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
        out.push_str(&crate::tr!(
            "## 工作区（{n}，不计入结论）\n\n\
             这些仓库有未提交改动 / stash / 落后远程。不影响「能不能发」，\
             但如果你是从工作区直接打包的，用 `--strict` 再跑一遍。\n\n\
             | 仓库 | 分支 | 待处理 |\n| --- | --- | --- |\n",
            "## Workspace ({n}, not counted)\n\n\
             These repos have uncommitted changes / stash / commits behind the remote. \
             They do not affect whether you can ship, but if you package straight from \
             the working tree, re-run with `--strict`.\n\n\
             | Repo | Branch | Pending |\n| --- | --- | --- |\n",
            n = workspace.len()
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
        out.push_str(&crate::tr!(
            "## 已通过（{n}）\n\n",
            "## Passed ({n})\n\n",
            n = passed.len()
        ));
        for repo in passed {
            let mut tag = String::new();
            if repo.config.is_some() {
                tag.push_str(crate::i18n::t(" · 依据仓库配置", " · repo config"));
            }
            // 靠跳过规则才过的仓库，不能只写「通过」——报告要能自证
            if !same_skip(&repo.effective_skip, &report.skipped) {
                tag.push_str(&crate::tr!(
                    " · 跳过 {v}",
                    " · skipped {v}",
                    v = md_escape(&repo.effective_skip.join(", "))
                ));
            }
            // 脏着过的仓库要标出来：不然「全部通过」会被读成「什么都不用管」
            if repo.workspace_pending() {
                tag.push_str(crate::i18n::t(" · 工作区有活", " · workspace pending"));
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
    if let Some(b) = &opts.baseline_file {
        parts.push(format!("--baseline \"{b}\""));
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
        None => crate::i18n::t(
            "自动探测（main / master / develop）",
            "auto (main / master / develop)",
        )
        .to_string(),
    }
}

/// 两种模式的区别就一句话：脏树算不算阻断。
fn mode_text(report: &Report) -> String {
    if report.strict {
        crate::i18n::t(
            "严格（未提交改动算阻断）",
            "strict (uncommitted changes block)",
        )
        .to_string()
    } else {
        crate::i18n::t("默认（未提交改动只提示）", "default (uncommitted changes are hints)")
            .to_string()
    }
}

fn code_join(list: &[String]) -> String {
    list.iter()
        .map(|s| format!("`{}`", md_escape(s)))
        .collect::<Vec<_>>()
        .join(crate::i18n::t("、", ", "))
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
            baseline_file: None,
            repos,
            totals: Totals {
                repos: 1,
                passed: 1,
                ..Default::default()
            },
            verdict: Verdict::Passed,
            elapsed_ms: 0,
        };
        // 断言跟着当前语言走：这两个词是给人看的，随语言设置变
        assert_eq!(base.verdict.word(), crate::i18n::t("通过", "Passed"));

        // 有提示、没阻断：注意（能发，但报告里要交代一句）
        assert_eq!(Verdict::Attention.word(), crate::i18n::t("注意", "Attention"));
        assert_eq!(Verdict::Blocked.word(), crate::i18n::t("未通过", "Blocked"));

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

    /* ---------------- 基线 ---------------- */

    /// 两个仓库各一条 finding，基线只认识其中一个仓库的 dirty
    #[test]
    fn baseline_marks_known_findings() {
        let mut a = dirty_only(false);
        a.path = "D:/w/api".into();
        let mut b = dirty_only(true); // strict 下 dirty 是阻断
        b.path = "D:/w/beta".into();

        let mut set = HashSet::new();
        // 故意换大小写：基线键的归一化和去重键是同一套
        set.insert(baseline_key("D:/W/API", "dirty"));
        let mut repos = [a, b];
        let (ke, kw) = apply_baseline(&mut repos, &set);

        assert_eq!((ke, kw), (0, 1), "api 的 dirty 是提示级、在基线里");
        // beta 的 dirty 是阻断、不在基线里 → New
        // api 的 dirty 在基线里 → Known
        assert_eq!(repos[0].findings[0].baseline, Some(BaselineState::Known));
        assert_eq!(repos[1].findings[0].baseline, Some(BaselineState::New));
    }

    #[test]
    fn known_blockers_do_not_fail_the_gate() {
        // 全是已知项：门禁放行
        let mut t = Totals { errors: 2, warnings: 1, ..Default::default() };
        t.known_errors = 2;
        t.known_warnings = 1;
        assert_eq!(t.new_errors(), 0);
        let report = Report {
            generated_at: String::new(),
            roots: vec![],
            depth: 1,
            base: None,
            branch_allow: vec![],
            skipped: vec![],
            strict: false,
            config_files: vec![],
            root_errors: vec![],
            baseline_file: Some("baseline.json".into()),
            repos: vec![],
            totals: t,
            verdict: Verdict::Passed,
            elapsed_ms: 0,
        };
        assert!(report.passed(), "两个都是已知项，不该拦");
        // 混进一个新阻断：拦下
        let mut t2 = report.totals.clone();
        t2.errors = 3;
        let mut r2 = report;
        r2.totals = t2;
        assert!(!r2.passed(), "第三个是新的，必须拦");
    }

    #[test]
    fn baseline_roundtrip_through_json() {
        let repo = dirty_only(false);
        let report = Report {
            generated_at: "2026-11-16T00:00:00Z".into(),
            roots: vec!["D:/w".into()],
            depth: 1,
            base: None,
            branch_allow: vec![],
            skipped: vec![],
            strict: false,
            config_files: vec![],
            root_errors: vec![],
            baseline_file: None,
            repos: vec![repo],
            totals: Totals::default(),
            verdict: Verdict::Attention,
            elapsed_ms: 0,
        };
        let json = render_json(&report);
        let set = load_baseline(&json).expect("自己刚写的 JSON 一定读得回");
        assert!(set.contains(&baseline_key("D:/w/api", "dirty")));
        // 大小写不同也认得：键的归一化和报告里存的是同一套
        assert!(set.contains(&baseline_key("D:/W/API", "DIRTY")));
        // 不是合法 JSON 的文件要报错，不能静默当「没有基线」
        assert!(load_baseline("not json").is_err());
    }

    #[test]
    fn baseline_in_command_line() {
        let opts = Options {
            roots: vec![PathBuf::from(".")],
            baseline_file: Some("baseline.json".into()),
            ..Default::default()
        };
        let cmd = command_line(&opts);
        assert!(cmd.contains("--baseline \"baseline.json\""), "{cmd}");
    }

    /* ---------------- SARIF ---------------- */

    #[test]
    fn sarif_lists_findings_with_matching_rules() {
        let repo = dirty_only(true);
        let report = Report {
            generated_at: "2026-11-16T00:00:00Z".into(),
            roots: vec!["D:/w".into()],
            depth: 1,
            base: None,
            branch_allow: vec![],
            skipped: vec![],
            strict: true,
            config_files: vec![],
            root_errors: vec![],
            baseline_file: None,
            repos: vec![repo],
            totals: Totals { errors: 1, ..Default::default() },
            verdict: Verdict::Blocked,
            elapsed_ms: 0,
        };
        let v: serde_json::Value =
            serde_json::from_str(&render_sarif(&report)).expect("SARIF 必须是合法 JSON");
        assert_eq!(v["version"], "2.1.0");
        assert_eq!(v["runs"][0]["tool"]["driver"]["name"], "gittracer-audit");
        let results = &v["runs"][0]["results"];
        assert_eq!(results.as_array().map(|a| a.len()), Some(1));
        assert_eq!(results[0]["ruleId"], "dirty");
        assert_eq!(results[0]["level"], "error");
        // ruleIndex 必须真指到 ruleId 那一项，code scanning 会按它取规则
        let idx = results[0]["ruleIndex"].as_u64().unwrap() as usize;
        let rules = &v["runs"][0]["tool"]["driver"]["rules"];
        assert_eq!(rules[idx]["id"], "dirty");
        assert!(results[0]["message"]["text"]
            .as_str()
            .unwrap()
            .contains("api"), "列表页只显示 message，得带上仓库名");
        assert_eq!(
            v["runs"][0]["invocations"][0]["executionSuccessful"], false,
            "有阻断时 executionSuccessful 必须是 false"
        );
    }

    #[test]
    fn sarif_suppresses_known_findings() {
        let mut repo = dirty_only(false);
        repo.findings[0].baseline = Some(BaselineState::Known);
        let report = Report {
            generated_at: String::new(),
            roots: vec![],
            depth: 1,
            base: None,
            branch_allow: vec![],
            skipped: vec![],
            strict: false,
            config_files: vec![],
            root_errors: vec![],
            baseline_file: Some("b.json".into()),
            repos: vec![repo],
            totals: Totals::default(),
            verdict: Verdict::Passed,
            elapsed_ms: 0,
        };
        let v: serde_json::Value =
            serde_json::from_str(&render_sarif(&report)).expect("SARIF 必须是合法 JSON");
        let r = &v["runs"][0]["results"][0];
        assert!(r.get("suppressions").is_some(), "已知项要带 suppressions");
        assert_eq!(r["suppressions"][0]["kind"], "external");
    }

    // ---------------------------------------------------------------- 基准分支

    /// 造一个真仓库。CI 的早退保护：没装 git 就跳过，不要把环境问题算成测试失败。
    fn repo_with(name: &str, script: &[&[&str]]) -> Option<PathBuf> {
        let dir = std::env::temp_dir().join(format!("gt-base-{}-{}", std::process::id(), name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).ok()?;
        for args in script {
            let out = std::process::Command::new("git")
                .args(*args)
                .current_dir(&dir)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .output()
                .ok()?;
            if !out.status.success() {
                return None; // 没装 git 或者这版 git 不认参数 —— 跳过
            }
        }
        Some(dir)
    }

    #[test]
    fn base_prefers_local_branch() {
        let dir = match repo_with(
            "local",
            &[
                &["init", "-q"],
                &["commit", "-q", "--allow-empty", "-m", "init"],
            ],
        ) {
            Some(d) => d,
            None => return,
        };
        assert!(git::git_lines(&dir, &["branch", "-M", "main"]).is_ok());
        assert_eq!(resolve_base(&dir, Some("main")).as_deref(), Some("main"));
    }

    #[test]
    fn base_falls_back_to_remote_tracking_ref() {
        // CI 里的样子：detached HEAD、没有本地 main，只有 origin/main
        let dir = match repo_with(
            "origin",
            &[
                &["init", "-q"],
                &["commit", "-q", "--allow-empty", "-m", "init"],
                &["branch", "-M", "main"],
                &["update-ref", "refs/remotes/origin/main", "HEAD"],
                &["checkout", "-q", "--detach"],
                &["branch", "-D", "main"],
            ],
        ) {
            Some(d) => d,
            None => return,
        };
        assert_eq!(
            resolve_base(&dir, Some("main")).as_deref(),
            Some("origin/main"),
            "本地没有 main 时应该退到 origin/main"
        );
        // 退过去之后这条检查要真的跑起来，而不是留一条「本条未生效」
        assert!(
            unmerged_finding(&dir, Some("main"), Some("detached")).is_none(),
            "基准分支能从 origin 解析出来时，不该再报未生效"
        );
    }
}
