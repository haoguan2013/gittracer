/* Git Tracer 前端 —— 无框架，直接操作 DOM。
 * 与后端只通过 window.__TAURI_INTERNALS__.invoke 通信，省掉 @tauri-apps/api 依赖。 */
'use strict';

/* ---------- 常量 ---------- */
const LS = {
  roots: 'git-tracer.roots.v1',
  root: 'git-tracer.root', // 旧版单目录，仅用于迁移
  depth: 'git-tracer.depth',
  // v2：默认改成「显示全部仓库」，旧的 true 值不再沿用
  dirtyOnly: 'git-tracer.dirtyOnly.v2',
  view: 'git-tracer.view',
  recent: 'git-tracer.recent',
};
const MAX_LIST_ROWS = 6000;

/* 分支树的行高 / 车道宽 / 车道配色，SVG 与行 HTML 必须共用同一套数 */
const G_ROW_H = 26;
const G_LANE_W = 22;
const G_COLORS = ['#9fe4f2', '#b9a8f0', '#6fb3e6', '#d99a4a', '#d96357', '#7fc99a', '#e08c4a', '#e3c684'];

/** 状态码 → 展示用的字母 / 说明 / 着色类名 */
const CODE = {
  M: { text: 'M', label: '修改', cls: 'mod' },
  A: { text: 'A', label: '新增', cls: 'add' },
  D: { text: 'D', label: '删除', cls: 'del' },
  R: { text: 'R', label: '重命名', cls: 'ren' },
  C: { text: 'C', label: '复制', cls: 'ren' },
  T: { text: 'T', label: '类型变更', cls: 'mod' },
  U: { text: 'U', label: '冲突', cls: 'conflict' },
  '?': { text: '?', label: '未跟踪', cls: 'new' },
  X: { text: '!', label: '未知', cls: 'mod' },
};

/* ---------- DOM ---------- */
const $ = (id) => document.getElementById(id);
const els = {
  rootInput: $('rootInput'),
  rootChips: $('rootChips'),
  browseBtn: $('browseBtn'),
  scanBtn: $('scanBtn'),
  refreshBtn: $('refreshBtn'),
  auditBtn: $('auditBtn'),
  depthSel: $('depthSel'),
  dirtyOnly: $('dirtyOnly'),
  statRepos: $('statRepos'),
  statDirty: $('statDirty'),
  statFiles: $('statFiles'),
  statTime: $('statTime'),
  statGit: $('statGit'),
  pushEntry: $('pushEntry'),
  pushCount: $('pushCount'),
  repoFilter: $('repoFilter'),
  repoCount: $('repoCount'),
  repoList: $('repoList'),
  repoHead: $('repoHead'),
  fileFilter: $('fileFilter'),
  fileList: $('fileList'),
  diffHead: $('diffHead'),
  diffBody: $('diffBody'),
  viewTabs: $('viewTabs'),
  statusLeft: $('statusLeft'),
  toastWrap: $('toastWrap'),
  commitSel: $('commitSel'),
  commitMsg: $('commitMsg'),
  commitExpandBtn: $('commitExpandBtn'),
  selectAllBtn: $('selectAllBtn'),
  selectNoneBtn: $('selectNoneBtn'),
  pickAll: $('pickAll'),
  listSelectNoneBtn: $('listSelectNoneBtn'),
  commitBtn: $('commitBtn'),
  pullBtn: $('pullBtn'),
  pushBtn: $('pushBtn'),
  modalWrap: $('modalWrap'),
  modalTitle: $('modalTitle'),
  modalBody: $('modalBody'),
  modalOk: $('modalOk'),
  modalCancel: $('modalCancel'),
};

/* ---------- 状态 ---------- */
const state = {
  roots: [],           // 绑定的根目录（可以多个）
  depth: 4,
  // 默认把没有待提交项的仓库也列出来（用户要求「即使没有待提交项也要显示」）
  dirtyOnly: false,
  git: null,
  scan: null,
  repos: [],
  status: new Map(),   // path -> RepoStatus | { error }
  pending: new Set(),
  selectedRepo: null,
  checked: new Set(),  // 当前仓库里勾选待提交的文件（仓库相对路径）
  busy: false,         // 提交 / 推送进行中，避免重复触发
  file: null,          // { path, staged, untracked, conflicted, code, additions, deletions }
  diff: null,
  view: 'diff',        // diff = 文件差异 / graph = 分支树
  graph: null,         // { data } | { loading: true } | { error }
  graphSearch: '',     // 分支树里的模糊搜索词
  commit: null,        // 分支树里点中的提交 { hash, short, subject }
  commitDiff: null,    // 该提交的补丁
  repoFilter: '',
  fileFilter: '',
  seq: 0,
  diffToken: 0,
};

/* ---------- 工具 ---------- */
/** Windows 路径统一显示成反斜杠（用户手输的根目录可能带正斜杠） */
const nicePath = (p) => {
  const s = String(p || '');
  return /^[A-Za-z]:[\\/]/.test(s) ? s.replace(/\//g, '\\') : s;
};

/** 去掉末尾分隔符，方便比较 */
function trimRoot(p) {
  const s = nicePath(p).trim();
  return s.length > 3 ? s.replace(/[\\/]+$/, '') : s;
}

/** 去重用的路径键：统一分隔符 + 不分大小写。
 *  同一个目录可能被写成 D:/x，也可能被遍历回来时写成 D:\x，字符串不相等但确实是同一个。 */
const pathKey = (p) => String(p || '').replace(/\//g, '\\').toLowerCase();

/** 同一个目录只留一个（Windows 路径不分大小写） */
function dedupeRoots(list) {
  const seen = new Set();
  const out = [];
  for (const raw of list) {
    const p = trimRoot(raw);
    if (!p) continue;
    const k = pathKey(p);
    if (seen.has(k)) continue;
    seen.add(k);
    out.push(p);
  }
  return out;
}

/** 路径的最后一段，用来在界面上区分多个根目录 */
const baseName = (p) => {
  const parts = trimRoot(p).split(/[\\/]/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : trimRoot(p);
};

const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

function hasTauri() {
  return !!(window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke);
}

async function invoke(cmd, args) {
  if (!hasTauri()) throw new Error('当前不在桌面应用内运行（缺少 Tauri 宿主）');
  return window.__TAURI_INTERNALS__.invoke(cmd, args || {});
}

function errText(e) {
  if (!e) return '未知错误';
  if (typeof e === 'string') return e;
  if (e.message) return e.message;
  return String(e);
}

function toast(msg, isErr) {
  const div = document.createElement('div');
  div.className = 'toast' + (isErr ? ' err' : '');
  div.textContent = msg;
  els.toastWrap.appendChild(div);
  setTimeout(() => {
    div.style.transition = 'opacity .2s, transform .2s';
    div.style.opacity = '0';
    div.style.transform = 'translateY(4px)';
    setTimeout(() => div.remove(), 220);
  }, isErr ? 4200 : 1800);
}

function fmtMs(ms) {
  return ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(2)} s`;
}

function fmtSize(bytes) {
  if (bytes == null) return '';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function changedCount(st) {
  if (!st || st.error) return 0;
  return st.staged + st.unstaged + st.untracked + st.conflicted;
}

function distinctCount(st) {
  if (!st || st.error) return 0;
  return new Set(st.entries.map((e) => e.path)).size;
}

function splitPath(p) {
  const i = p.lastIndexOf('/');
  return i < 0 ? { dir: '', name: p } : { dir: p.slice(0, i + 1), name: p.slice(i + 1) };
}

function pathHtml(p, cls) {
  const { dir, name } = splitPath(p);
  return `<span class="${cls}"><span class="dir">${esc(dir)}</span><span class="name">${esc(name)}</span></span>`;
}

const isInput = (el) => el && (el.tagName === 'INPUT' || el.tagName === 'SELECT' || el.tagName === 'TEXTAREA');

/* ---------- 记忆 ---------- */
function savePrefs() {
  try {
    localStorage.setItem(LS.roots, JSON.stringify(state.roots));
    localStorage.setItem(LS.depth, String(state.depth));
    localStorage.setItem(LS.dirtyOnly, state.dirtyOnly ? '1' : '0');
    localStorage.setItem(LS.view, state.view);
  } catch (_) { /* 忽略 */ }
}

function readPrefs() {
  try {
    let list = [];
    try {
      const arr = JSON.parse(localStorage.getItem(LS.roots) || '[]');
      if (Array.isArray(arr)) list = arr.filter((x) => typeof x === 'string' && x.trim());
    } catch (_) { /* 忽略 */ }
    // 老版本的单个根目录迁过来，迁完就当没有
    const old = localStorage.getItem(LS.root);
    if (!list.length && old && old.trim()) list = [old.trim()];
    state.roots = dedupeRoots(list);
    const d = parseInt(localStorage.getItem(LS.depth) || '4', 10);
    state.depth = Number.isFinite(d) ? d : 4;
    state.dirtyOnly = localStorage.getItem(LS.dirtyOnly) === '1';
    state.view = localStorage.getItem(LS.view) === 'graph' ? 'graph' : 'diff';
  } catch (_) { /* 忽略 */ }
}

function recents() {
  try {
    const arr = JSON.parse(localStorage.getItem(LS.recent) || '[]');
    return Array.isArray(arr) ? arr.filter((x) => typeof x === 'string').slice(0, 6) : [];
  } catch (_) {
    return [];
  }
}

function pushRecent(root) {
  if (!root) return;
  const list = [root, ...recents().filter((x) => x !== root)].slice(0, 6);
  try {
    localStorage.setItem(LS.recent, JSON.stringify(list));
  } catch (_) { /* 忽略 */ }
}

/* ---------- 绑定的根目录 ---------- */
function renderRoots() {
  if (!els.rootChips) return;
  const repos = state.repos;
  els.rootChips.innerHTML = state.roots
    .map((root) => {
      const n = repos.filter((r) => r.root === root).length;
      const count = state.scan ? `<span class="chip-count">${n}</span>` : '';
      return `<span class="root-chip" data-root="${esc(root)}" title="${esc(root)}">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
          <path d="M3 7.5A1.5 1.5 0 0 1 4.5 6h4l1.8 2.2h7.2A1.5 1.5 0 0 1 19 9.7v7.8A1.5 1.5 0 0 1 17.5 19h-13A1.5 1.5 0 0 1 3 17.5z"/>
        </svg>
        <span class="chip-name">${esc(baseName(root))}</span>${count}
        <button class="chip-x" data-remove="${esc(root)}" title="移除这个目录">×</button>
      </span>`;
    })
    .join('');
  els.rootChips.hidden = !state.roots.length;
}

/** 添加一个根目录（重复的忽略），加完自动重扫 */
function addRoot(raw) {
  const p = trimRoot(raw);
  if (!p) return false;
  if (state.roots.some((r) => pathKey(r) === pathKey(p))) {
    toast('这个目录已经在列表里了');
    return false;
  }
  state.roots.push(p);
  savePrefs();
  renderRoots();
  scan();
  return true;
}

function removeRoot(p) {
  const before = state.roots.length;
  state.roots = state.roots.filter((r) => r !== p);
  if (state.roots.length === before) return;
  savePrefs();
  renderRoots();
  if (state.roots.length) scan();
  else {
    state.scan = null;
    state.repos = [];
    state.status.clear();
    state.pending.clear();
    state.selectedRepo = null;
    state.checked.clear();
    state.file = null;
    state.diff = null;
    state.graph = null;
    state.commit = null;
    state.commitDiff = null;
    renderStats();
    renderRepoList();
    renderRepoHead();
    renderFileList();
    renderCommitBar();
    renderDiffHead();
    renderDiffBody();
    els.statusLeft.textContent = '就绪';
  }
}

/* ---------- 渲染：统计条 ---------- */
function renderStats() {
  if (!state.scan) {
    els.statRepos.innerHTML = '<b>—</b> 个仓库';
    els.statDirty.innerHTML = '<b>—</b> 个有变更';
    els.statFiles.innerHTML = '<b>—</b> 处变更';
    els.statTime.textContent = '';
    renderPushEntry();
    renderRoots();
    return;
  }
  const repos = state.scan.repos;
  let dirty = 0;
  let files = 0;
  for (const r of repos) {
    const st = state.status.get(r.path);
    if (st && !st.error) {
      const n = distinctCount(st);
      if (n > 0) dirty += 1;
      files += n;
    }
  }
  els.statRepos.innerHTML = `<b>${repos.length}</b> 个仓库`;
  els.statDirty.innerHTML = `<b>${dirty}</b> 个有变更`;
  els.statFiles.innerHTML = `<b>${files}</b> 处变更`;
  els.statDirty.classList.toggle('hot', dirty > 0);
  const bits = [];
  if (state.roots.length > 1) bits.push(`${state.roots.length} 个根目录`);
  bits.push(`扫描 ${state.scan.dirs} 个目录 · ${fmtMs(state.scan.elapsed_ms)}`);
  if (state.scan.truncated) bits.push('已达仓库数量上限');
  if (state.scan.errors.length) bits.push(`${state.scan.errors.length} 个目录无权限`);
  els.statTime.textContent = bits.join(' · ');
  renderPushEntry();
  renderRoots();
}

/* ---------- 待推送清单（哪些仓库有没推的提交） ---------- */
function pushTargets() {
  const out = [];
  for (const ref of state.repos) {
    const st = state.status.get(ref.path);
    if (!st || st.error) continue;
    if ((st.ahead || 0) > 0) out.push({ ref, st });
  }
  out.sort(
    (a, b) =>
      (b.st.ahead || 0) - (a.st.ahead || 0) || a.ref.rel.localeCompare(b.ref.rel, 'zh-CN')
  );
  return out;
}

function renderPushEntry() {
  const list = pushTargets();
  els.pushEntry.hidden = !list.length;
  els.pushCount.textContent = String(list.length);
}

/* ---------- 渲染：仓库列表 ---------- */
function repoBadges(st) {
  if (!st) return '';
  if (st.error) return '<span class="badge err" title="读取失败">!</span>';
  const ahead =
    st.ahead > 0
      ? `<span class="badge ahead" title="本地有 ${st.ahead} 个提交还没推送">↑${st.ahead}</span>`
      : '';
  const behind =
    st.behind > 0
      ? `<span class="badge behind" title="远程有 ${st.behind} 个提交还没拉取">↓${st.behind}</span>`
      : '';
  const n = changedCount(st);
  if (n === 0) return `${ahead}${behind}<span class="badge ok">干净</span>`;
  const parts = [];
  if (st.staged) parts.push(`<span class="badge s" title="已暂存 ${st.staged} 个文件">${st.staged}</span>`);
  if (st.unstaged) parts.push(`<span class="badge u" title="未暂存 ${st.unstaged} 个文件">${st.unstaged}</span>`);
  if (st.untracked) parts.push(`<span class="badge n" title="未跟踪 ${st.untracked} 个文件">${st.untracked}</span>`);
  if (st.conflicted) parts.push(`<span class="badge c" title="冲突 ${st.conflicted} 个文件">${st.conflicted}</span>`);
  return ahead + behind + parts.join('');
}

function visibleRepos() {
  const f = state.repoFilter.trim().toLowerCase();
  let list = state.repos.map((r) => ({ ref: r, st: state.status.get(r.path) }));
  if (state.dirtyOnly) {
    list = list.filter((x) => (x.st ? x.st.error || changedCount(x.st) > 0 : true));
  }
  if (f) {
    list = list.filter((x) =>
      (x.ref.name + ' ' + x.ref.rel + ' ' + x.ref.path + ' ' + (x.ref.root || '')).toLowerCase().includes(f)
    );
  }
  list.sort((a, b) => {
    const d = changedCount(b.st) - changedCount(a.st);
    if (d !== 0) return d;
    const ae = !!(a.st && a.st.error);
    const be = !!(b.st && b.st.error);
    if (ae !== be) return ae ? -1 : 1;
    const ri = state.roots.indexOf(a.ref.root) - state.roots.indexOf(b.ref.root);
    if (ri !== 0) return ri;
    return a.ref.rel.localeCompare(b.ref.rel, 'zh-CN');
  });
  return list;
}

function renderRepoList() {
  const list = visibleRepos();
  els.repoCount.textContent = `${list.length} 个仓库`;

  if (!state.repos.length) {
    const scanned = !!state.scan;
    els.repoList.innerHTML = `<li class="empty-item"><div class="empty" style="min-height:240px">
      <h3>${scanned ? '没找到 Git 仓库' : '还没有扫描结果'}</h3></div></li>`;
    return;
  }
  if (!list.length) {
    els.repoList.innerHTML = `<li class="empty-item"><div class="empty" style="min-height:200px">
      <h3>没有匹配的仓库</h3></div></li>`;
    return;
  }

  const html = list.map(({ ref, st }) => {
    const pending = state.pending.has(ref.path);
    const n = changedCount(st);
    const branch = st && !st.error && st.branch ? `<span class="branch">${esc(st.branch)}</span>` : '';
    // .git 是文件而不是目录：说明这是 git worktree 或者子模块
    const wt = ref.kind === 'worktree' ? '<span class="wt" title=".git 是文件：worktree 或子模块">worktree</span>' : '';
    const clean = st && !st.error && n === 0 ? ' clean' : '';
    const active = state.selectedRepo === ref.path ? ' active' : '';
    const counts = pending
      ? '<span class="skeleton" style="width:34px"></span>'
      : repoBadges(st);
    const sub = nicePath(ref.rel || ref.name);
    // 绑定了多个根目录时，副标题前加一个小标签说明它属于哪个根
    const rootTag =
      state.roots.length > 1 && ref.root
        ? `<span class="repo-root" title="${esc(ref.root)}">${esc(baseName(ref.root))}</span>`
        : '';
    return `<li class="repo-row${n > 0 ? ' dirty' : ''}${clean}${active}" data-path="${esc(ref.path)}" title="${esc(nicePath(ref.path))}">
      <div class="repo-main">
        <div class="repo-title"><span class="repo-name">${esc(ref.name)}</span>${branch}${wt}</div>
        <div class="repo-sub">${rootTag}${esc(sub)}${st && st.error ? ' · <span style="color:#ff9a94">读取失败</span>' : ''}</div>
      </div>
      <div class="repo-counts">${counts}</div>
    </li>`;
  }).join('');

  const top = els.repoList.scrollTop;
  els.repoList.innerHTML = html;
  els.repoList.scrollTop = top;
}

/* ---------- 渲染：仓库头部 ---------- */
function renderRepoHead() {
  const path = state.selectedRepo;
  const st = path ? state.status.get(path) : null;
  const ref = state.repos.find((r) => r.path === path);

  if (!path) {
    els.repoHead.innerHTML = '<span class="pane-title">文件</span>';
    return;
  }
  if (!st) {
    els.repoHead.innerHTML = `<span class="pane-title">文件</span><span class="grow"></span>
      <span style="display:flex;gap:8px;align-items:center;color:var(--dim);font-size:12px">
        <span class="spinner"></span> 读取中…</span>`;
    return;
  }
  if (st.error) {
    els.repoHead.innerHTML = `<span class="pane-title">文件</span><span class="grow"></span>
      <span style="color:#ff9a94;font-size:12px">读取失败</span>`;
    return;
  }

  const bits = [];
  if (st.branch) bits.push(esc(st.branch));
  else if (st.detached) bits.push('detached');
  else if (st.initial) bits.push('尚无提交');
  if (st.ahead) bits.push(`↑${st.ahead}`);
  if (st.behind) bits.push(`↓${st.behind}`);
  if (st.stashes) bits.push(`stash ${st.stashes}`);

  els.repoHead.innerHTML = `
    <span class="pane-title" style="text-transform:none;letter-spacing:0;font-size:12px;color:var(--text)">
      ${esc(ref ? ref.name : path)}
    </span>
    <span class="repo-bits">${bits.join(' ')}</span>
    <span class="grow"></span>
    <button class="btn small ghost" id="copyRepoBtn" title="复制仓库路径">复制路径</button>
    <button class="btn small ghost" id="revealRepoBtn" title="在资源管理器中打开">打开目录</button>`;

  $('copyRepoBtn').onclick = () => copyText(path, '仓库路径已复制');
  $('revealRepoBtn').onclick = () => invoke('reveal_path', { path }).catch((e) => toast(errText(e), true));
}

/* ---------- 渲染：文件列表 ---------- */
function groupsOf(st) {
  const entries = st.entries || [];
  const g = [
    { key: 'conflicted', label: '冲突', color: 'var(--conflict)', items: entries.filter((e) => e.conflicted) },
    { key: 'staged', label: '已暂存', color: 'var(--mod)', items: entries.filter((e) => e.staged && !e.conflicted) },
    { key: 'unstaged', label: '未暂存', color: 'var(--accent)', items: entries.filter((e) => e.unstaged && !e.conflicted) },
    { key: 'untracked', label: '未跟踪', color: 'var(--new)', items: entries.filter((e) => e.untracked) },
  ];
  return g.filter((x) => x.items.length);
}

function codeOf(entry, groupKey) {
  if (entry.untracked) return CODE['?'];
  if (entry.conflicted) return CODE.U;
  const ch = groupKey === 'staged' ? entry.x : entry.y;
  return CODE[ch] || CODE.X;
}

/** 把 status 里的 entry 转成界面用的「文件视图」对象 */
function viewOf(entry, groupKey) {
  const info = codeOf(entry, groupKey);
  return {
    path: entry.path,
    staged: groupKey === 'staged',
    untracked: groupKey === 'untracked',
    conflicted: groupKey === 'conflicted',
    code: info.text,
    additions: entry.additions,
    deletions: entry.deletions,
  };
}

function fileKey(f) {
  return `${f.untracked ? 'u' : f.staged ? 's' : 'w'}:${f.path}`;
}

function renderFileList() {
  const path = state.selectedRepo;
  const st = path ? state.status.get(path) : null;

  if (!path) {
    els.fileList.innerHTML = `<div class="empty" style="min-height:260px">
      <h3>未选择仓库</h3></div>`;
    return;
  }
  if (!st) {
    els.fileList.innerHTML = `<div class="empty" style="min-height:260px"><span class="spinner"></span>
      <p>正在读取 git status…</p></div>`;
    return;
  }
  if (st.error) {
    els.fileList.innerHTML = `<div class="notice">读取失败：${esc(st.error)}</div>`;
    return;
  }
  const groups = groupsOf(st);
  if (!groups.length) {
    els.fileList.innerHTML = `<div class="empty" style="min-height:260px">
      <h3>工作区干净</h3></div>`;
    return;
  }

  const f = state.fileFilter.trim().toLowerCase();
  const selKey = state.file ? fileKey(state.file) : '';
  let html = '';
  for (const g of groups) {
    const items = f ? g.items.filter((e) => e.path.toLowerCase().includes(f)) : g.items;
    if (!items.length) continue;
    html += `<div class="group-head"><span class="group-dot" style="background:${g.color}"></span>
      ${g.label} <span class="count">${items.length}</span></div>`;
    for (const e of items) {
      const info = codeOf(e, g.key);
      const view = viewOf(e, g.key);
      const active = fileKey(view) === selKey ? ' active' : '';
      const on = state.checked.has(e.path);
      const num =
        e.additions != null || e.deletions != null
          ? `<span class="file-num">${
              e.additions != null ? `<span class="a">+${e.additions}</span>` : ''
            }${e.deletions != null ? `<span class="d">-${e.deletions}</span>` : ''}</span>`
          : '';
      html += `<div class="file-row${active}${on ? ' picked' : ''}" data-key="${esc(fileKey(view))}" data-path="${esc(e.path)}" title="${esc(e.path)}">
        <span class="pick${on ? ' on' : ''}" role="checkbox" aria-checked="${on}" title="勾选后可一起提交（空格键切换）"></span>
        <span class="st ${info.cls}" title="${info.label}">${info.text}</span>
        ${pathHtml(e.path, 'file-path')}
        ${num}
      </div>`;
      if (e.orig) {
        html += `<div class="file-rename" title="原路径">← ${esc(e.orig)}</div>`;
      }
    }
  }
  els.fileList.innerHTML = html;

  // 把选中项滚到可见区域
  const activeEl = els.fileList.querySelector('.file-row.active');
  if (activeEl) activeEl.scrollIntoView({ block: 'nearest' });
}

/* ---------- 渲染：差异 ---------- */
function diffRow(kind, o, n, text, sign) {
  return `<div class="dl ${kind}"><span class="ln">${o || ''}</span><span class="ln">${n || ''}</span>` +
    `<span class="sign">${sign || ''}</span><span class="txt">${esc(text)}</span></div>`;
}

function diffMetaRow(text) {
  return `<div class="dl meta"><span class="ln"></span><span class="ln"></span><span class="sign"></span>` +
    `<span class="txt">${esc(text)}</span></div>`;
}

const META_PREFIXES = [
  'diff --', 'index ', '--- ', '+++ ', 'new file mode', 'deleted file mode', 'old mode',
  'new mode', 'similarity index', 'dissimilarity index', 'rename from', 'rename to',
  'copy from', 'copy to', 'Binary files', '\\ No newline',
];

function renderUnified(text) {
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  // 冲突 / 合并中的文件，git diff 给的是 --cc 两列前缀的「combined diff」，
  // 行前缀是 `++`、`+ `、` -` 这种两字符组合，hunk 头也是 `@@@ … @@@`。
  const combined = lines.some((l) => l.startsWith('@@@ -'));
  const out = [];
  let oldNo = 0;
  let newNo = 0;
  let shown = 0;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (i === lines.length - 1 && line === '') break;
    if (shown >= MAX_LIST_ROWS) {
      out.push(diffMetaRow(`… 差异超过 ${MAX_LIST_ROWS} 行，其余 ${lines.length - i} 行未渲染（可在此文件上用命令行查看）`));
      break;
    }
    shown += 1;
    if (combined && line.startsWith('@@@')) {
      const m = /\+(\d+)(?:,\d+)?/.exec(line);
      if (m) newNo = parseInt(m[1], 10);
      out.push(`<div class="hunk">${esc(line)}</div>`);
      continue;
    }
    if (line.startsWith('@@')) {
      const m = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line);
      if (m) {
        oldNo = parseInt(m[1], 10);
        newNo = parseInt(m[2], 10);
      }
      out.push(`<div class="hunk">${esc(line)}</div>`);
      continue;
    }
    if (META_PREFIXES.some((p) => line.startsWith(p))) {
      out.push(diffMetaRow(line));
      continue;
    }
    if (combined) {
      // 第一列是「相对父提交 1」，第二列是「相对父提交 2」；不认识的（比如纯空格）按上下文处理。
      const pre = line.slice(0, 2);
      const body = line.slice(2);
      const removed = pre[0] === '-';
      const added = !removed && (pre[0] === '+' || pre[1] === '+');
      const kind = removed ? 'del' : added ? 'add' : 'ctx';
      // combined diff 里只有「不带 - 的行」会出现在工作区文件中，用最后一列行号顺推
      out.push(diffRow(kind, '', removed ? '' : newNo, body, esc(pre)));
      if (!removed) newNo += 1;
      continue;
    }
    if (line.startsWith('+')) {
      out.push(diffRow('add', '', newNo, line.slice(1), '+'));
      newNo += 1;
    } else if (line.startsWith('-')) {
      out.push(diffRow('del', oldNo, '', line.slice(1), '-'));
      oldNo += 1;
    } else if (line.startsWith(' ')) {
      out.push(diffRow('ctx', oldNo, newNo, line.slice(1), ' '));
      oldNo += 1;
      newNo += 1;
    } else {
      out.push(diffMetaRow(line));
    }
  }
  return `<div class="diff${combined ? ' combined' : ''}">${out.join('')}</div>`;
}

function renderPlain(text) {
  const lines = text.replace(/\r\n/g, '\n').split('\n');
  if (lines.length && lines[lines.length - 1] === '') lines.pop();
  const out = [];
  const n = Math.min(lines.length, MAX_LIST_ROWS);
  for (let i = 0; i < n; i++) out.push(diffRow('add', '', i + 1, lines[i], '+'));
  if (lines.length > MAX_LIST_ROWS) {
    out.push(diffMetaRow(`… 文件超过 ${MAX_LIST_ROWS} 行，其余 ${lines.length - MAX_LIST_ROWS} 行未渲染`));
  }
  return `<div class="diff">${out.join('')}</div>`;
}

function emptyBlock(title, desc, extra) {
  return `<div class="empty"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.4">
      <path d="M4 4h9l4 4v12H4z" stroke-linejoin="round"/><path d="M13 4v4h4"/></svg>
      <h3>${esc(title)}</h3><p>${desc || ''}</p>${extra || ''}</div>`;
}

function renderViewTabs() {
  els.viewTabs.querySelectorAll('.tab').forEach((t) => {
    t.classList.toggle('on', t.dataset.view === state.view);
  });
}

function switchView(view) {
  state.view = view === 'graph' ? 'graph' : 'diff';
  if (state.view === 'diff') state.commit = null;
  savePrefs();
  renderViewTabs();
  renderDiffHead();
  renderDiffBody();
  if (state.view === 'graph' && state.selectedRepo) loadGraph();
}

function renderDiffHead() {
  const st = state.status.get(state.selectedRepo);
  if (state.view === 'graph') {
    const br = st && !st.error && st.branch ? `<span class="branch">${esc(st.branch)}</span>` : '';
    const search = state.selectedRepo
      ? `<div class="g-search">
          <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true">
            <circle cx="10.5" cy="10.5" r="6.5" /><path d="M15.5 15.5 21 21" stroke-linecap="round" />
          </svg>
          <input id="graphSearch" type="text" spellcheck="false" autocomplete="off"
            placeholder="搜索提交 / 作者 / 分支" value="${esc(state.graphSearch)}" />
          <button class="g-clear" id="graphClear" title="清空搜索"${state.graphSearch ? '' : ' hidden'}>×</button>
        </div>`
      : '';
    const btn = state.selectedRepo ? '<span class="actions"><button class="btn small ghost" id="graphRefreshBtn">刷新</button></span>' : '';
    els.diffHead.innerHTML = `${br}${search}<span class="grow"></span>${btn}`;
    const inp = $('graphSearch');
    if (inp) {
      inp.oninput = () => {
        state.graphSearch = inp.value;
        const clear = $('graphClear');
        if (clear) clear.hidden = !inp.value;
        renderDiffBody();
      };
      inp.onkeydown = (e) => {
        if (e.key === 'Escape') {
          e.preventDefault();
          e.stopPropagation();
          if (inp.value) {
            inp.value = '';
            state.graphSearch = '';
            renderDiffHead();
            renderDiffBody();
            focusGraphSearch();
          } else {
            inp.blur();
          }
          return;
        }
        if (e.key === 'Enter') {
          e.preventDefault();
          const g = state.graph;
          if (!g || !g.data || !state.graphSearch.trim()) return;
          const hits = searchCommits(g.data.commits, state.graphSearch);
          if (!hits.length) return;
          const c = hits[0].c;
          openCommit(c.hash, c.short, c.subject);
        }
      };
    }
    const cl = $('graphClear');
    if (cl) {
      cl.onclick = () => {
        state.graphSearch = '';
        renderDiffHead();
        renderDiffBody();
        focusGraphSearch();
      };
    }
    const b = $('graphRefreshBtn');
    if (b) b.onclick = () => loadGraph(true);
    return;
  }
  if (state.commit) {
    const c = state.commit;
    els.diffHead.innerHTML = `
      <code class="hash">${esc(c.short)}</code>
      <span class="commit-subject" title="${esc(c.subject)}">${esc(c.subject)}</span>
      <span class="grow"></span>
      <span class="actions">
        <button class="btn small danger" id="resetBtn">回滚至此处</button>
        <button class="btn small ghost" id="copyHashBtn">复制哈希</button>
        <button class="btn small ghost" id="backToGraphBtn">返回分支树</button>
      </span>`;
    $('copyHashBtn').onclick = () => copyText(c.hash, '提交哈希已复制');
    $('resetBtn').onclick = () => openResetDialog();
    $('backToGraphBtn').onclick = () => switchView('graph');
    return;
  }
  const f = state.file;
  if (!f) {
    els.diffHead.innerHTML = '<span class="pane-title">差异</span>';
    return;
  }
  const info = CODE[f.code] || CODE.X;
  els.diffHead.innerHTML = `
    <span class="st ${info.cls}" title="${info.label}">${info.text}</span>
    ${pathHtml(f.path, 'path')}
    <span class="grow"></span>
    <span class="actions">
      <button class="btn small ghost" id="copyFileBtn">复制路径</button>
      <button class="btn small ghost" id="revealFileBtn">定位文件</button>
    </span>`;
  $('copyFileBtn').onclick = () => copyText(f.path, '文件路径已复制');
  $('revealFileBtn').onclick = () => {
    const full = `${state.selectedRepo.replace(/[\\/]+$/, '')}\\${f.path.replace(/\//g, '\\')}`;
    invoke('reveal_path', { path: full }).catch((e) => toast(errText(e), true));
  };
}

function renderDiffBody() {
  if (state.view === 'graph') return renderGraph();
  if (state.commit) return renderCommitDiff();
  if (!state.selectedRepo) {
    const chips = recents()
      .map((r) => `<span class="chip" data-root="${esc(r)}" title="${esc(r)}">${esc(r)}</span>`)
      .join('');
    els.diffBody.innerHTML = emptyBlock('开始扫描', '', chips ? `<div class="quick">${chips}</div>` : '');
    els.diffBody.querySelectorAll('.chip').forEach((chip) => {
      chip.onclick = () => {
        addRoot(chip.dataset.root);
      };
    });
    return;
  }
  if (!state.file) {
    const st = state.status.get(state.selectedRepo);
    if (st && !st.error && !groupsOf(st).length) {
      els.diffBody.innerHTML = emptyBlock('工作区干净');
    } else {
      els.diffBody.innerHTML = emptyBlock('选择一个文件');
    }
    return;
  }
  const d = state.diff;
  if (!d || d.loading) {
    els.diffBody.innerHTML = `<div class="empty"><span class="spinner"></span><p>正在读取差异…</p></div>`;
    return;
  }
  if (d.error) {
    els.diffBody.innerHTML = `<div class="notice">读取差异失败：${esc(d.error)}</div>`;
    return;
  }
  const data = d.data;
  els.diffBody.scrollTop = 0;
  if (data.kind === 'binary') {
    els.diffBody.innerHTML = emptyBlock('二进制文件');
  } else if (data.kind === 'too_large') {
    els.diffBody.innerHTML = emptyBlock('文件过大');
  } else if (data.kind === 'empty') {
    els.diffBody.innerHTML = emptyBlock('没有差异');
  } else if (data.kind === 'text') {
    els.diffBody.innerHTML = renderPlain(data.text || '');
  } else {
    els.diffBody.innerHTML = renderUnified(data.text || '');
  }
}

/* ---------- 分支树 ---------- */

/**
 * 车道布局：按顺序给每个提交分配车道。
 * `before` 记录这一行开始时各车道在等哪个提交（用来画上下贯通的线），
 * `after` 记录把父提交排好位之后的状态（用来找父提交落在哪条车道）。
 */
function layoutGraph(commits) {
  const lanes = [];
  const rows = [];
  for (const c of commits) {
    const before = lanes.slice();
    let lane = lanes.indexOf(c.hash);
    if (lane === -1) {
      // 还没人等的提交（分支尖端）：占一个空车道，没有就新开一条
      lane = lanes.indexOf(null);
      if (lane === -1) {
        lane = lanes.length;
        lanes.push(null);
      }
      lanes[lane] = c.hash;
    }
    for (let k = 0; k < lanes.length; k++) {
      if (k !== lane && lanes[k] === c.hash) lanes[k] = null;
    }
    lanes[lane] = null;
    // 父提交：已经在某个车道等着的就复用那条车道，否则占当前车道
    const parentLanes = [];
    for (const p of c.parents) {
      let k = lanes.indexOf(p);
      if (k === -1) {
        if (lanes[lane] === null) {
          k = lane;
          lanes[lane] = p;
        } else {
          k = lanes.indexOf(null);
          if (k === -1) {
            k = lanes.length;
            lanes.push(p);
          } else {
            lanes[k] = p;
          }
        }
      }
      parentLanes.push(k);
    }
    rows.push({ c, lane, before, after: lanes.slice(), parentLanes });
  }
  return rows;
}

function graphSvg(rows, maxLane) {
  const H = G_ROW_H;
  const color = (k) => G_COLORS[k % G_COLORS.length];
  const pos = new Map();
  rows.forEach((r, i) => {
    if (!pos.has(r.c.hash)) pos.set(r.c.hash, i);
  });
  const out = [];
  rows.forEach((r, i) => {
    const top = i * H;
    const mid = top + H / 2;
    const bot = top + H;
    const lw = 2;
    // 只是路过的车道：竖线贯穿这一行
    for (let k = 0; k < maxLane; k++) {
      if (k === r.lane) continue;
      const hasTop = !!r.before[k];
      const hasBot = !!r.after[k];
      if (!hasTop && !hasBot) continue;
      const x = k * G_LANE_W + G_LANE_W / 2;
      const y1 = hasTop ? top : mid;
      const y2 = hasBot ? bot : mid;
      if (y2 > y1) {
        out.push(`<path d="M${x} ${y1}V${y2}" stroke="${color(k)}" stroke-width="${lw}" opacity=".5"/>`);
      }
    }
    const x = r.lane * G_LANE_W + G_LANE_W / 2;
    if (r.before[r.lane]) {
      out.push(`<path d="M${x} ${top}V${mid}" stroke="${color(r.lane)}" stroke-width="${lw}" opacity=".5"/>`);
    }
    // 父提交连线：同车道直下，跨车道画曲线
    r.c.parents.forEach((ph, idx) => {
      const k = r.parentLanes[idx];
      if (k === undefined || k === null) return;
      const px = k * G_LANE_W + G_LANE_W / 2;
      if (k === r.lane) {
        out.push(`<path d="M${x} ${mid}V${bot}" stroke="${color(k)}" stroke-width="${lw}" opacity=".5"/>`);
        return;
      }
      const j = pos.get(ph);
      const py = j === undefined ? bot - H / 2 : j * H + H / 2;
      out.push(
        `<path d="M${x} ${mid}C${x} ${py},${px} ${mid},${px} ${py}" ` +
          `stroke="${color(k)}" stroke-width="${lw + 0.2}" fill="none" opacity=".85"/>`
      );
    });
    out.push(
      `<circle cx="${x}" cy="${mid}" r="4.2" fill="${color(r.lane)}" stroke="#061017" stroke-width="2"/>`
    );
  });
  return out.join('');
}

function refBadges(refs) {
  if (!refs) return '';
  return refs
    .split(',')
    .map((s) => s.trim())
    .filter(Boolean)
    .map((r) => {
      if (r.includes('->')) {
        const target = r.split('->').pop().trim();
        const head = r.startsWith('HEAD');
        return `<span class="g-ref ${head ? 'head' : 'remote'}">${esc(target)}</span>`;
      }
      if (r.startsWith('tag: ')) return `<span class="g-ref tag">${esc(r.slice(5))}</span>`;
      return `<span class="g-ref ${r.includes('/') ? 'remote' : 'branch'}">${esc(r)}</span>`;
    })
    .join('');
}

function graphRowHtml(row, hit) {
  const c = row.c;
  const sel = state.commit && state.commit.hash === c.hash ? ' sel' : '';
  const subject = hit && hit.marks.subject ? highlight(c.subject, hit.marks.subject) : esc(c.subject);
  const author = hit && hit.marks.author ? highlight(c.author, hit.marks.author) : esc(c.author);
  return `<div class="g-row${sel}" data-hash="${c.hash}" data-short="${esc(c.short)}" data-subject="${esc(
    c.subject
  )}">
    <span class="g-hash">${esc(c.short)}</span>
    ${refBadges(c.refs)}
    <span class="g-subject">${subject}</span>
    <span class="g-meta">${author} · ${esc(c.date)}</span>
  </div>`;
}

/* ---------- 分支树模糊搜索 ---------- */

// 算「词首」用的分隔符：命中这些字符后面的字额外加分
const FUZZ_SEPS = new Set([' ', '-', '_', '/', '.', ':', ',', '(', ')', '[', ']', '·', '—', '，', '。', '：', '、', '「', '」']);

/** 子序列模糊匹配：query 的字符要按顺序出现在 text 里（不要求连续）。
 *  连续命中 / 词首命中 / 首字符命中加分，中间跳过太多扣分。返回 { score, positions } 或 null。 */
function fuzzyMatch(query, text) {
  if (!query || !text) return null;
  const q = query.toLowerCase();
  const low = text.toLowerCase();
  if (q.length > low.length) return null;
  const positions = [];
  let score = 0;
  let qi = 0;
  let lastHit = -2;
  for (let i = 0; i < low.length && qi < q.length; i++) {
    if (low[i] !== q[qi]) continue;
    const prev = i > 0 ? text[i - 1] : '';
    const isWordStart = i === 0 || FUZZ_SEPS.has(prev) || (prev !== prev.toUpperCase() && text[i] !== text[i].toLowerCase());
    score += 1;
    if (i === lastHit + 1) score += 3;            // 紧挨着上一个命中
    if (isWordStart) score += 3;                  // 落在词首
    if (i === 0) score += 2;                      // 落在开头
    if (lastHit >= 0 && i > lastHit + 1) score -= Math.min(2.5, (i - lastHit - 1) * 0.25);
    positions.push(i);
    lastHit = i;
    qi++;
  }
  if (qi < q.length) return null;                 // 没凑齐，不算命中
  score -= text.length * 0.02;                    // 越短越精确
  return { score, positions };
}

const SEARCH_FIELDS = [
  { key: 'hash', weight: 1.0 },
  { key: 'subject', weight: 1.0 },
  { key: 'refs', weight: 0.85 },
  { key: 'author', weight: 0.7 },
  { key: 'date', weight: 0.5 },
];

function commitFields(c) {
  return {
    hash: `${c.short || ''} ${c.hash || ''}`,
    subject: c.subject || '',
    refs: (c.refs || '').replace(/tag:\s*/g, ''),
    author: c.author || '',
    date: c.date || '',
  };
}

/** 空格分开的每个词都要命中（命中在不同字段也算），按得分从高到低排 */
function searchCommits(commits, rawQuery) {
  const terms = String(rawQuery || '').toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return [];
  const out = [];
  commits.forEach((c, idx) => {
    const fields = commitFields(c);
    const marks = {};
    let total = 0;
    for (const term of terms) {
      let best = null;
      for (const f of SEARCH_FIELDS) {
        const m = fuzzyMatch(term, fields[f.key]);
        if (!m) continue;
        const s = m.score * f.weight;
        if (!best || s > best.score) best = { score: s, key: f.key, positions: m.positions };
      }
      if (!best) return; // 这个词哪个字段都没命中
      total += best.score;
      if (best.key === 'subject' || best.key === 'author') {
        (marks[best.key] = marks[best.key] || []).push(...best.positions);
      }
    }
    out.push({ c, idx, score: total, marks });
  });
  out.sort((a, b) => b.score - a.score || a.idx - b.idx);
  return out;
}

/** 把命中的字符包成 <span class="hl">（先切再转义，避免把正文字符夹断） */
function highlight(text, positions) {
  if (!positions || !positions.length || !text) return esc(text || '');
  const set = [...new Set(positions)].filter((p) => p >= 0 && p < text.length).sort((a, b) => a - b);
  let out = '';
  let last = 0;
  for (const p of set) {
    if (p < last) continue;
    out += esc(text.slice(last, p)) + `<span class="hl">${esc(text[p])}</span>`;
    last = p + 1;
  }
  return out + esc(text.slice(last));
}

function focusGraphSearch() {
  if (state.view !== 'graph') return;
  const inp = $('graphSearch');
  if (!inp) return;
  inp.focus();
  inp.select();
}

function renderGraph() {
  if (!state.selectedRepo) {
    els.diffBody.innerHTML = emptyBlock('选择一个仓库');
    return;
  }
  const g = state.graph;
  if (!g || g.loading) {
    els.diffBody.innerHTML = `<div class="empty"><span class="spinner"></span></div>`;
    return;
  }
  if (g.error) {
    els.diffBody.innerHTML = `<div class="notice">读取失败：${esc(g.error)}</div>`;
    return;
  }
  const data = g.data;
  if (!data.commits.length) {
    els.diffBody.innerHTML = emptyBlock('还没有提交');
    return;
  }

  // 搜到了就摊成平铺列表（车道线在过滤后的子集里没有意义）
  const q = state.graphSearch.trim();
  if (q) {
    const hits = searchCommits(data.commits, q);
    if (!hits.length) {
      els.diffBody.innerHTML = `<div class="graph flat">
        <div class="flat-bar">没有匹配「${esc(q)}」的提交</div>
        <div class="empty"><h3>换个词试试</h3><p>可以搜提交标题、作者、分支或标签名，多个词用空格分开。</p></div>
      </div>`;
      return;
    }
    els.diffBody.innerHTML = `<div class="graph flat">
      <div class="flat-bar">${hits.length} 个提交匹配「${esc(q)}」</div>
      <div class="graph-rows">${hits.map((h) => graphRowHtml({ c: h.c }, h)).join('')}</div>
    </div>`;
    return;
  }

  const rows = layoutGraph(data.commits);
  const maxLane = rows.reduce((m, r) => Math.max(m, r.before.length, r.after.length, r.lane + 1), 1);
  const width = maxLane * G_LANE_W;
  const height = rows.length * G_ROW_H;
  const more = data.truncated
    ? `<div class="graph-more" style="margin-left:${width}px">只显示最近 ${data.commits.length} 个提交</div>`
    : '';
  els.diffBody.innerHTML = `<div class="graph">
      <svg width="${width}" height="${height}" viewBox="0 0 ${width} ${height}">${graphSvg(rows, maxLane)}</svg>
      <div class="graph-rows" style="margin-left:${width}px">${rows.map((r) => graphRowHtml(r)).join('')}</div>
      ${more}
    </div>`;
}

async function loadGraph(force) {
  if (!state.selectedRepo) {
    state.graph = null;
    return;
  }
  const repo = state.selectedRepo;
  if (!force && state.graph && state.graph.data) {
    renderDiffBody();
    return;
  }
  state.graph = { loading: true };
  renderDiffBody();
  try {
    const data = await invoke('repo_graph', { path: repo, limit: 300 });
    if (state.selectedRepo !== repo) return;
    state.graph = { data };
  } catch (e) {
    if (state.selectedRepo !== repo) return;
    state.graph = { error: errText(e) };
  }
  renderDiffBody();
}

/* ---------- 提交补丁 ---------- */

async function openCommit(hash, short, subject) {
  if (!state.selectedRepo) return;
  state.commit = { hash, short: short || hash.slice(0, 7), subject: subject || '' };
  state.commitDiff = { loading: true };
  state.view = 'diff';
  renderViewTabs();
  renderDiffHead();
  renderDiffBody();
  try {
    const data = await invoke('commit_diff', { path: state.selectedRepo, hash });
    if (!state.commit || state.commit.hash !== hash) return;
    state.commitDiff = { data };
  } catch (e) {
    if (!state.commit || state.commit.hash !== hash) return;
    state.commitDiff = { error: errText(e) };
  }
  renderDiffBody();
}

function renderCommitDiff() {
  const d = state.commitDiff;
  if (!d || d.loading) {
    els.diffBody.innerHTML = `<div class="empty"><span class="spinner"></span></div>`;
    return;
  }
  if (d.error) {
    els.diffBody.innerHTML = `<div class="notice">读取失败：${esc(d.error)}</div>`;
    return;
  }
  const data = d.data;
  els.diffBody.scrollTop = 0;
  if (data.kind === 'binary') {
    els.diffBody.innerHTML = emptyBlock('二进制改动', esc(data.message || ''));
  } else if (data.kind === 'too_large') {
    els.diffBody.innerHTML = emptyBlock('改动太大', esc(data.message || ''));
  } else if (data.kind === 'empty') {
    els.diffBody.innerHTML = emptyBlock('没有内容变化', esc(data.message || ''));
  } else {
    els.diffBody.innerHTML = renderUnified(data.text || '');
  }
}

/* ---------- 回滚到某个提交 ---------- */

const RESET_MODES = {
  mixed: '混合',
  soft: '软',
  hard: '硬',
};

const RESET_HELP = {
  mixed: '撤销提交，改动留在工作区（还没暂存）',
  soft: '撤销提交，改动留在暂存区，可以直接重新提交',
  hard: '撤销提交，并丢弃已跟踪文件的未提交改动',
};

async function openResetDialog() {
  const repo = state.selectedRepo;
  const c = state.commit;
  if (!repo || !c || state.busy) return;

  let plan;
  try {
    plan = await invoke('reset_plan', { path: repo, hash: c.hash });
  } catch (e) {
    return showError('读不到回滚信息', errText(e));
  }

  const from = plan.head ? esc(plan.head) : 'HEAD';
  const to = esc(plan.short);
  const where = plan.detached
    ? 'detached HEAD（不在任何分支上）'
    : `分支 <span class="code">${esc(plan.branch || '')}</span>`;

  const facts = [];
  if (plan.drop_commits) facts.push(`撤销 <b>${plan.drop_commits}</b> 个提交`);
  if (plan.gain_commits) facts.push(`这个提交上有 <b>${plan.gain_commits}</b> 个提交不在当前分支里`);
  if (plan.dirty) facts.push(`还有 <b>${plan.dirty}</b> 处未提交的改动`);

  const opts = Object.keys(RESET_MODES)
    .map(
      (k) => `<label class="reset-opt">
        <input type="radio" name="resetMode" value="${k}"${k === 'mixed' ? ' checked' : ''} />
        <span><b>${RESET_MODES[k]}</b>${RESET_HELP[k]}</span>
      </label>`,
    )
    .join('');

  const html = `
    <p class="dim">${where} 从 <span class="code">${from}</span> 退到
      <span class="code">${to}</span>「${esc(plan.subject)}」</p>
    ${
      plan.short === plan.head
        ? '<p class="dim">目标就是当前提交，只会按所选方式处理下面这些未提交改动。</p>'
        : ''
    }
    ${facts.length ? `<p class="dim">${facts.join(' · ')}</p>` : ''}
    <div class="reset-opts">${opts}</div>
    <p class="reset-warn">撤销掉的提交只留在 reflog 里，本程序不提供撤销；未跟踪的新文件不会删除。</p>
    <label class="reset-confirm" id="resetConfirmWrap" hidden>
      <input type="checkbox" id="resetConfirm" /> 我确认这些未提交改动会被丢弃
    </label>`;

  const answer = askModal({
    title: '回滚到这个提交？',
    okText: '回滚',
    html,
  });

  // 选「硬」才要求额外勾选确认，避免顺手点到
  const body = els.modalBody;
  const confirmWrap = body.querySelector('#resetConfirmWrap');
  const confirmBox = body.querySelector('#resetConfirm');
  const syncMode = () => {
    const hard = (body.querySelector('input[name="resetMode"]:checked') || {}).value === 'hard';
    confirmWrap.hidden = !hard || !plan.dirty;
    if (confirmBox) confirmBox.checked = false;
    els.modalOk.disabled = hard && plan.dirty > 0;
  };
  body.querySelectorAll('input[name="resetMode"]').forEach((r) => (r.onchange = syncMode));
  if (confirmBox) confirmBox.onchange = () => {
    els.modalOk.disabled = !confirmBox.checked;
  };
  syncMode();

  const ok = await answer;
  if (!ok) return;

  const mode = (body.querySelector('input[name="resetMode"]:checked') || {}).value || 'mixed';
  state.busy = true;
  renderCommitBar();
  try {
    const res = await invoke('reset_repo', { path: repo, hash: c.hash, mode });
    state.commit = null;
    state.commitDiff = null;
    state.graph = null;
    await refreshRepo(repo);
    switchView('graph');
    loadGraph(true);
    toast(`已回滚至 ${res.short}（${RESET_MODES[mode] || mode}）`);
    const extra = res.drop_commits ? ` · 撤销 ${res.drop_commits} 个提交` : '';
    els.statusLeft.innerHTML = `回滚完成 · ${esc(res.branch || 'detached')} → ${esc(res.short)}${extra}`;
  } catch (e) {
    showError('回滚失败', errText(e));
  } finally {
    state.busy = false;
    renderCommitBar();
  }
}

/* ---------- 选择逻辑 ---------- */
function pickFirstEntry(st) {
  if (!st || st.error) return null;
  const groups = groupsOf(st);
  if (!groups.length) return null;
  const g = groups[0];
  return viewOf(g.items[0], g.key);
}

function selectRepo(path) {
  if (path !== state.selectedRepo) state.checked.clear();
  state.selectedRepo = path;
  state.graphSearch = '';
  state.fileFilter = '';
  els.fileFilter.value = '';
  state.file = pickFirstEntry(state.status.get(path));
  state.diff = null;
  state.commit = null;
  state.commitDiff = null;
  state.graph = null;
  renderRepoList();
  renderRepoHead();
  renderFileList();
  renderCommitBar();
  renderDiffHead();
  renderDiffBody();
  if (state.view === 'graph') loadGraph();
  else if (state.file) loadDiff();
}

function selectFile(key) {
  const st = state.status.get(state.selectedRepo);
  if (!st || st.error) return;
  for (const g of groupsOf(st)) {
    for (const e of g.items) {
      const view = viewOf(e, g.key);
      if (fileKey(view) === key) {
        state.file = view;
        state.commit = null;
        state.view = 'diff';
        renderViewTabs();
        renderFileList();
        renderDiffHead();
        loadDiff();
        return;
      }
    }
  }
}

async function loadDiff() {
  const f = state.file;
  if (!f || !state.selectedRepo) {
    state.diff = null;
    renderDiffBody();
    return;
  }
  const token = ++state.diffToken;
  state.diff = { loading: true };
  renderDiffBody();
  try {
    const data = await invoke('file_diff', {
      path: state.selectedRepo,
      file: f.path,
      staged: !!f.staged,
      untracked: !!f.untracked,
    });
    if (token !== state.diffToken) return;
    state.diff = { data };
  } catch (e) {
    if (token !== state.diffToken) return;
    state.diff = { error: errText(e) };
  }
  renderDiffBody();
}

/* ---------- 复制 ---------- */
async function copyText(text, okMsg) {
  try {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      await navigator.clipboard.writeText(text);
    } else {
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      ta.remove();
    }
    toast(okMsg || '已复制');
  } catch (e) {
    toast(`复制失败：${errText(e)}`, true);
  }
}

/* ---------- 弹窗 ---------- */
let modalDone = null;

function closeModal(result) {
  els.modalWrap.hidden = true;
  const done = modalDone;
  modalDone = null;
  if (done) done(result);
}

/** 打开一个确认框；resolve(true/false)。focus 可指定打开后聚焦的元素选择器 */
function askModal({ title, html, okText, cancelText, showCancel, focus, wide }) {
  if (modalDone) closeModal(false);
  return new Promise((resolve) => {
    modalDone = resolve;
    els.modalTitle.textContent = title || '';
    els.modalBody.innerHTML = html || '';
    els.modalOk.textContent = okText || '确定';
    els.modalOk.disabled = false;
    els.modalCancel.textContent = cancelText || '取消';
    els.modalCancel.hidden = showCancel === false;
    const box = els.modalWrap.querySelector('.modal');
    if (box) box.classList.toggle('wide', !!wide);
    els.modalWrap.hidden = false;
    const target = focus ? els.modalBody.querySelector(focus) : null;
    if (target) {
      target.focus();
      if (target.setSelectionRange && typeof target.value === 'string') {
        const n = target.value.length;
        target.setSelectionRange(n, n);
      }
    } else {
      els.modalOk.focus();
    }
  });
}

/** 把 git 的原始报错原样摊给用户（错误信息经常很长，toast 装不下） */
function showError(title, text) {
  return askModal({
    title,
    html: `<div class="raw">${esc(text || '（没有任何输出）')}</div>`,
    okText: '知道了',
    showCancel: false,
  });
}

/* ---------- 提交 / 推送 ---------- */
function repoStatus() {
  const p = state.selectedRepo;
  return p ? state.status.get(p) : null;
}

function renderCommitBar() {
  const path = state.selectedRepo;
  const st = repoStatus();
  const entries = st && !st.error ? st.entries || [] : [];

  // 已经不在 status 里的勾选（提交掉 / 被删掉）自动丢弃
  if (entries.length) {
    const alive = new Set(entries.map((e) => e.path));
    for (const p of [...state.checked]) if (!alive.has(p)) state.checked.delete(p);
  }

  const n = state.checked.size;
  const total = distinctCount(st);
  const branch = st && !st.error ? st.branch || (st.detached ? 'detached' : st.initial ? '尚无提交' : '') : '';
  let text;
  if (!path) text = '未选择仓库';
  else if (!st) text = '读取中…';
  else if (st.error) text = '读取失败';
  else {
    text = `已选 <b>${n}</b> / ${total} 个文件`;
    if (branch) text += ` · ${esc(branch)}`;
    if (st.ahead) text += ` · <span style="color:var(--mod)">↑${st.ahead}</span>`;
  }
  els.commitSel.innerHTML = text;

  const canWrite = !!(path && st && !st.error);
  const hasRemote = canWrite && (st.remotes || []).length > 0;
  els.commitBtn.disabled = !canWrite || n === 0 || state.busy;
  els.commitBtn.textContent = state.busy ? '处理中…' : '提交选中';
  els.selectAllBtn.disabled = !canWrite || !total || n === total || state.busy;
  els.selectAllBtn.title = total ? `勾选全部 ${total} 个文件` : '勾选当前仓库的全部文件';
  els.selectNoneBtn.disabled = !canWrite || n === 0 || state.busy;
  els.selectNoneBtn.title = n ? `取消已勾选的 ${n} 个文件` : '取消所有勾选';
  // 文件列表顶部那颗「全选」：全选 / 半选 / 未选三态
  els.pickAll.disabled = !canWrite || !total || state.busy;
  els.pickAll.checked = total > 0 && n === total;
  els.pickAll.indeterminate = n > 0 && n < total;
  els.pickAll.title = total ? `全选（共 ${total} 个文件，已选 ${n}）` : '勾选本仓库的全部文件';
  els.listSelectNoneBtn.disabled = !canWrite || n === 0 || state.busy;
  els.listSelectNoneBtn.title = n ? `清空已勾选的 ${n} 个文件` : '清空勾选';
  els.commitExpandBtn.disabled = state.busy;
  // 没配远程的仓库就别让用户点进去再吃一次报错
  els.pushBtn.disabled = !hasRemote || state.busy;
  els.pushBtn.textContent = canWrite && st.ahead ? `推送 ↑${st.ahead}` : '推送';
  els.pushBtn.title = !canWrite
    ? '推送'
    : !hasRemote
      ? '没有远程仓库'
      : st.upstream
        ? `推送到 ${st.upstream}`
        : `推送到 ${(st.remotes || [])[0]}（未设上游）`;

  // 拉取：detached / 尚无提交 / 没远程都不能拉
  els.pullBtn.disabled = !canWrite || !hasRemote || st.detached || st.initial || state.busy;
  els.pullBtn.textContent = canWrite && st.behind ? `拉取 ↓${st.behind}` : '拉取';
  els.pullBtn.title = !canWrite
    ? '拉取'
    : !hasRemote
      ? '没有远程仓库'
      : st.detached
        ? 'detached HEAD，先切回分支'
        : st.initial
          ? '还没有任何提交'
          : st.upstream
            ? `从 ${st.upstream} 拉取`
            : `从 ${(st.remotes || [])[0]} 拉取（未设上游）`;
}

function toggleChecked(path) {
  if (state.checked.has(path)) state.checked.delete(path);
  else state.checked.add(path);
  renderFileList();
  renderCommitBar();
}

/** 勾选当前仓库的全部文件 */
function checkAll() {
  const st = repoStatus();
  if (!st || st.error) return;
  const all = [...new Set((st.entries || []).map((e) => e.path))];
  if (!all.length) return;
  for (const p of all) state.checked.add(p);
  renderFileList();
  renderCommitBar();
}

/** 取消所有勾选 */
function checkNone() {
  if (!state.checked.size) return;
  state.checked.clear();
  renderFileList();
  renderCommitBar();
}

/** 重新读一个仓库的 status（提交 / 推送之后用），顺手修正当前选中的文件 */
async function refreshRepo(path) {
  try {
    const st = await invoke('repo_status', { path });
    state.status.set(path, st);
  } catch (e) {
    state.status.set(path, { error: errText(e) });
  }
  if (state.selectedRepo === path) {
    const st = state.status.get(path);
    const keys = new Set();
    if (st && !st.error) for (const g of groupsOf(st)) for (const e of g.items) keys.add(fileKey(viewOf(e, g.key)));
    if (!state.file || !keys.has(fileKey(state.file))) {
      state.file = pickFirstEntry(st);
      state.diff = null;
      renderDiffHead();
      renderDiffBody();
      if (state.file) loadDiff();
    }
  }
  renderStats();
  renderRepoList();
  renderRepoHead();
  renderFileList();
  renderCommitBar();
}

/** 用大文本框编辑提交信息；确认后写回底部那条固定高度的小输入框 */
let editCommitAfter = false;
async function editMessage() {
  editCommitAfter = false;
  const ok = await askModal({
    title: '编辑提交信息',
    okText: '用这条信息',
    focus: '#msgEditor',
    html: `<textarea id="msgEditor" class="msg-editor" spellcheck="false"
        placeholder="提交信息">${esc(els.commitMsg.value)}</textarea>
      <p class="dim">Ctrl+Enter 提交 · Esc 取消</p>`,
  });
  if (!ok) {
    editCommitAfter = false;
    return;
  }
  const ta = els.modalBody.querySelector('#msgEditor');
  if (ta) {
    els.commitMsg.value = ta.value.replace(/\r\n/g, '\n');
    els.commitMsg.scrollTop = 0;
  }
  els.commitMsg.focus();
  if (editCommitAfter) {
    editCommitAfter = false;
    doCommit();
  }
}

async function doCommit() {
  const path = state.selectedRepo;
  const st = repoStatus();
  if (!path) return toast('先选一个仓库', true);
  if (!st) return toast('status 还在读取', true);
  if (st.error) return toast('读取失败，没法提交', true);
  if (state.busy) return;

  const files = [...state.checked];
  if (!files.length) return toast('先勾选文件', true);
  const message = els.commitMsg.value.trim();
  if (!message) {
    toast('提交信息不能为空', true);
    els.commitMsg.focus();
    return;
  }

  const shown = files.slice(0, 12).map((f) => `<li>${esc(f)}</li>`).join('');
  const more = files.length > 12 ? `<li>… 还有 ${files.length - 12} 个文件</li>` : '';
  const ok = await askModal({
    title: '确认提交',
    html: `<p>把 <b>${files.length}</b> 个文件提交到 <code>${esc(st.branch || '当前分支')}</code></p>
      <div class="msg-preview">${esc(message)}</div>
      <ul class="file-preview">${shown}${more}</ul>`,
    okText: '提交',
  });
  if (!ok) return;

  state.busy = true;
  renderCommitBar();
  try {
    const res = await invoke('commit_files', { path, files, message });
    state.checked.clear();
    els.commitMsg.value = '';
    toast(`已提交 ${res.hash}${res.subject ? `：${res.subject}` : ''}`);
    els.statusLeft.innerHTML = `提交成功 <b>${esc(res.hash)}</b>${
      res.subject ? ` · ${esc(res.subject)}` : ''
    }${res.branch ? ` · ${esc(res.branch)}` : ''}`;
    await refreshRepo(path);
  } catch (e) {
    els.statusLeft.innerHTML = '<span class="err">提交失败</span>';
    await showError('提交失败', errText(e));
  } finally {
    state.busy = false;
    renderCommitBar();
  }
}

async function doPush() {
  const path = state.selectedRepo;
  const st = repoStatus();
  if (!path) return toast('先选一个仓库', true);
  if (!st) return toast('status 还在读取', true);
  if (st.error) return toast('读取失败，没法推送', true);
  if (!(st.remotes || []).length) return toast('没有远程仓库', true);
  if (state.busy) return;

  const target = st.upstream
    ? `到 <code>${esc(st.upstream)}</code>`
    : `到 <code>${esc(st.remotes[0])}</code>/${esc(st.branch || '')}`;
  const ok = await askModal({
    title: '确认推送',
    html: `<p>把 <code>${esc(st.branch || '当前分支')}</code> 推${target}？</p>
      <p class="dim">${
        st.ahead ? `待推送 ${st.ahead} 个提交` : '没有待推送的提交'
      }${st.behind ? ` · 远程多 ${st.behind} 个提交` : ''}</p>`,
    okText: '推送',
  });
  if (!ok) return;

  state.busy = true;
  renderCommitBar();
  try {
    const res = await invoke('push_repo', { path });
    toast(`已推送 ${res.branch}`);
    els.statusLeft.innerHTML = `推送成功 · <b>${esc(res.branch)}</b>${
      res.set_upstream ? ` → ${esc(res.remote)}` : res.upstream ? ` → ${esc(res.upstream)}` : ''
    }`;
    await refreshRepo(path);
    const lines = (res.output || '').split('\n').filter((l) => l.trim());
    if (lines.length > 1) {
      await askModal({
        title: '推送完成',
        html: `<div class="raw" style="color:#bfe6c8;background:#0d1a12;border-color:rgba(63,185,80,.28)">${esc(
          res.output
        )}</div>`,
        okText: '知道了',
        showCancel: false,
      });
    }
  } catch (e) {
    els.statusLeft.innerHTML = '<span class="err">推送失败</span>';
    await showError('推送失败', errText(e));
  } finally {
    state.busy = false;
    renderCommitBar();
  }
}

async function doPull() {
  const path = state.selectedRepo;
  const st = repoStatus();
  if (!path) return toast('先选一个仓库', true);
  if (!st) return toast('status 还在读取', true);
  if (st.error) return toast('读取失败，没法拉取', true);
  if (!(st.remotes || []).length) return toast('没有远程仓库', true);
  if (st.detached) return toast('detached HEAD，先切回分支', true);
  if (state.busy) return;

  const from = st.upstream || `${(st.remotes || [])[0]}/${st.branch || ''}`;
  const ok = await askModal({
    title: '确认拉取',
    html: `<p>把 <code>${esc(from)}</code> 拉到 <code>${esc(st.branch || '当前分支')}</code>？</p>
      <p class="dim">${
        st.behind ? `远程多 ${st.behind} 个提交` : '远程没有新提交'
      }${st.ahead ? ` · 本地多 ${st.ahead} 个提交` : ''}</p>`,
    okText: '拉取',
  });
  if (!ok) return;

  state.busy = true;
  renderCommitBar();
  try {
    const res = await invoke('pull_repo', { path });
    toast(`已拉取 · ${res.summary}`);
    els.statusLeft.innerHTML = `拉取完成 · <b>${esc(res.branch)}</b> ← ${esc(
      res.upstream || res.remote || ''
    )} · ${esc(res.summary)}`;
    await refreshRepo(path);
    if (state.view === 'graph') loadGraph(true);
  } catch (e) {
    els.statusLeft.innerHTML = '<span class="err">拉取失败</span>';
    await showError('拉取失败', errText(e));
  } finally {
    state.busy = false;
    renderCommitBar();
  }
}

/* ---------- 待推送面板 ---------- */
function pushRowHtml({ ref, st }) {  const ahead = st.ahead || 0;
  const remote = (st.remotes || [])[0] || '';
  const up = st.upstream
    ? `<code>${esc(st.upstream)}</code>`
    : `${esc(remote || '无远程')} <span class="dim">（未设上游）</span>`;
  return `<li class="push-row" data-path="${esc(ref.path)}">
    <div class="push-main">
      <div class="push-title"><span class="push-name">${esc(ref.name)}</span>${
        st.branch ? `<span class="branch">${esc(st.branch)}</span>` : ''
      }</div>
      <div class="push-sub" title="${esc(nicePath(ref.path))}">${esc(nicePath(ref.path))}</div>
    </div>
    <div class="push-meta">
      <span class="push-ahead" title="待推送 ${ahead} 个提交">↑${ahead}</span>
      <span class="push-up">${up}</span>
    </div>
    <span class="push-state"></span>
    <button class="btn small" data-push>推送</button>
  </li>`;
}

/** 推一个仓库，返回 { ok, msg } */
async function pushOne(path) {
  try {
    const res = await invoke('push_repo', { path });
    await refreshRepo(path);
    const to = res.upstream || res.remote || '远程';
    return { ok: true, msg: `已推送 → ${to}` };
  } catch (e) {
    return { ok: false, msg: errText(e) };
  }
}

let boardBusy = false;

/** 依次推送这些行（行来自待推送面板） */
async function pushRows(rows) {
  if (!rows.length || boardBusy) return;
  boardBusy = true;
  state.busy = true;
  els.modalOk.disabled = true;
  renderCommitBar();
  let okCount = 0;
  let failCount = 0;
  try {
    for (const row of rows) {
      const path = row.dataset.path;
      const btn = row.querySelector('[data-push]');
      const stateEl = row.querySelector('.push-state');
      if (btn) btn.disabled = true;
      if (stateEl) {
        stateEl.textContent = '推送中…';
        stateEl.className = 'push-state run';
      }
      const res = await pushOne(path);
      if (stateEl) {
        stateEl.textContent = res.ok ? '已推送' : res.msg;
        stateEl.className = `push-state ${res.ok ? 'ok' : 'err'}`;
      }
      if (res.ok) okCount += 1;
      else failCount += 1;
    }
  } finally {
    boardBusy = false;
    state.busy = false;
    els.modalOk.disabled = false;
    renderStats();
    renderRepoList();
    renderCommitBar();
  }

  const left = pushTargets().length;
  els.pushEntry.hidden = left === 0;
  if (okCount && failCount) toast(`推送完成：${okCount} 个成功，${failCount} 个失败`, true);
  else if (okCount) toast(`已推送 ${okCount} 个仓库`);
  else if (failCount) toast(`推送失败：${failCount} 个仓库`, true);
  if (!left && !els.modalWrap.hidden) {
    await askModal({
      title: '推送完成',
      html: '<p>没有待推送的提交了。</p>',
      okText: '知道了',
      showCancel: false,
    });
  }
}

async function openPushBoard() {
  const list = pushTargets();
  if (!list.length) {
    toast('现在没有需要推送的仓库');
    return;
  }
  const total = list.reduce((a, x) => a + (x.st.ahead || 0), 0);
  await askModal({
    title: '哪些仓库需要推送',
    okText: '关闭',
    showCancel: false,
    html: `<p class="push-sum">共 <b>${list.length}</b> 个仓库、<b>${total}</b> 个提交待推送</p>
      <ul class="push-list">${list.map(pushRowHtml).join('')}</ul>
      <div class="push-actions"><button class="btn primary small" id="pushAllBtn">全部推送（${list.length} 个）</button></div>`,
  });
}


/* ---------- 发版前巡检 ---------- */

/** 巡检选项：分支规则跟基准分支存在本地，下次打开还在 */
const AUDIT_SKIP_OPTIONS = [
  { id: 'stash', label: '忽略 stash' },
  { id: 'unmerged', label: '忽略遗留分支' },
  { id: 'behind', label: '忽略落后远程' },
];
const AUDIT_PREF_KEY = 'git-tracer.audit.v1';

const auditState = {
  report: null,
  markdown: '',
  command: '',
  error: '',
  running: false,
  branch: '',
  base: '',
  depth: '',
  useConfig: true,
  skip: [],
};

function loadAuditPrefs() {
  try {
    const o = JSON.parse(localStorage.getItem(AUDIT_PREF_KEY) || '{}');
    if (typeof o.branch === 'string') auditState.branch = o.branch;
    if (typeof o.base === 'string') auditState.base = o.base;
    if (typeof o.depth === 'string') auditState.depth = o.depth;
    if (typeof o.useConfig === 'boolean') auditState.useConfig = o.useConfig;
    if (Array.isArray(o.skip)) auditState.skip = o.skip.filter((x) => typeof x === 'string');
  } catch (_) { /* 坏掉的配置当没有 */ }
}

function saveAuditPrefs() {
  try {
    localStorage.setItem(
      AUDIT_PREF_KEY,
      JSON.stringify({
        branch: auditState.branch,
        base: auditState.base,
        depth: auditState.depth,
        useConfig: auditState.useConfig,
        skip: auditState.skip,
      })
    );
  } catch (_) { /* 忽略 */ }
}

/** 两组跳过项是否一样（顺序、大小写不算差异）；不一样才在行上标出来 */
function skipDiff(a, b) {
  const norm = (list) => (Array.isArray(list) ? list.map((x) => String(x).toLowerCase()).sort() : []);
  const x = norm(a);
  const y = norm(b);
  return x.length !== y.length || x.some((v, i) => v !== y[i]);
}

function auditFormHtml() {
  const checked = (id) => (auditState.skip.includes(id) ? ' checked' : '');
  return `<div class="audit-form">
    <label class="audit-field">
      <span>允许分支</span>
      <input id="auditBranch" value="${esc(auditState.branch)}"
             placeholder="main, release/*（留空 = 不检查）" spellcheck="false" autocomplete="off" />
    </label>
    <label class="audit-field">
      <span>基准分支</span>
      <input id="auditBase" value="${esc(auditState.base)}"
             placeholder="留空 = 自动探测 main / master" spellcheck="false" autocomplete="off" />
    </label>
    <label class="audit-field">
      <span>扫描深度</span>
      <input id="auditDepth" type="number" min="1" max="16" value="${esc(auditState.depth)}"
             placeholder="留空 = 配置文件 / 4" spellcheck="false" autocomplete="off" />
    </label>
    <div class="audit-skips">
      ${AUDIT_SKIP_OPTIONS.map(
        (s) =>
          `<label class="check"><input type="checkbox" data-skip="${s.id}"${checked(s.id)} /><span>${s.label}</span></label>`
      ).join('')}
    </div>
    <label class="check audit-cfg">
      <input type="checkbox" id="auditUseConfig"${auditState.useConfig ? ' checked' : ''} />
      <span>使用目录里的 <code>.gittracer.toml</code></span>
    </label>
    <div class="audit-actions">
      <button class="btn primary small" id="auditRunBtn"${auditState.running ? ' disabled' : ''}>
        ${auditState.running ? '巡检中…' : auditState.report ? '重新巡检' : '开始巡检'}
      </button>
      <span class="dim">只读检查，不动任何仓库</span>
    </div>
    <div id="auditResult" class="audit-result">${auditResultHtml()}</div>
  </div>`;
}

function auditResultHtml() {
  if (auditState.error) {
    return `<div class="audit-empty err">${esc(auditState.error)}</div>`;
  }
  const r = auditState.report;
  if (!r) {
    return `<div class="audit-empty dim">
      还没跑。默认检查：未提交改动、冲突、未推送、detached HEAD、未完成的合并 / 变基、
      落后远程、无上游、stash、分支是否符合规则、遗留的本地分支。<br />
      规则可以写进目录里的 <code>.gittracer.toml</code> 跟着仓库走，命令行
      <code>gittracer-audit</code> 读的是同一份，按返回码判定。
    </div>`;
  }

  const t = r.totals || {};
  const ok = !t.errors;
  const cfgCount = (r.config_files || []).length;
  const head = `<div class="audit-sum">
    <span class="audit-verdict ${ok ? 'ok' : 'bad'}">${ok ? '通过' : '未通过'}</span>
    <span class="audit-nums">${t.repos || 0} 个仓库 · ${t.passed || 0} 通过 / ${t.failed || 0} 未通过
      · ${t.errors || 0} error / ${t.warnings || 0} warn · ${((r.elapsed_ms || 0) / 1000).toFixed(1)}s</span>
    ${
      cfgCount
        ? `<span class="audit-cfg-note" title="${esc(
            (r.config_files || []).join('\n')
          )}">依据 ${cfgCount} 个 .gittracer.toml</span>`
        : ''
    }
  </div>`;

  if (!r.repos || !r.repos.length) {
    return `${head}<div class="audit-empty">这个目录下没找到 Git 仓库。</div>`;
  }

  const rows = r.repos
    .map((repo) => {
      const failed = (repo.findings || []).some((f) => f.severity === 'error');
      const findings = (repo.findings || [])
        .map(
          (f) => `<li class="audit-finding">
            <span class="sev ${esc(f.severity)}">${
              f.severity === 'error' ? 'ERROR' : f.severity === 'warn' ? 'WARN' : 'INFO'
            }</span>
            <code>${esc(f.code)}</code>
            <span class="audit-msg">${esc(f.message)}</span>
            ${f.detail ? `<div class="audit-detail">${esc(f.detail)}</div>` : ''}
          </li>`
        )
        .join('');
      return `<li class="audit-row ${failed ? 'fail' : 'pass'}">
        <div class="audit-head">
          <span class="audit-dot"></span>
          <span class="audit-name">${esc(repo.name)}</span>
          <span class="branch">${esc(repo.branch || 'detached')}</span>
          ${
            repo.config
              ? `<span class="audit-tag" title="${esc(repo.config)}">本地配置</span>`
              : ''
          }
          ${
            skipDiff(repo.effective_skip, r.skipped)
              ? `<span class="audit-tag skip" title="本仓库跳过：${esc(
                  repo.effective_skip.join(', ')
                )}">跳过 ${(repo.effective_skip || []).length}</span>`
              : ''
          }
          <span class="grow"></span>
          <button class="icon-mini" data-reveal="${esc(repo.path)}" title="在资源管理器中打开">&#8599;</button>
        </div>
        <div class="audit-path" title="${esc(nicePath(repo.path))}">${esc(nicePath(repo.path))}</div>
        ${findings ? `<ul class="audit-findings">${findings}</ul>` : ''}
      </li>`;
    })
    .join('');

  return `${head}
    <ul class="audit-list">${rows}</ul>
    <div class="audit-foot">
      <button class="btn small" id="auditCopyMd">复制报告（Markdown）</button>
      <button class="btn small" id="auditCopyCmd">复制命令行</button>
      <span class="dim">命令行返回码：0 通过 / 1 未通过</span>
    </div>`;
}

function openAuditPanel() {
  if (!state.roots.length) {
    toast('先添加要扫描的目录', true);
    els.rootInput.focus();
    return;
  }
  auditState.report = null;
  auditState.markdown = '';
  auditState.command = '';
  auditState.error = '';
  askModal({
    title: '发版前巡检',
    html: auditFormHtml(),
    okText: '关闭',
    showCancel: false,
    focus: '#auditBranch',
    wide: true,
  });
}

async function runAudit() {
  if (auditState.running) return;
  const branchEl = document.getElementById('auditBranch');
  const baseEl = document.getElementById('auditBase');
  auditState.branch = branchEl ? branchEl.value.trim() : '';
  auditState.base = baseEl ? baseEl.value.trim() : '';
  const depthEl = document.getElementById('auditDepth');
  const cfgEl = document.getElementById('auditUseConfig');
  auditState.depth = depthEl ? depthEl.value.trim() : '';
  auditState.useConfig = cfgEl ? cfgEl.checked : true;
  auditState.skip = [...els.modalBody.querySelectorAll('[data-skip]:checked')].map((x) => x.dataset.skip);
  saveAuditPrefs();

  auditState.running = true;
  auditState.error = '';
  auditState.report = null;

  const btn = document.getElementById('auditRunBtn');
  const box = document.getElementById('auditResult');
  if (btn) {
    btn.disabled = true;
    btn.textContent = '巡检中…';
  }
  if (box) {
    box.innerHTML =
      `<div class="audit-empty"><span class="spinner" style="display:inline-block;vertical-align:-2px"></span>` +
      ` 正在跑 ${state.roots.length} 个根目录…</div>`;
  }
  els.modalOk.disabled = true;

  try {
    // 空 = 交给 .gittracer.toml，再没有就按 4 层；不在这里替用户填死
    const depthNum = parseInt(auditState.depth, 10);
    const res = await invoke('audit_repos', {
      roots: state.roots,
      depth: Number.isFinite(depthNum) ? depthNum : null,
      branch: auditState.branch ? auditState.branch.split(',').map((s) => s.trim()).filter(Boolean) : [],
      base: auditState.base || null,
      skip: auditState.skip,
      useConfig: auditState.useConfig,
    });
    auditState.report = res.report || null;
    auditState.markdown = res.markdown || '';
    auditState.command = res.command || '';
  } catch (e) {
    auditState.error = errText(e);
  } finally {
    auditState.running = false;
    els.modalOk.disabled = false;
    if (btn) {
      btn.disabled = false;
      btn.textContent = '重新巡检';
    }
    const out = document.getElementById('auditResult');
    if (out) out.innerHTML = auditResultHtml();
  }
}

/* ---------- 扫描 ---------- */
let rafPending = false;
function scheduleRender() {
  if (rafPending) return;
  rafPending = true;
  requestAnimationFrame(() => {
    rafPending = false;
    renderStats();
    renderRepoList();
    renderRepoHead();
    renderFileList();
    renderCommitBar();
  });
}

async function loadOne(repo, seq) {
  state.pending.add(repo.path);
  try {
    const st = await invoke('repo_status', { path: repo.path });
    if (seq !== state.seq) return;
    state.status.set(repo.path, st);
  } catch (e) {
    if (seq !== state.seq) return;
    state.status.set(repo.path, { error: errText(e) });
  } finally {
    state.pending.delete(repo.path);
  }
  progress();
  scheduleRender();
}

/// 读取 status 是个长尾过程（几个巨型仓库能把尾巴拖到几十秒），
/// 这里在底部状态栏显示 已完成/总数，免得用户以为界面卡死。
function progress() {
  if (!state.repos.length) return;
  const done = state.repos.filter((r) => state.status.has(r.path)).length;
  if (done >= state.repos.length) return;
  els.statusLeft.innerHTML =
    `<span class="spinner" style="display:inline-block;vertical-align:-2px"></span> 正在读取 git status… ` +
    `<b>${done}</b>/${state.repos.length}`;
}

async function loadAll(list, seq) {
  const queue = list.slice();
  const workers = [];
  const n = Math.min(6, Math.max(1, queue.length));
  const run = async () => {
    for (;;) {
      if (seq !== state.seq) return;
      const repo = queue.shift();
      if (!repo) return;
      await loadOne(repo, seq);
    }
  };
  for (let i = 0; i < n; i++) workers.push(run());
  await Promise.all(workers);
}

async function scan() {
  if (!state.roots.length) {
    toast('先添加要扫描的目录', true);
    els.rootInput.focus();
    return;
  }
  const seq = ++state.seq;
  state.depth = parseInt(els.depthSel.value, 10) || 4;
  state.dirtyOnly = els.dirtyOnly.checked;
  savePrefs();

  state.scan = null;
  state.repos = [];
  state.status.clear();
  state.pending.clear();
  state.selectedRepo = null;
  state.checked.clear();
  state.file = null;
  state.diff = null;
  state.graph = null;
  state.commit = null;
  state.commitDiff = null;

  els.scanBtn.disabled = true;
  els.refreshBtn.disabled = true;
  els.statusLeft.innerHTML = '<span class="spinner" style="display:inline-block;vertical-align:-2px"></span> 正在遍历目录…';
  renderRoots();
  renderRepoList();
  renderFileList();
  renderCommitBar();
  renderDiffBody();
  renderStats();

  const t0 = performance.now();
  // 每个根目录各扫一遍，互不影响：一个目录出错不该拖垮其他目录
  const results = await Promise.all(
    state.roots.map(async (root) => {
      try {
        const res = await invoke('scan_repos', { root, depth: state.depth });
        return { root, res };
      } catch (e) {
        return { root, error: errText(e) };
      }
    })
  );
  if (seq !== state.seq) return;

  const repos = [];
  const seen = new Set();
  const errors = [];
  let dirs = 0;
  let truncated = false;
  let failedRoots = 0;
  for (const { root, res, error } of results) {
    if (error) {
      failedRoots += 1;
      errors.push(`${baseName(root)}：${error}`);
      continue;
    }
    dirs += res.dirs || 0;
    truncated = truncated || !!res.truncated;
    for (const e of res.errors || []) errors.push(e);
    for (const r of res.repos || []) {
      // 嵌套的根目录会把同一个仓库扫两遍，按规范路径去重（Windows 不分大小写）
      const key = pathKey(r.path);
      if (seen.has(key)) continue;
      seen.add(key);
      repos.push({ ...r, root });
    }
  }

  if (failedRoots === results.length) {
    els.scanBtn.disabled = false;
    els.refreshBtn.disabled = false;
    els.statusLeft.innerHTML = '<span class="err">扫描失败</span>';
    renderRepoList();
    els.diffBody.innerHTML = `<div class="notice">扫描失败：${esc(errors[0] || '未知错误')}</div>`;
    toast(errors[0] || '扫描失败', true);
    return;
  }

  state.scan = {
    repos,
    dirs,
    errors,
    truncated,
    elapsed_ms: Math.round(performance.now() - t0),
  };
  state.repos = repos;
  if (state.roots.length === 1) pushRecent(state.roots[0]);
  renderStats();
  renderRepoList();
  els.statusLeft.textContent = `找到 ${repos.length} 个仓库，正在读取 git status…`;

  if (repos.length) await loadAll(repos.slice(), seq);
  if (seq !== state.seq) return;

  els.scanBtn.disabled = false;
  els.refreshBtn.disabled = false;

  const dirty = repos.filter((r) => {
    const st = state.status.get(r.path);
    return st && !st.error && distinctCount(st) > 0;
  });
  let failed = 0;
  for (const r of repos) {
    const st = state.status.get(r.path);
    if (st && st.error) failed += 1;
  }
  els.statusLeft.innerHTML =
    `扫描完成：<b>${repos.length}</b> 个仓库，<b>${dirty.length}</b> 个有变更` +
    ` · ${fmtMs(Math.round(performance.now() - t0))}` +
    (failed ? ` · <span class="err">${failed} 个读取失败</span>` : '') +
    (errors.length ? ` · <span class="err">${errors.length} 个目录出错</span>` : '');

  renderStats();
  renderRepoList();

  // 自动选中第一个有变更的仓库，省掉一次点击
  if (!state.selectedRepo && repos.length) {
    const target = dirty[0] || repos[0];
    selectRepo(target.path);
  }
}

/* ---------- 事件 ---------- */
function moveRepoSelection(delta) {
  const list = visibleRepos();
  if (!list.length) return;
  const idx = list.findIndex((x) => x.ref.path === state.selectedRepo);
  const next = Math.max(0, Math.min(list.length - 1, (idx < 0 ? -1 : idx) + delta));
  const path = list[next].ref.path;
  if (path !== state.selectedRepo) selectRepo(path);
  const el = els.repoList.querySelector(`.repo-row[data-path="${CSS.escape(path)}"]`);
  if (el) el.scrollIntoView({ block: 'nearest' });
}

function bind() {
  els.repoList.tabIndex = 0;
  els.fileList.tabIndex = 0;

  els.repoList.addEventListener('click', (e) => {
    const row = e.target.closest('.repo-row');
    if (row) selectRepo(row.dataset.path);
  });

  els.fileList.addEventListener('click', (e) => {
    const row = e.target.closest('.file-row');
    if (!row) return;
    if (e.target.closest('.pick')) {
      toggleChecked(row.dataset.path);
      return;
    }
    selectFile(row.dataset.key);
  });

  els.repoList.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      moveRepoSelection(e.key === 'ArrowDown' ? 1 : -1);
    }
  });

  els.fileList.addEventListener('keydown', (e) => {
    const rows = Array.from(els.fileList.querySelectorAll('.file-row'));
    if (e.key === ' ') {
      // 空格勾选当前行，方便连着点几个文件
      e.preventDefault();
      const cur = rows.find((r) => r.classList.contains('active'));
      if (cur) toggleChecked(cur.dataset.path);
      return;
    }
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
    e.preventDefault();
    if (!rows.length) return;
    const cur = rows.findIndex((r) => r.classList.contains('active'));
    const next = Math.max(0, Math.min(rows.length - 1, cur + (e.key === 'ArrowDown' ? 1 : -1)));
    selectFile(rows[next].dataset.key);
  });

  els.selectAllBtn.onclick = () => checkAll();
  els.selectNoneBtn.onclick = () => checkNone();
  els.pickAll.onchange = () => (els.pickAll.checked ? checkAll() : checkNone());
  els.listSelectNoneBtn.onclick = () => checkNone();
  els.commitExpandBtn.onclick = () => editMessage();
  els.pushEntry.onclick = () => openPushBoard();

  els.viewTabs.querySelectorAll('.tab').forEach((tab) => {
    tab.onclick = () => switchView(tab.dataset.view);
  });

  els.diffBody.addEventListener('click', (e) => {
    const row = e.target.closest('.g-row');
    if (row) openCommit(row.dataset.hash, row.dataset.short, row.dataset.subject);
  });

  els.modalBody.addEventListener('click', (e) => {
    const btn = e.target.closest('button');
    if (!btn) return;
    if (btn.id === 'pushAllBtn') {
      pushRows([...els.modalBody.querySelectorAll('.push-row')]);
      return;
    }
    if (btn.id === 'auditRunBtn') {
      runAudit();
      return;
    }
    if (btn.id === 'auditCopyMd') {
      copyText(auditState.markdown, '巡检报告已复制（Markdown）');
      return;
    }
    if (btn.id === 'auditCopyCmd') {
      copyText(auditState.command, '命令行已复制');
      return;
    }
    if (btn.hasAttribute('data-reveal')) {
      invoke('reveal_path', { path: btn.dataset.reveal }).catch((e) => toast(errText(e), true));
      return;
    }
    if (btn.hasAttribute('data-push')) {
      const row = btn.closest('.push-row');
      if (row) pushRows([row]);
    }
  });

  els.modalBody.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey) && e.target.id === 'msgEditor') {
      e.preventDefault();
      editCommitAfter = true;
      closeModal(true);
      return;
    }
    // 巡检的两个输入框里回车就等于「开始巡检」
    if (e.key === 'Enter' && (e.target.id === 'auditBranch' || e.target.id === 'auditBase')) {
      e.preventDefault();
      runAudit();
    }
  });

  els.commitBtn.onclick = () => doCommit();
  els.pullBtn.onclick = () => doPull();
  els.pushBtn.onclick = () => doPush();

  els.commitMsg.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      doCommit();
    }
  });

  els.modalOk.onclick = () => closeModal(true);
  els.modalCancel.onclick = () => closeModal(false);
  els.modalWrap.addEventListener('mousedown', (e) => {
    if (e.target === els.modalWrap && !boardBusy) closeModal(false);
  });

  els.scanBtn.onclick = () => scan();
  els.refreshBtn.onclick = () => scan();
  els.auditBtn.onclick = () => openAuditPanel();

  els.browseBtn.onclick = async () => {
    try {
      const picked = await invoke('pick_folder', { initial: trimRoot(els.rootInput.value) });
      if (picked) addRoot(picked);
    } catch (e) {
      toast(errText(e), true);
    }
  };

  els.rootInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') {
      e.preventDefault();
      const raw = els.rootInput.value.trim();
      if (!raw) {
        scan();
        return;
      }
      if (addRoot(raw)) els.rootInput.value = '';
    }
  });

  if (els.rootChips) {
    els.rootChips.addEventListener('click', (e) => {
      const x = e.target.closest('[data-remove]');
      if (!x) return;
      e.stopPropagation();
      removeRoot(x.dataset.remove);
    });
  }

  els.repoFilter.addEventListener('input', () => {
    state.repoFilter = els.repoFilter.value;
    renderRepoList();
  });
  els.fileFilter.addEventListener('input', () => {
    state.fileFilter = els.fileFilter.value;
    renderFileList();
  });

  els.depthSel.addEventListener('change', () => {
    state.depth = parseInt(els.depthSel.value, 10) || 4;
    savePrefs();
  });
  els.dirtyOnly.addEventListener('change', () => {
    state.dirtyOnly = els.dirtyOnly.checked;
    savePrefs();
    renderRepoList();
  });

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && !els.modalWrap.hidden) {
      e.preventDefault();
      if (boardBusy) return; // 正在推送，别让 Esc 把进度面板关掉
      closeModal(false);
      return;
    }
    if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === 'e') {
      e.preventDefault();
      if (!els.modalWrap.hidden) return;
      editMessage();
      return;
    }
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === 'f' && state.view === 'graph') {
      e.preventDefault();
      focusGraphSearch();
      return;
    }
    if (e.key === 'F5' || (e.ctrlKey && e.key === 'r')) {
      e.preventDefault();
      scan();
      return;
    }
    if (e.key === '/' && !isInput(e.target)) {
      e.preventDefault();
      els.repoFilter.focus();
      els.repoFilter.select();
    }
  });
}

/* ---------- 启动 ---------- */
async function init() {
  readPrefs();
  loadAuditPrefs();
  els.rootInput.value = '';
  if (![...els.depthSel.options].some((o) => Number(o.value) === state.depth)) {
    state.depth = 4;
  }
  els.depthSel.value = String(state.depth);
  els.dirtyOnly.checked = state.dirtyOnly;

  bind();
  renderRoots();
  renderViewTabs();
  renderStats();
  renderRepoList();
  renderFileList();
  renderCommitBar();
  renderDiffBody();
  renderDiffHead();

  if (!hasTauri()) {
    els.statusLeft.innerHTML = '<span class="err">未在桌面应用中运行</span>';
    els.diffBody.innerHTML = `<div class="notice">请通过 Git Tracer 桌面程序打开。</div>`;
    return;
  }

  try {
    state.git = await invoke('git_info');
  } catch (e) {
    state.git = { available: false, hint: errText(e) };
  }
  if (state.git && state.git.available) {
    // "git version 2.49.0.windows.1" → "git 2.49.0"
    const m = /(\d+\.\d+\.\d+)/.exec(state.git.version || '');
    els.statGit.textContent = m ? `git ${m[1]}` : 'git';
    els.statGit.title = state.git.version || 'git';
  } else {
    els.statGit.textContent = 'git 不可用';
    els.statGit.style.color = '#ff9a94';
    const hint = (state.git && state.git.hint) || '未找到 git';
    els.diffBody.innerHTML = `<div class="notice">${esc(hint)}</div>`;
    els.scanBtn.disabled = true;
    els.refreshBtn.disabled = true;
    return;
  }

  els.statusLeft.textContent = '就绪';
  if (state.roots.length) scan();
}

window.addEventListener('DOMContentLoaded', init);
