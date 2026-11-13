//! `gittracer-audit` —— 命令行巡检入口。
//!
//! 单独编成 console 子系统的小 exe，而不是给 GUI 加参数：主程序是
//! `windows_subsystem = "windows"`，从命令行启动既拿不到一个正常的 stdout
//! （父控制台不会自动接上），还会白弹一个窗口，没法接在 CI 后面。
//!
//! 逻辑全在 `git_tracer_lib::audit` 里，这里只做参数解析、输出和退出码。

use std::io::Write;
use std::process::ExitCode;

use git_tracer_lib::audit;

const USAGE: &str = r#"gittracer-audit —— 发版 / 交付前的多仓库巡检（只读）

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
  --no-config        不读目录里的 .gittracer.toml
  --format <格式>    text（默认）/ md / json
  --out <文件>       写到文件；"-" 表示标准输出
  --fail-on <级别>   error（默认）/ warn / never —— 决定退出码
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
  gittracer-audit . --format md --out audit.md --fail-on warn
"#;

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Text,
    Markdown,
    Json,
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

    let cli = match parse_args(std::env::args().skip(1)) {
        Ok(Some(cli)) => cli,
        // --help / --version 已经打印完了
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            eprintln!("\n用 gittracer-audit --help 看用法。");
            return ExitCode::from(2);
        }
    };

    let report = match audit::run(&cli.opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("巡检失败：{e}");
            return ExitCode::from(2);
        }
    };

    let text = match cli.format {
        Format::Text => audit::render_text(&report),
        Format::Markdown => audit::render_markdown(&report),
        Format::Json => audit::render_json(&report),
    };

    match cli.out.as_deref() {
        Some(path) if path != "-" => {
            if let Err(e) = std::fs::write(path, text.as_bytes()) {
                eprintln!("写文件失败：{path}：{e}");
                return ExitCode::from(2);
            }
            if !cli.quiet {
                eprintln!("{}", summary(&report));
                eprintln!("报告已写入 {path}");
            }
        }
        _ => {
            print!("{text}");
            let _ = std::io::stdout().flush();
        }
    }

    let code = match cli.fail_on {
        FailOn::Never => 0,
        FailOn::Error => u8::from(report.totals.errors > 0),
        FailOn::Warn => u8::from(report.totals.errors + report.totals.warnings > 0),
    };
    ExitCode::from(code)
}

fn summary(report: &audit::Report) -> String {
    let t = &report.totals;
    let cfg = if report.config_files.is_empty() {
        String::new()
    } else {
        format!(" · 依据 {} 个 .gittracer.toml", report.config_files.len())
    };
    format!(
        "{} 个仓库：{} 通过 / {} 未通过 · {} error / {} warn · 结论：{}{}",
        t.repos,
        t.passed,
        t.failed,
        t.errors,
        t.warnings,
        report.verdict_word(),
        cfg
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
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("gittracer-audit {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--quiet" | "-q" => quiet = true,
            "--no-config" => opts.use_config = false,
            "--root" => opts
                .roots
                .push(std::path::PathBuf::from(value(&mut it, "--root")?)),
            "--depth" => {
                let raw = value(&mut it, "--depth")?;
                let n: u32 = raw
                    .parse()
                    .map_err(|_| format!("--depth 要一个整数，收到 {raw}"))?;
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
                    other => return Err(format!("不认识的 --format：{other}（text / md / json）")),
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
                        return Err(format!("不认识的 --fail-on：{other}（error / warn / never）"))
                    }
                };
            }
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(format!("不认识的参数：{other}"));
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
        return Err(format!(
            "不认识的规则：{bad}\n可用的规则：{}",
            audit::RULES.join(", ")
        ));
    }
    if opts.roots.is_empty() {
        // CI 里最常见是 `cd` 到仓库根再跑，省掉一个参数
        let cwd = std::env::current_dir().map_err(|e| format!("读不到当前目录：{e}"))?;
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

fn value(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    let v = it.next().ok_or_else(|| format!("{flag} 后面缺少取值"))?;
    if v.trim().is_empty() {
        return Err(format!("{flag} 的取值为空"));
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
