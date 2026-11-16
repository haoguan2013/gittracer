//! `gittracer-audit` —— 命令行巡检入口。
//!
//! 编在 `git-tracer-core` 里（不依赖 tauri）：这个程序要进 CI —— Linux runner、
//! 无界面的服务器、别人仓库的 workflow——在那些建地方装一整套 webkit2gtk
//! 既不现实也没道理。
//!
//! 逻辑全在 `git_tracer_core::audit` 里，这里只做参数解析、输出和退出码。
//! 用户可见的文案走 `i18n`：`--lang` 指语言，不指就是环境变量，再没有就中文。

use std::io::Write;
use std::process::ExitCode;

use git_tracer_core::audit;
use git_tracer_core::i18n::{self, Lang};
use git_tracer_core::tr;

const USAGE_ZH: &str = r#"gittracer-audit —— 发版 / 交付前的多仓库巡检（只读）

用法：
  gittracer-audit [目录...] [选项]

选项：
  --root <目录>      要巡检的根目录，可重复；也可以直接写成位置参数
  --depth <N>        向下找仓库的层数（1~16）；不写就用配置文件里的，再没有就是 4
  --branch <模式>    允许的分支，可重复或逗号分隔；支持 * 通配，如 "release/*"
  --base <分支>      未合并分支检查的基准，默认按 main / master / develop 自动探测
  --skip <规则>      跳过某条规则，可重复或逗号分隔
                     可选：dirty conflict operation detached unpushed behind
                           no-upstream stash branch unmerged no-remote
  --strict           未提交改动也算阻断（从工作区直接打包发版时用这个）
  --no-strict        关掉配置文件里写的 strict
  --no-config        不读目录里的 .gittracer.toml
  --baseline <文件>  上一次巡检的 JSON 报告（--format json --out 存的那种）。
                     之后结论只看**新增**问题：第一次接入跑出来全红不稀奇，
                     存一份基线，门禁从下次起只盯新冒出来的。
  --format <格式>    text（默认）/ md / json / sarif / gitlab
                     sarif 可用 github/codeql-action/upload-sarif 传到
                     仓库的 Code scanning 页
                     gitlab 是 GitLab Code Quality（CodeClimate）格式：
                     --out gl-code-quality-report.json，
                     再在 .gitlab-ci.yml 里声明 artifacts:reports:codequality
                     路径是相对项目根目录的，仓库级结论挂在仓库目录上
  --out <文件>       写到文件；"-" 表示标准输出
  --fail-on <级别>   error（默认）/ warn / never —— 决定退出码
  --lang <语言>      zh / en；不写就看 GITTRACER_LANG、LC_ALL、LANG，再没有就 zh
  --quiet            不在 stderr 打印摘要
  -h, --help         显示这段帮助
  -V, --version      显示版本

规则文件：
  <根目录>/.gittracer.toml   管这个目录下的所有仓库，还能定 depth
  <仓库>/.gittracer.toml     只覆盖这一个仓库
  写法和优先级见 README。命令行显式给的参数优先于文件。

退出码：
  0  没有达到 --fail-on 的问题
  1  有达到 --fail-on 的问题
  2  参数错误 / 目录不存在 / 配置文件有错 / 找不到 git

示例：
  gittracer-audit --root D:\work --branch "main,release/*" --base main
  gittracer-audit . --strict      # 从工作区直接打包，脏树也算不能发
  gittracer-audit . --format md --out audit.md --fail-on warn
  gittracer-audit . --format json --out baseline.json --fail-on never   # 存基线
  gittracer-audit . --baseline baseline.json    # 从此只拦新增
  gittracer-audit . --format gitlab --out gl-code-quality-report.json --fail-on never
"#;

const USAGE_EN: &str = r#"gittracer-audit — pre-release audit across many repositories (read-only)

Usage:
  gittracer-audit [directory...] [options]

Options:
  --root <dir>       Root directory to audit; repeatable, or pass it positionally
  --depth <N>        How many levels down to look for repos (1-16); defaults to the
                     config file value, then to 4
  --branch <glob>    Allowed branches, repeatable or comma separated; * wildcards
                     work, e.g. "release/*"
  --base <branch>    Base for the unmerged-branch check; auto-detects
                     main / master / develop when omitted
  --skip <rule>      Skip a rule, repeatable or comma separated. Available:
                     dirty conflict operation detached unpushed behind
                     no-upstream stash branch unmerged no-remote
  --strict           Treat uncommitted changes as blocking (use this when you
                     package straight from the working tree)
  --no-strict        Ignore strict = true from the config file
  --no-config        Do not read .gittracer.toml files
  --baseline <file>  A previous JSON report (written with --format json --out).
                     From then on only **new** findings count: a first run that is
                     all red is normal, so snapshot it and let the gate watch what
                     shows up next.
  --format <fmt>     text (default) / md / json / sarif / gitlab
                     sarif can be uploaded with github/codeql-action/upload-sarif
                     and shows up under the repo's Code scanning tab
                     gitlab is the Code Quality (CodeClimate) report: write it
                     with --out gl-code-quality-report.json and declare it as
                     artifacts:reports:codequality in .gitlab-ci.yml.
                     Paths are relative to the project root; repo-level
                     findings are anchored on the repository directory.
  --out <file>       Write to a file; "-" means stdout
  --fail-on <level>  error (default) / warn / never — decides the exit code
  --lang <lang>      zh / en; defaults to GITTRACER_LANG, LC_ALL, LANG, then zh
  --quiet            Do not print the summary on stderr
  -h, --help         Show this help
  -V, --version      Show the version

Config files:
  <root>/.gittracer.toml    applies to every repo under that root, sets depth
  <repo>/.gittracer.toml    overrides for that one repo
  See the README for precedence. Explicit command-line flags win over files.

Exit codes:
  0  nothing reached the --fail-on threshold
  1  something reached the --fail-on threshold
  2  bad arguments / missing directory / broken config / git not found

Examples:
  gittracer-audit --root D:\work --branch "main,release/*" --base main
  gittracer-audit . --strict      # packaging from the working tree
  gittracer-audit . --format md --out audit.md --fail-on warn
  gittracer-audit . --format json --out baseline.json --fail-on never   # snapshot
  gittracer-audit . --baseline baseline.json    # only new findings from now on
  gittracer-audit . --format gitlab --out gl-code-quality-report.json --fail-on never
"#;

fn usage() -> &'static str {
    i18n::t(USAGE_ZH, USAGE_EN)
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Text,
    Markdown,
    Json,
    Sarif,
    /// GitLab Code Quality（CodeClimate 的报告格式，字段更少）
    Gitlab,
}

#[derive(Clone, Copy, PartialEq)]
enum FailOn {
    Error,
    Warn,
    Never,
}

struct Cli {
    opts: audit::Options,
    format: Format,
    out: Option<String>,
    fail_on: FailOn,
    quiet: bool,
}

fn main() -> ExitCode {
    let _cp = setup_console();

    let args: Vec<String> = std::env::args().skip(1).collect();
    // 语言必须在打印第一句人话之前定下来：帮助、报错、摘要都要用它
    i18n::set(pick_lang(&args));

    let cli = match parse_args(args.into_iter()) {
        Ok(Some(cli)) => cli,
        // --help / --version 已经打印完了
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            eprintln!(
                "{}",
                i18n::t(
                    "\n用 gittracer-audit --help 看用法。",
                    "\nRun gittracer-audit --help for usage."
                )
            );
            return ExitCode::from(2);
        }
    };

    let report = match audit::run(&cli.opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", tr!("巡检失败：{e}", "Audit failed: {e}", e = e));
            return ExitCode::from(2);
        }
    };

    let text = match cli.format {
        Format::Text => audit::render_text(&report),
        Format::Markdown => audit::render_markdown(&report),
        Format::Json => audit::render_json(&report),
        Format::Sarif => audit::render_sarif(&report),
        Format::Gitlab => audit::render_gitlab(&report),
    };

    match cli.out.as_deref() {
        Some(path) if path != "-" => {
            if let Err(e) = std::fs::write(path, text.as_bytes()) {
                eprintln!(
                    "{}",
                    tr!(
                        "写文件失败：{path}：{e}",
                        "Cannot write {path}: {e}",
                        path = path,
                        e = e
                    )
                );
                return ExitCode::from(2);
            }
            if !cli.quiet {
                eprintln!("{}", summary(&report));
                eprintln!(
                    "{}",
                    tr!("报告已写入 {path}", "Report written to {path}", path = path)
                );
            }
        }
        _ => {
            print!("{text}");
            let _ = std::io::stdout().flush();
        }
    }

    // 用 passed() 而不是直接数 errors：根目录不可用也算不过，
    // 否则 CI 里把路径拼错会拿到「0 个仓库、退出码 0」的假通过。
    let code = match cli.fail_on {
        FailOn::Never => 0,
        FailOn::Error => u8::from(!report.passed()),
        // 已知的提示不拦：基线的意义就是只盯新增
        FailOn::Warn => u8::from(!report.passed() || report.totals.new_warnings() > 0),
    };
    ExitCode::from(code)
}

/// 先把 `--lang` 挑出来。单独扫一遍是因为帮助文本本身也要按语言选，
/// 等走完整个参数解析再定就晚了。
fn pick_lang(args: &[String]) -> Lang {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--lang" || a == "-l" {
            if let Some(v) = it.next() {
                if let Some(l) = Lang::parse(v) {
                    return l;
                }
            }
        } else if let Some(v) = a.strip_prefix("--lang=") {
            if let Some(l) = Lang::parse(v) {
                return l;
            }
        }
    }
    i18n::from_env()
}

fn summary(report: &audit::Report) -> String {
    let t = &report.totals;
    let cfg = if report.config_files.is_empty() {
        String::new()
    } else {
        tr!(
            " · 依据 {n} 个 .gittracer.toml",
            " · from {n} .gittracer.toml",
            n = report.config_files.len()
        )
    };
    let ws = if t.workspace > 0 {
        tr!(
            " · {n} 个仓库工作区有活",
            " · {n} repo(s) with work in progress",
            n = t.workspace
        )
    } else {
        String::new()
    };
    tr!(
        "{repos} 个仓库：{failed} 未通过 / {passed} 通过 · {errors} 阻断 / {warn} 提示 · 结论：{verdict}{ws}{cfg}",
        "{repos} repo(s): {failed} failed / {passed} passed · {errors} blocking / {warn} hint(s) · verdict: {verdict}{ws}{cfg}",
        repos = t.repos,
        failed = t.failed,
        passed = t.passed,
        errors = t.errors,
        warn = t.warnings + t.infos,
        verdict = report.verdict_word(),
        ws = ws,
        cfg = cfg
    )
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Option<Cli>, String> {
    let mut opts = audit::Options::default();
    let mut format = Format::Text;
    let mut out: Option<String> = None;
    let mut fail_on = FailOn::Error;
    let mut quiet = false;
    let mut positional: Vec<String> = Vec::new();
    let mut branch: Vec<String> = Vec::new();

    let mut it = args.peekable();
    while let Some(arg) = it.next() {
        // --lang=en 这种写法先拆开，好让它和 --lang en 走同一条路
        if let Some(v) = arg.strip_prefix("--lang=") {
            check_lang(v)?;
            continue;
        }
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{}", usage());
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("gittracer-audit {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--lang" | "-l" => {
                let raw = value(&mut it, "--lang")?;
                i18n::set(check_lang(&raw)?);
            }
            "--quiet" | "-q" => quiet = true,
            "--no-config" => opts.use_config = false,
            "--baseline" => {
                let raw = value(&mut it, "--baseline")?;
                let text = std::fs::read_to_string(&raw).map_err(|e| {
                    tr!(
                        "--baseline 读不了 {path}：{e}",
                        "--baseline cannot read {path}: {e}",
                        path = raw,
                        e = e
                    )
                })?;
                let set = audit::load_baseline(&text)?;
                opts.baseline = Some(set);
                opts.baseline_file = Some(raw);
            }
            "--strict" => opts.strict = Some(true),
            "--no-strict" => opts.strict = Some(false),
            "--root" => opts
                .roots
                .push(std::path::PathBuf::from(value(&mut it, "--root")?)),
            "--depth" => {
                let raw = value(&mut it, "--depth")?;
                let n: u32 = raw.parse().map_err(|_| {
                    tr!(
                        "--depth 要一个整数，收到 {raw}",
                        "--depth wants an integer, got {raw}",
                        raw = raw
                    )
                })?;
                opts.depth = Some(n.clamp(1, 16));
            }
            "--branch" => {
                for p in split_list(&value(&mut it, "--branch")?) {
                    branch.push(p);
                }
            }
            "--base" => opts.base = Some(value(&mut it, "--base")?),
            "--skip" => {
                for p in split_list(&value(&mut it, "--skip")?) {
                    opts.skip.push(p);
                }
            }
            "--format" => {
                let raw = value(&mut it, "--format")?;
                format = match raw.to_ascii_lowercase().as_str() {
                    "text" | "txt" => Format::Text,
                    "md" | "markdown" => Format::Markdown,
                    "json" => Format::Json,
                    "sarif" => Format::Sarif,
                    // Code Quality 就是 CodeClimate 的裁剪版，两个叫法都认
                    "gitlab" | "codeclimate" | "code-quality" => Format::Gitlab,
                    other => {
                        return Err(tr!(
                            "不认识的 --format：{other}（text / md / json / sarif / gitlab）",
                            "Unknown --format: {other} (text / md / json / sarif / gitlab)",
                            other = other
                        ))
                    }
                };
            }
            "--out" => out = Some(value(&mut it, "--out")?),
            "--fail-on" => {
                let raw = value(&mut it, "--fail-on")?;
                fail_on = match raw.to_ascii_lowercase().as_str() {
                    "error" => FailOn::Error,
                    "warn" | "warning" => FailOn::Warn,
                    "never" | "none" => FailOn::Never,
                    other => {
                        return Err(tr!(
                            "不认识的 --fail-on：{other}（error / warn / never）",
                            "Unknown --fail-on: {other} (error / warn / never)",
                            other = other
                        ))
                    }
                };
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(tr!(
                    "不认识的参数：{other}",
                    "Unknown option: {other}",
                    other = other
                ));
            }
            other => positional.push(other.to_string()),
        }
    }

    for p in positional {
        opts.roots.push(std::path::PathBuf::from(p));
    }
    // 空数组和「没写」不是一回事：`--branch ""` 不合法也不会有人这么写，
    // 所以这里空就是「没指定」，交给配置文件。
    if !branch.is_empty() {
        opts.branch_allow = Some(branch);
    }
    if let Some(bad) = opts
        .skip
        .iter()
        .find(|s| !audit::RULES.iter().any(|r| r.eq_ignore_ascii_case(s)))
    {
        return Err(tr!(
            "不认识的规则：{bad}\n可用的规则：{rules}",
            "Unknown rule: {bad}\nAvailable rules: {rules}",
            bad = bad,
            rules = audit::RULES.join(", ")
        ));
    }
    if opts.roots.is_empty() {
        // CI 里最常见是 `cd` 到仓库根再跑，省掉一个参数
        let cwd = std::env::current_dir().map_err(|e| {
            tr!(
                "读不到当前目录：{e}",
                "Cannot read the current directory: {e}",
                e = e
            )
        })?;
        opts.roots.push(cwd);
    }

    Ok(Some(Cli {
        opts,
        format,
        out,
        fail_on,
        quiet,
    }))
}

fn check_lang(raw: &str) -> Result<Lang, String> {
    Lang::parse(raw).ok_or_else(|| {
        tr!(
            "--lang 只认 zh / en，收到 {raw}",
            "--lang accepts zh / en, got {raw}",
            raw = raw
        )
    })
}

fn value(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    let v = it.next().ok_or_else(|| {
        tr!(
            "{flag} 后面缺少取值",
            "{flag} is missing its value",
            flag = flag
        )
    })?;
    if v.trim().is_empty() {
        return Err(tr!(
            "{flag} 的取值为空",
            "{flag} has an empty value",
            flag = flag
        ));
    }
    Ok(v)
}

fn split_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/* ---------------------------------------------------------------- 控制台代码页
 *
 * cmd.exe 默认是 936（GBK），直接往 stdout 写 UTF-8 会变成乱码。
 * 这里临时切到 65001，退出时（Drop）还原，不把父控制台的状态改坏。
 */

#[cfg(windows)]
struct ConsoleCp(u32);

#[cfg(windows)]
fn setup_console() -> ConsoleCp {
    extern "system" {
        fn GetConsoleOutputCP() -> u32;
        fn SetConsoleOutputCP(code_page: u32) -> i32;
    }
    const CP_UTF8: u32 = 65001;
    let old = unsafe { GetConsoleOutputCP() };
    if old != 0 && old != CP_UTF8 {
        unsafe {
            SetConsoleOutputCP(CP_UTF8);
        }
    }
    ConsoleCp(old)
}

#[cfg(windows)]
impl Drop for ConsoleCp {
    fn drop(&mut self) {
        extern "system" {
            fn SetConsoleOutputCP(code_page: u32) -> i32;
        }
        if self.0 != 0 && self.0 != 65001 {
            unsafe {
                SetConsoleOutputCP(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
struct ConsoleCp;

#[cfg(not(windows))]
fn setup_console() -> ConsoleCp {
    ConsoleCp
}
