//! Git Tracer 核心：扫描（scan）、git 封装（git）、巡检（audit）、配置（config）。
//!
//! 这个 crate 刻意不依赖 tauri —— 巡检 CLI（`gittracer-audit`）要跑在
//! 无界面的环境里：GitHub Actions 的 Linux runner、交付现场的服务器。
//! 依赖越少，"拿到 CI 里当门禁"这件事就越容易发生。
//!
//! 界面那一层（`git-tracer` 主包）只是把这些能力包成 tauri 命令。

pub mod audit;
pub mod config;
pub mod git;
pub mod i18n;
pub mod scan;
