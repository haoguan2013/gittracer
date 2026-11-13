//! `.gittracer.toml` —— 把巡检规则放进目录 / 仓库里。
//!
//! 为什么要有文件：命令行参数和界面输入都只活在「这台机器的这次运行」里，
//! 换台机器、换个人、换到 CI 就全没了。规则写进文件之后，它会跟着仓库走，
//! 谁 clone 下来跑出来的结论都一样——这也是「门禁」和「面板」的分界。
//!
//! 两个位置，从外到内：
//! - `<根目录>/.gittracer.toml`：管这个根目录下的所有仓库（还能定 `depth`）
//! - `<仓库>/.gittracer.toml`：只覆盖这一个仓库
//!
//! 优先级：命令行 / 界面显式给的 > 仓库文件 > 根目录文件 > 内置默认。
//!
//! 解析失败一律当错误抛出，不做「读不懂就忽略」：一个因为拼错键名而被静默
//! 忽略的配置文件，结果就是「以为在拦、其实没拦」，和假通过一样危险。

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub const FILE_NAME: &str = ".gittracer.toml";

/// 一个配置文件的内容。`None` 表示这个键没写，从而让上一层生效。
#[derive(Debug, Clone, Default)]
pub struct FileConfig {
    /// 配置文件的绝对路径，用来在报告里说明「依据的是哪个文件」
    pub path: PathBuf,
    /// 只在根目录的配置里生效；仓库里的会被忽略
    pub depth: Option<u32>,
    /// `Some(vec![])` 是「显式不检查分支」，和「没写」不一样
    pub branch: Option<Vec<String>>,
    pub base: Option<String>,
    pub skip: Vec<String>,
}

impl FileConfig {
    pub fn display(&self) -> String {
        self.path.display().to_string()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    depth: Option<u32>,
    branch: Option<Vec<String>>,
    base: Option<String>,
    #[serde(default)]
    skip: Vec<String>,
}

/// 目录里有没有配置文件。只认这一层，不往上找：扫描根目录是用户显式给的，
/// 再往上翻容易翻到别人机器上的文件，行为会变得不可预测。
pub fn find_in(dir: &Path) -> Option<PathBuf> {
    let p = dir.join(FILE_NAME);
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// 读一个配置文件并校验。`rules` 是允许的规则名，用于校验 `skip` 的拼写。
pub fn load(path: &Path, rules: &[&str]) -> Result<FileConfig, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("读不到配置文件 {}：{e}", path.display()))?;
    let raw: Raw = toml::from_str(&text).map_err(|e| {
        format!(
            "配置文件 {} 有错：{e}\n\
             支持的键：depth（1~16）、branch（字符串数组）、base（字符串）、skip（字符串数组）",
            path.display()
        )
    })?;

    if let Some(d) = raw.depth {
        if !(1..=16).contains(&d) {
            return Err(format!(
                "配置文件 {}：depth 要在 1~16 之间，收到 {d}",
                path.display()
            ));
        }
    }
    if let Some(b) = &raw.base {
        check_ref_like("base", b).map_err(|e| format!("配置文件 {}：{e}", path.display()))?;
    }

    let branch = match raw.branch {
        Some(list) => {
            let mut out: Vec<String> = Vec::new();
            for p in list {
                let p = p.trim().to_string();
                if p.is_empty() {
                    continue;
                }
                check_ref_like("branch", &p)
                    .map_err(|e| format!("配置文件 {}：{e}", path.display()))?;
                push_unique(&mut out, p);
            }
            Some(out)
        }
        None => None,
    };

    let mut skip: Vec<String> = Vec::new();
    for s in raw.skip {
        let s = s.trim().to_string();
        if s.is_empty() {
            continue;
        }
        // 拼错的规则名要当场报错：否则「以为跳过了、其实还在检查」，
        // 或者反过来「以为检查了、其实早就跳过」，两边都得对着 CI 排查半天。
        if !rules.iter().any(|r| r.eq_ignore_ascii_case(&s)) {
            return Err(format!(
                "配置文件 {}：不认识的规则 {s}\n可用的规则：{}",
                path.display(),
                rules.join(", ")
            ));
        }
        push_unique(&mut skip, s);
    }

    Ok(FileConfig {
        path: path.to_path_buf(),
        depth: raw.depth,
        branch,
        base: raw.base.map(|b| b.trim().to_string()),
        skip,
    })
}

/// 合并之后真正拿去跑的那套规则。
#[derive(Debug, Clone, Default)]
pub struct Rules {
    pub branch_allow: Vec<String>,
    pub base: Option<String>,
    pub skip: Vec<String>,
}

impl Rules {
    /// 三层合并。`cli_*` 是命令行 / 界面显式给的值，`repo` 是仓库自带的文件，
    /// `root` 是根目录那份。
    pub fn resolve(
        cli_branch: Option<&[String]>,
        cli_base: Option<&str>,
        cli_skip: &[String],
        repo: Option<&FileConfig>,
        root: Option<&FileConfig>,
    ) -> Self {
        let branch_allow = cli_branch
            .map(|v| v.to_vec())
            .or_else(|| repo.and_then(|c| c.branch.clone()))
            .or_else(|| root.and_then(|c| c.branch.clone()))
            .unwrap_or_default();

        let base = cli_base
            .map(str::to_string)
            .or_else(|| repo.and_then(|c| c.base.clone()))
            .or_else(|| root.and_then(|c| c.base.clone()));

        // skip 只能往里加：命令行给的不会被文件里的空数组清掉。
        // 少拦一条比多拦一条危险，所以合并方向固定成「越外越宽」。
        let mut skip: Vec<String> = Vec::new();
        for s in cli_skip {
            push_unique(&mut skip, s.trim().to_string());
        }
        for src in [repo, root] {
            if let Some(c) = src {
                for s in &c.skip {
                    push_unique(&mut skip, s.clone());
                }
            }
        }

        Self {
            branch_allow,
            base,
            skip,
        }
    }

    pub fn skipped(&self, code: &str) -> bool {
        self.skip.iter().any(|s| s.eq_ignore_ascii_case(code))
    }
}

/// 会被拼进 git 参数位的取值校验。
///
/// `base` 要传给 `rev-parse` 和 `branch --no-merged`：以 `-` 开头的值会被 git 当成
/// 选项（`--base --all` 就是），命令报错后被当成「这个仓库没有基准分支」而跳过，
/// 结论照样显示「通过」。假通过比假失败危险，所以入口就挡掉。
pub fn check_ref_like(flag: &str, v: &str) -> Result<(), String> {
    if v.starts_with('-') {
        return Err(format!("{flag} 的取值不能以 - 开头：{v}"));
    }
    if v.chars().any(|c| c == '"' || c.is_control()) {
        return Err(format!("{flag} 的取值不能包含引号或控制字符：{v}"));
    }
    Ok(())
}

fn push_unique(list: &mut Vec<String>, item: String) {
    if item.is_empty() {
        return;
    }
    if !list.iter().any(|x| x.eq_ignore_ascii_case(&item)) {
        list.push(item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES: &[&str] = &["dirty", "stash", "branch"];

    fn tmp(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("gt-cfg-{}-{}", std::process::id(), name));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(FILE_NAME);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn reads_every_supported_key() {
        let p = tmp("full", "depth = 2\nbranch = [\"main\", \"release/*\"]\nbase = \"main\"\nskip = [\"stash\"]\n");
        let c = load(&p, RULES).unwrap();
        assert_eq!(c.depth, Some(2));
        assert_eq!(c.branch.unwrap(), vec!["main", "release/*"]);
        assert_eq!(c.base.as_deref(), Some("main"));
        assert_eq!(c.skip, vec!["stash"]);
    }

    #[test]
    fn absent_branch_is_not_the_same_as_empty() {
        // 没写 branch = 继承上一层；写了空数组 = 显式不检查分支
        let absent = load(&tmp("absent", "base = \"main\"\n"), RULES).unwrap();
        assert!(absent.branch.is_none());
        let empty = load(&tmp("empty", "branch = []\n"), RULES).unwrap();
        assert_eq!(empty.branch, Some(vec![]));
    }

    #[test]
    fn rejects_unknown_key() {
        let err = load(&tmp("bad-key", "branchs = [\"main\"]\n"), RULES).unwrap_err();
        assert!(err.contains("配置文件"), "{err}");
    }

    #[test]
    fn rejects_unknown_rule_name() {
        let err = load(&tmp("bad-rule", "skip = [\"dirtyy\"]\n"), RULES).unwrap_err();
        assert!(err.contains("不认识的规则"), "{err}");
    }

    #[test]
    fn rejects_dash_and_quotes() {
        assert!(load(&tmp("dash", "base = \"--all\"\n"), RULES).is_err());
        assert!(load(&tmp("quote", "branch = [\"a\\\"b\"]\n"), RULES).is_err());
    }

    #[test]
    fn rejects_depth_out_of_range() {
        assert!(load(&tmp("deep", "depth = 40\n"), RULES).is_err());
    }

    #[test]
    fn cli_wins_over_files() {
        let repo = FileConfig {
            branch: Some(vec!["repo".into()]),
            base: Some("repo-base".into()),
            skip: vec!["stash".into()],
            ..Default::default()
        };
        let root = FileConfig {
            branch: Some(vec!["root".into()]),
            base: Some("root-base".into()),
            skip: vec!["behind".into()],
            ..Default::default()
        };
        let cli = vec!["cli".to_string()];
        let r = Rules::resolve(Some(&cli), Some("cli-base"), &[], Some(&repo), Some(&root));
        assert_eq!(r.branch_allow, vec!["cli"]);
        assert_eq!(r.base.as_deref(), Some("cli-base"));
        // skip 是并集，命令行没给就吃文件
        assert_eq!(r.skip, vec!["stash", "behind"]);
    }

    #[test]
    fn repo_wins_over_root() {
        let repo = FileConfig {
            branch: Some(vec!["repo".into()]),
            ..Default::default()
        };
        let root = FileConfig {
            branch: Some(vec!["root".into()]),
            base: Some("main".into()),
            ..Default::default()
        };
        let r = Rules::resolve(None, None, &[], Some(&repo), Some(&root));
        assert_eq!(r.branch_allow, vec!["repo"]);
        // 仓库没写 base，落到根目录那份
        assert_eq!(r.base.as_deref(), Some("main"));
    }

    #[test]
    fn empty_branch_in_repo_config_turns_check_off() {
        let repo = FileConfig {
            branch: Some(vec![]),
            ..Default::default()
        };
        let root = FileConfig {
            branch: Some(vec!["main".into()]),
            ..Default::default()
        };
        let r = Rules::resolve(None, None, &[], Some(&repo), Some(&root));
        assert!(r.branch_allow.is_empty());
    }
}
