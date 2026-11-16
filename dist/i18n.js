/* 界面文案的中英对照与切换。
 *
 * 为什么 key 直接用中文原文：
 *   - 少一层「起名字」的维护成本，改中文时字典跟着一眼能对上；
 *   - 漏翻时 t() 退回中文，不会出现空白按钮；
 *   - 这个项目的文案都短，重复率低，拿原文当 key 不会撞。
 *
 * 静态文案在 index.html 上标 data-i18n（正文）/ data-i18n-ph（placeholder）/
 * data-i18n-title（title），首次调用 applyI18n() 时把元素原本的中文记下来当 key，
 * 所以 HTML 里不用把中文再抄一遍。
 *
 * 动态文案在 app.js 里写 t('中文 {x}', {x: ...})，占位符用 {名字}。
 */

const I18N_EN = {
  // ---- 顶栏 / 全局
  '添加目录（回车）': 'Add a directory (Enter)',
  '浏览…': 'Browse…',
  '扫描': 'Scan',
  '扫描深度': 'Depth',
  '只看有变更': 'Changes only',
  '发版前体检：跑一遍固定规则，给出通过 / 未通过':
    'Pre-release check: run the fixed rule set and report pass / fail',
  '巡检': 'Audit',
  '重新扫描': 'Rescan',
  '过滤仓库…': 'Filter repos…',
  '仓库': 'Repos',
  '文件': 'Files',
  '勾选本仓库全部文件': 'Select every file in this repo',
  '全选': 'Select all',
  '过滤文件路径…': 'Filter paths…',
  '取消全部勾选': 'Clear every selection',
  '取消全选': 'Clear all',
  '差异': 'Diff',
  '分支树': 'Branches',
  '提交': 'Commit',
  '提交信息': 'Commit message',
  '编辑提交信息': 'Edit the commit message',
  '取消': 'Cancel',
  '确定': 'OK',
  '就绪': 'Ready',
  '语言': 'Language',
  '切换到中文': 'Switch to Chinese',
  '切换到英文': 'Switch to English',

  // ---- 状态栏 / 统计
  '{n} 个仓库': '{n} repo(s)',
  '个仓库': 'repo(s)',
  '个有变更': 'with changes',
  '处变更': 'change(s)',
  '个根目录': 'root(s)',
  '已扫描': 'Scanned',
  '个目录': 'dirs',
  '个目录无权限': 'dirs not accessible',
  '没有匹配的仓库': 'No matching repos',
  '.git 是文件：worktree 或子模块': '.git is a file: a worktree or submodule',
  '{n} 个有变更': '{n} with changes',
  '{n} 处变更': '{n} change(s)',
  '{n} 个根目录': '{n} root(s)',
  '扫描 {dirs} 个目录 · {ms}': 'Scanned {dirs} dirs · {ms}',
  '已达仓库数量上限': 'Repository limit reached',
  '{n} 个目录无权限': '{n} dirs not accessible',
  '待推送 {n} 个仓库': '{n} repo(s) to push',
  '没找到 Git 仓库': 'No Git repositories found',
  '还没有扫描结果': 'Nothing scanned yet',
  '读取失败': 'Read failed',
  '干净': 'clean',
  '读取失败：{e}': 'Read failed: {e}',
  '这个目录下没找到 Git 仓库。': 'No Git repositories under this directory.',
  '没找到 git': 'git not found',
  'git 不可用': 'git unavailable',
  '未在桌面应用中运行': 'Not running in the desktop app',
  '请通过 Git Tracer 桌面程序打开。': 'Open this through the Git Tracer desktop app.',
  '当前不在桌面应用内运行（缺少 Tauri 宿主）':
    'Not running in the desktop app (no Tauri host)',
  '未知错误': 'Unknown error',
  '未知': 'unknown',

  // ---- 根目录标签
  '移除这个目录': 'Remove this directory',
  '这个目录已经在列表里了': 'That directory is already in the list',

  // ---- 仓库列表
  '本地有 {n} 个提交还没推送': '{n} commit(s) not pushed yet',
  '远程有 {n} 个提交还没拉取': '{n} commit(s) on the remote not pulled yet',
  '已暂存 {n} 个文件': '{n} staged file(s)',
  '未暂存 {n} 个文件': '{n} unstaged file(s)',
  '未跟踪 {n} 个文件': '{n} untracked file(s)',
  '冲突 {n} 个文件': '{n} file(s) in conflict',
  '复制仓库路径': 'Copy the repo path',
  '复制路径': 'Copy path',
  '打开目录': 'Open folder',
  '在资源管理器中打开': 'Show in the file manager',
  '仓库路径已复制': 'Repo path copied',
  '已暂存': 'Staged',
  '未暂存': 'Unstaged',
  '未跟踪': 'Untracked',
  '尚未提交': 'No commits yet',
  '还没有任何提交': 'No commits yet',
  '尚无提交': 'No commits yet',
  '勾选后可一起提交（空格键切换）': 'Tick files to commit together (Space toggles)',
  '原路径': 'Original path',

  // ---- 差异 / 分支树
  '搜索提交 / 作者 / 分支': 'Search commits / authors / branches',
  '清空搜索': 'Clear the search',
  '刷新': 'Refresh',
  '提交哈希已复制': 'Commit hash copied',
  '回滚至此处': 'Reset to this commit',
  '复制哈希': 'Copy hash',
  '返回分支树': 'Back to branches',
  '定位文件': 'Reveal the file',
  '文件路径已复制': 'File path copied',
  '开始扫描': 'Start scanning',
  '工作区干净': 'Working tree clean',
  '选择一个文件': 'Pick a file',
  '选择一个仓库': 'Pick a repo',
  '正在读取差异…': 'Loading the diff…',
  '读取差异失败：{e}': 'Cannot load the diff: {e}',
  '二进制文件': 'Binary file',
  '文件过大': 'File too large',
  '没有差异': 'No differences',
  '只显示最近 {n} 个提交': 'Showing the latest {n} commit(s)',
  '… 差异超过 {max} 行，其余 {rest} 行未渲染（可在此文件上用命令行查看）':
    '… diff exceeds {max} lines; the remaining {rest} are not rendered (use the command line for this file)',
  '… 文件超过 {max} 行，其余 {rest} 行未渲染':
    '… file exceeds {max} lines; the remaining {rest} are not rendered',

  // ---- 回滚
  '二进制改动': 'Binary change',
  '改动太大': 'Change too large',
  '没有内容变化': 'No content changes',
  '混合': 'mixed',
  '软': 'soft',
  '硬': 'hard',
  '撤销提交，改动留在工作区（还没暂存）':
    'Undo the commit, keep the changes in the working tree (unstaged)',
  '撤销提交，改动留在暂存区，可以直接重新提交':
    'Undo the commit, keep the changes staged so you can re-commit',
  '撤销提交，并丢弃已跟踪文件的未提交改动':
    'Undo the commit and discard uncommitted changes to tracked files',
  '读不到回滚信息': 'Cannot read the reset plan',
  'detached HEAD（不在任何分支上）': 'detached HEAD (not on any branch)',
  '分支 {name}': 'Branch {name}',
  '撤销 {n} 个提交': 'Undo {n} commit(s)',
  '这个提交上有 {n} 个提交不在当前分支里':
    '{n} commit(s) reachable from this one are not on the current branch',
  '还有 {n} 处未提交的改动': '{n} uncommitted change(s) remain',
  '目标就是当前提交，只会按所选方式处理下面这些未提交改动。':
    'The target is the current commit, so only the uncommitted changes below are affected.',
  '回滚到这个提交？': 'Reset to this commit?',
  '回滚': 'Reset',
  '已回滚至 {short}（{mode}）': 'Reset to {short} ({mode})',
  ' · 撤销 {n} 个提交': ' · {n} commit(s) undone',
  '回滚完成 · {branch} → {short}': 'Reset done · {branch} → {short}',
  '回滚失败': 'Reset failed',

  // ---- 通用反馈
  '已复制': 'Copied',
  '复制失败：{e}': 'Copy failed: {e}',
  '知道了': 'Got it',
  '（没有任何输出）': '(no output)',

  // ---- 提交区
  '未选择仓库': 'No repo selected',
  '读取中…': 'Loading…',
  '已选 {n} / {total} 个文件': '{n} of {total} file(s) selected',
  '处理中…': 'Working…',
  '提交选中': 'Commit selected',
  '勾选全部 {n} 个文件': 'Select all {n} file(s)',
  '取消已勾选的 {n} 个文件': 'Clear the {n} selected file(s)',
  '取消所有勾选': 'Clear all selections',
  '全选（共 {n} 个文件，已选 {sel}）': 'Select all ({n} files, {sel} selected)',
  '清空已勾选的 {n} 个文件': 'Clear the {n} selected file(s)',
  '清空勾选': 'Clear selection',
  '推送 ↑{n}': 'Push ↑{n}',
  '推送': 'Push',
  '拉取 ↓{n}': 'Pull ↓{n}',
  '拉取': 'Pull',
  '没有远程仓库': 'No remote configured',
  '推送到 {target}': 'Push to {target}',
  '推送到 {target}（未设上游）': 'Push to {target} (no upstream set)',
  '从 {target} 拉取': 'Pull from {target}',
  '从 {target} 拉取（未设上游）': 'Pull from {target} (no upstream set)',
  'detached HEAD，先切回分支': 'detached HEAD; switch to a branch first',
  '用这条信息': 'Use this message',
  '先选一个仓库': 'Pick a repo first',
  'status 还在读取': 'git status is still loading',
  '读取失败，没法提交': 'Read failed, cannot commit',
  '读取失败，没法推送': 'Read failed, cannot push',
  '读取失败，没法拉取': 'Read failed, cannot pull',
  '先勾选文件': 'Tick some files first',
  '提交信息不能为空': 'The commit message cannot be empty',
  '… 还有 {n} 个文件': '…and {n} more file(s)',
  '确认提交': 'Confirm the commit',
  '当前分支': 'Current branch',
  '已提交 {hash}': 'Committed {hash}',
  '提交失败': 'Commit failed',

  // ---- 推送
  '确认推送': 'Confirm the push',
  '待推送 {n} 个提交': '{n} commit(s) to push',
  '没有待推送的提交': 'Nothing to push',
  ' · 远程多 {n} 个提交': ' · remote is {n} commit(s) ahead',
  '已推送 {branch}': 'Pushed {branch}',
  '推送完成': 'Push finished',
  '推送失败': 'Push failed',
  '远程': 'remote',
  '已推送 → {to}': 'Pushed → {to}',
  '推送中…': 'Pushing…',
  '已推送': 'Pushed',
  '推送完成：{ok} 个成功，{fail} 个失败': 'Push finished: {ok} succeeded, {fail} failed',
  '已推送 {n} 个仓库': 'Pushed {n} repo(s)',
  '推送失败：{n} 个仓库': 'Push failed for {n} repo(s)',
  '没有待推送的提交了。': 'Nothing left to push.',
  '现在没有需要推送的仓库': 'No repos need pushing right now',
  '哪些仓库需要推送': 'Repos waiting to be pushed',
  '关闭': 'Close',
  '无远程': 'no remote',
  '（未设上游）': '(no upstream)',

  // ---- 拉取
  '确认拉取': 'Confirm the pull',
  '远程多 {n} 个提交': 'The remote is {n} commit(s) ahead',
  '远程没有新提交': 'The remote has nothing new',
  ' · 本地多 {n} 个提交': ' · local is {n} commit(s) ahead',
  '已拉取 · {summary}': 'Pulled · {summary}',
  '拉取失败': 'Pull failed',
  ' · 远程有 {n} 个提交还没拉取': ' · {n} commit(s) on the remote not pulled yet',

  // ---- 巡检
  '忽略 stash': 'Ignore stash',
  '忽略遗留分支': 'Ignore unmerged branches',
  '忽略落后远程': 'Ignore being behind the remote',
  'main, release/*（留空 = 不检查）': 'main, release/* (empty = do not check)',
  '留空 = 自动探测 main / master': 'empty = auto-detect main / master',
  '留空 = 配置文件 / 4': 'empty = config file / 4',
  '巡检中…': 'Auditing…',
  '重新巡检': 'Re-run the audit',
  '开始巡检': 'Start the audit',
  '未通过': 'Failed',
  '注意': 'Attention',
  '通过': 'Passed',
  '有阻断项，现在发出去会出问题': 'Blocking findings; shipping now would be a problem',
  '没有阻断项，但有提示需要看一眼': 'Nothing blocking, but a few hints worth a look',
  '没有阻断项，也没有提示': 'Nothing blocking and no hints',
  '严格模式': 'strict mode',
  '未提交改动也算阻断': 'Uncommitted changes count as blocking',
  '，已计入阻断': ', counted as blocking',
  '，只提示不影响结论': ', hints only, verdict unaffected',
  '本地配置': 'repo config',
  '发版前巡检': 'Pre-release audit',
    '正在读取 git status…': 'Reading git status…',
  '正在遍历目录…': 'Walking the directories…',
  '扫描失败': 'Scan failed',
  '扫描失败：{e}': 'Scan failed: {e}',
  '找到 {n} 个仓库，正在读取 git status…':
    'Found {n} repo(s), reading git status…',
  '扫描完成：{n} 个仓库，{dirty} 个有变更':
    'Scan finished: {n} repo(s), {dirty} with changes',
  ' · {n} 个读取失败': ' · {n} failed to read',
  ' · {n} 个目录出错': ' · {n} dirs errored',
  '巡检报告已复制（Markdown）': 'Audit report copied (Markdown)',
  '命令行已复制': 'Command line copied',
  '先添加要扫描的目录': 'Add a directory to scan first',

  // ---- 相对时间 / 其他零碎
  '刚刚': 'just now',
  '前天': '2 days ago',
  '昨天': 'yesterday',
  '今天': 'today',
  '天前': ' days ago',
  '小时前': ' hours ago',
  '分钟前': ' min ago',

  // ---- 文件状态字母的含义（ST_LABEL 的 label）
  '修改': 'modified',
  '新增': 'added',
  '删除': 'deleted',
  '重命名': 'renamed',
  '复制': 'copied',
  '类型变更': 'type changed',
  '冲突': 'conflict',

  // ---- 待推送入口
  '待推送': 'To push',

  // ---- 分支树 / 提交补丁
  '还没有提交': 'No commits yet',
  '没有匹配「{q}」的提交': 'No commits match “{q}”',
  '换个词试试': 'Try another word',
  '可以搜提交标题、作者、分支或标签名，多个词用空格分开。':
    'Search commit subjects, authors, branches or tags; separate words with spaces.',
  '{n} 个提交匹配「{q}」': '{n} commit(s) match “{q}”',

  // ---- 回滚（补漏）
  '{where} 从 {from} 退到 {to}「{subject}」':
    '{where} moves from {from} back to {to} “{subject}”',
  '撤销掉的提交只留在 reflog 里，本程序不提供撤销；未跟踪的新文件不会删除。':
    'Undone commits survive only in the reflog and this app cannot undo the undo; untracked new files are kept.',
  '我确认这些未提交改动会被丢弃':
    'I understand these uncommitted changes will be discarded',

  // ---- 提交 / 推送 / 拉取（补漏）
  '勾选当前仓库的全部文件': 'Select every file in this repo',
  '勾选本仓库的全部文件': 'Select every file in this repo',
  'Ctrl+Enter 提交 · Esc 取消': 'Ctrl+Enter commits · Esc cancels',
  '把 {n} 个文件提交到 {branch}': 'Commit {n} file(s) to {branch}',
  '已提交 {hash}：{subject}': 'Committed {hash}: {subject}',
  '提交成功 {hash}': 'Commit succeeded {hash}',
  '把 {branch} 推到 {target}？': 'Push {branch} to {target}?',
  '推送成功 · {branch}': 'Push succeeded · {branch}',
  '把 {from} 拉到 {branch}？': 'Pull {from} into {branch}?',
  '拉取完成 · {branch} ← {from}': 'Pull done · {branch} ← {from}',
  '共 {n} 个仓库、{m} 个提交待推送': '{n} repo(s), {m} commit(s) waiting to be pushed',
  '全部推送（{n} 个）': 'Push all ({n})',

  // ---- 巡检（补漏）
  '允许分支': 'Allowed branches',
  '基准分支': 'Base branch',
  '检查模式': 'Check mode',
  '跟随配置文件（没配置就是只提示）': 'Follow the config file (hints only when unset)',
  '严格：未提交改动也算不能发': 'Strict: uncommitted changes block the release',
  '宽松：未提交改动只提示': 'Lenient: uncommitted changes are hints only',
  '使用目录里的': "Use the repo's",
  '只读检查，不动任何仓库': 'Read-only check; no repository is touched',
  '还没跑。检查分两类：<b>发版状态</b>（冲突、半途的合并 / 变基、detached HEAD、未推送、分支不符合规则）不过就是阻断；<b>工作区</b>（未提交改动、stash、落后远程、遗留分支）只提示——几乎每台开发机都有没提交的活，把它当阻断会让每个仓库都变红，结论就没有信息量了。要从工作区直接打包发版，把「检查模式」切成严格。<br />规则可以写进目录里的 <code>.gittracer.toml</code> 跟着仓库走，命令行 <code>gittracer-audit</code> 读的是同一份，按返回码判定。':
    'Not run yet. Findings fall into two groups: <b>release state</b> (conflicts, an interrupted merge / rebase, detached HEAD, unpushed commits, a branch that breaks the rule) blocks the release; <b>working tree</b> (uncommitted changes, stash, behind the remote, leftover branches) is only a hint — nearly every dev machine has unfinished work, and treating that as blocking would paint every repo red and carry no signal. If you ship straight from the working tree, set the check mode to strict.<br />Rules can live in a <code>.gittracer.toml</code> inside the directory, so they travel with the repo; the <code>gittracer-audit</code> command line reads the same file and decides by exit code.',
  '{repos} 个仓库 · {failed} 未通过 / {passed} 通过 · {errors} 阻断 / {hints} 提示 · {secs}s':
    '{repos} repo(s) · {failed} failed / {passed} passed · {errors} blocking / {hints} hint(s) · {secs}s',
  '依据 {n} 个 .gittracer.toml': 'based on {n} .gittracer.toml',
  '这些根目录用不了，已跳过（其余照常）：{list}':
    'These roots are unusable and were skipped (the rest still ran): {list}',
  '{n} 个仓库工作区有活（未提交改动 / stash / 落后远程）':
    '{n} repo(s) have work in progress (uncommitted changes / stash / behind the remote)',
  '本仓库跳过：{list}': 'Skipped in this repo: {list}',
  '跳过 {n}': 'Skipped {n}',
  '复制报告（Markdown）': 'Copy the report (Markdown)',
  '复制命令行': 'Copy the command line',
  '命令行返回码：0 没有阻断项（可能带提示）/ 1 有阻断项':
    'Exit codes: 0 = nothing blocking (hints possible) / 1 = blocking findings',
  '正在跑 {n} 个根目录…': 'Running across {n} root(s)…',
  '未找到 git': 'git not found'
};

const I18N = {
  zh: {}, // 中文就是原文，不需要表
  en: I18N_EN
};

/** 当前语言：'zh' | 'en' */
let LANG = 'zh';

/** 取一段文案。第一个参数是中文原文（同时充当 key），第二个是占位符取值。 */
function t(zh, vars) {
  const table = I18N[LANG] || {};
  let s = table[zh] !== undefined ? table[zh] : zh;
  if (vars) {
    s = s.replace(/\{(\w+)\}/g, (m, k) => (vars[k] === undefined ? m : String(vars[k])));
  }
  return s;
}

/** 决定初始语言：存过的选择 > 系统语言（中文系统用中文，其余用英文）。 */
function detectLang() {
  try {
    const saved = localStorage.getItem('gt.lang');
    if (saved === 'zh' || saved === 'en') return saved;
  } catch (_) {
    /* 隐私模式 / 存储被禁：忽略，继续按系统语言判断 */
  }
  const sys = (navigator.language || navigator.userLanguage || '').toLowerCase();
  return sys.startsWith('zh') ? 'zh' : 'en';
}

/** 把静态文案刷一遍：正文、placeholder、title 都从元素自己记下的中文原文取 key。 */
function applyI18n() {
  document.documentElement.lang = LANG === 'en' ? 'en' : 'zh-CN';

  document.querySelectorAll('[data-i18n]').forEach((el) => {
    if (!el.dataset.i18nKey) el.dataset.i18nKey = el.textContent.trim();
    el.textContent = t(el.dataset.i18nKey);
  });
  document.querySelectorAll('[data-i18n-ph]').forEach((el) => {
    if (!el.dataset.i18nPhKey) el.dataset.i18nPhKey = el.getAttribute('placeholder') || '';
    el.setAttribute('placeholder', t(el.dataset.i18nPhKey));
  });
  document.querySelectorAll('[data-i18n-title]').forEach((el) => {
    if (!el.dataset.i18nTitleKey) el.dataset.i18nTitleKey = el.getAttribute('title') || '';
    el.setAttribute('title', t(el.dataset.i18nTitleKey));
  });

  // 数字型下拉（1 层 / 2 层…）和语言按钮在各处单独刷
  document.querySelectorAll('option[data-depth]').forEach((o) => {
    o.textContent =
      LANG === 'en'
        ? `${o.dataset.depth} level${o.dataset.depth === '1' ? '' : 's'}`
        : `${o.dataset.depth} 层`;
  });
  const btn = document.getElementById('langBtn');
  if (btn) {
    btn.textContent = LANG === 'en' ? '中文' : 'EN';
    btn.title = t(LANG === 'en' ? '切换到中文' : '切换到英文');
  }
}

/**
 * 切语言：存选择、刷静态文案、通知后端（窗口标题、对话框、报告都用它）、
 * 再让 app.js 重画所有动态区域。
 */
async function setLang(lang, opts) {
  LANG = lang === 'en' ? 'en' : 'zh';
  try {
    localStorage.setItem('gt.lang', LANG);
  } catch (_) {
    /* 存不下不影响本次使用 */
  }
  applyI18n();

  if (typeof invoke === 'function') {
    try {
      await invoke('set_lang', { lang: LANG });
    } catch (_) {
      /* 老版本后端没有这个命令：界面照常切，只是原生标题不变 */
    }
  }
  if (typeof onLangChanged === 'function') onLangChanged();
  if (opts && opts.announce === false) return;
}

function initLang() {
  LANG = detectLang();
  applyI18n();
}
