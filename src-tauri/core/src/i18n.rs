//! 界面语言：中文 / 英文。
//!
//! 为什么用全局开关而不是给每个函数加参数：git 封装那 1500 行里散着近百处
//! 「命令失败」的说明文字，逐个函数加 `lang` 参数会牵动全部调用点和一个字都
//! 不相关的测试。语言在一次运行里不会变（命令行在启动时定，界面在用户点
//! 切换时定），所以一个进程级的开关够用。
//!
//! 读取用 `Relaxed`：它只是一个「用哪种语言写字」的开关，不参与任何同步逻辑。
//! 界面在后台线程跑巡检、用户同时切语言时，最多是那一份报告用旧语言写完。

use std::sync::atomic::{AtomicU8, Ordering};

/// 支持的语言。将来加第三种就在这里加一档、`parse` 加一条分支。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

impl Lang {
    /// 从语言标签猜：`zh` / `zh-CN` / `en` / `en_US` / `en-US` 都认。
    ///
    /// 认不出来（`fr-FR`、空串）时返回 `None`，交给调用方决定默认值——
    /// 命令行选中文，界面选英文（非中文用户看到英文比看到中文更有用）。
    pub fn parse(tag: &str) -> Option<Lang> {
        let head = tag
            .trim()
            .split(['-', '_', '.'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match head.as_str() {
            "zh" | "cn" | "chs" | "cht" => Some(Lang::Zh),
            "en" => Some(Lang::En),
            _ => None,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Lang::Zh => "zh",
            Lang::En => "en",
        }
    }

    pub fn is_en(self) -> bool {
        self == Lang::En
    }
}

const ZH: u8 = 0;
const EN: u8 = 1;

static CURRENT: AtomicU8 = AtomicU8::new(ZH);

pub fn set(lang: Lang) {
    CURRENT.store(
        match lang {
            Lang::Zh => ZH,
            Lang::En => EN,
        },
        Ordering::Relaxed,
    );
}

pub fn get() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == EN {
        Lang::En
    } else {
        Lang::Zh
    }
}

pub fn is_en() -> bool {
    get().is_en()
}

/// 静态文案二选一，不分配。参数化的句子用 [`crate::tr!`]。
pub fn t(zh: &'static str, en: &'static str) -> &'static str {
    if is_en() {
        en
    } else {
        zh
    }
}

/// 带参数的文案：两句话各自 `format!`，参数按名字传，避免中英顺序不同导致错位。
///
/// ```
/// use git_tracer_core::tr;
/// let s = tr!("有 {n} 个提交没推", "{n} commit(s) not pushed", n = 2);
/// ```
///
/// 一个参数都没有时也能用，但静态句子直接用 [`i18n::t`] 更省事。
#[macro_export]
macro_rules! tr {
    ($zh:literal, $en:literal $(, $k:ident = $v:expr)* $(,)?) => {{
        if $crate::i18n::is_en() {
            format!($en $(, $k = $v)*)
        } else {
            format!($zh $(, $k = $v)*)
        }
    }};
}

/// 按系统环境猜默认语言。
///
/// Windows 上取 `LANG` / `LC_ALL` 往往为空，退到 `USERPROFILE` 之外没有更可靠的
/// 系统区域读法（要进注册表），所以这里保持「环境变量优先、否则中文」：
/// 图形界面不依赖它——前端用 `navigator.language` 自己判断，再把结果推下来。
pub fn from_env() -> Lang {
    for key in ["GITTRACER_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(key) {
            if let Some(l) = Lang::parse(&v) {
                return l;
            }
        }
    }
    Lang::Zh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_tags() {
        assert_eq!(Lang::parse("zh-CN"), Some(Lang::Zh));
        assert_eq!(Lang::parse("zh_CN.UTF-8"), Some(Lang::Zh));
        assert_eq!(Lang::parse("en-US"), Some(Lang::En));
        assert_eq!(Lang::parse("EN"), Some(Lang::En));
        assert_eq!(Lang::parse("fr-FR"), None);
        assert_eq!(Lang::parse(""), None);
    }

    #[test]
    fn switch_affects_both_helpers() {
        // 全局开关是进程级的，测完必须还原，否则会影响同进程里别的用例
        let old = get();
        set(Lang::En);
        assert!(is_en());
        assert_eq!(t("中文", "English"), "English");
        assert_eq!(crate::tr!("有 {n} 项", "{n} item(s)", n = 3), "3 item(s)");
        set(Lang::Zh);
        assert!(!is_en());
        assert_eq!(t("中文", "English"), "中文");
        assert_eq!(crate::tr!("有 {n} 项", "{n} item(s)", n = 3), "有 3 项");
        set(old);
    }
}
