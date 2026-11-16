// 录一段 README 用的动图：按脚本走一遍「加目录 → 扫描 → 打开巡检 → 跑 → 看结果」，
// 每一步吐一帧 PNG，最后用 tools/gif.py 合成 GIF（那一步要 Pillow）。
//
// 用法：node tools/make-gif.mjs <port> <outdir>
// 先启动带调试端口的应用：
//   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9223" GitTracer.exe
//
// 为什么要脚本而不是录屏：录屏会带上桌面其他窗口、鼠标抖动和码率噪声；
// 这里每一帧都是完整的界面截图，文字清晰、体积小，重跑一次结果一致。
'use strict';
import fs from 'node:fs';
import path from 'node:path';

const port = Number(process.argv[2] || 9223);
const outdir = process.argv[3] || '.tmp/frames';

const list = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
const page = list.find((t) => t.type === 'page') || list[0];
if (!page) {
  console.error('no debuggable target');
  process.exit(1);
}
console.log(`[cdp] target: ${page.title} ${page.url}`);

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((res, rej) => {
  ws.addEventListener('open', res, { once: true });
  ws.addEventListener('error', rej, { once: true });
});

let id = 0;
const pending = new Map();
ws.addEventListener('message', (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
  }
});

const send = (method, params = {}) =>
  new Promise((resolve) => {
    const mid = ++id;
    pending.set(mid, resolve);
    ws.send(JSON.stringify({ id: mid, method, params }));
  });

const evaluate = async (expression) => {
  const r = await send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
    userGesture: true,
  });
  if (r.result?.exceptionDetails) {
    console.log(`EXCEPTION: ${r.result.exceptionDetails.text} ${r.result.exceptionDetails.exception?.description || ''}`);
  }
  return r.result?.result?.value;
};

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function waitFor(expression, timeoutMs = 30000) {
  const t0 = Date.now();
  for (;;) {
    if (await evaluate(`(() => { try { return !!(${expression}); } catch (e) { return false; } })()`)) return true;
    if (Date.now() - t0 > timeoutMs) {
      console.log(`[waitFor] 超时：${expression}`);
      return false;
    }
    await sleep(80);
  }
}

fs.rmSync(outdir, { recursive: true, force: true });
fs.mkdirSync(outdir, { recursive: true });

let n = 0;
const timing = [];
// 每帧的停留时间（毫秒）由调用方给：结论那两帧要让人看清楚，转圈的短一点才有动感。
async function frame(hold = 700) {
  const r = await send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false });
  if (!r.result?.data) {
    console.log(`[frame] FAILED ${JSON.stringify(r.error || r)}`);
    return;
  }
  const file = path.join(outdir, `${String(++n).padStart(3, '0')}.png`);
  fs.writeFileSync(file, Buffer.from(r.result.data, 'base64'));
  timing.push(hold);
  console.log(`[frame] ${file}  ${hold}ms`);
}

// 点一下某个控件：先挂一圈高亮，帧里能看出「刚才点了这里」，然后撤掉。
// 高亮样式只注入到页面里，不进 styles.css——它是演示用的，不该出现在产品里。
const HILITE = `
  if (!document.getElementById('demo-hl')) {
    const s = document.createElement('style');
    s.id = 'demo-hl';
    s.textContent = '.demo-click{outline:2px solid #2f81f7 !important;outline-offset:3px;' +
      'box-shadow:0 0 0 6px rgba(47,129,247,.16) !important;transition:none !important}';
    document.head.appendChild(s);
  }
`;

async function hilite(selector, on = true) {
  await evaluate(`${HILITE}(()=>{document.querySelectorAll('.demo-click').forEach(e=>e.classList.remove('demo-click'));` +
    `const el=document.querySelector(${JSON.stringify(selector)}); if(!el) return 'no el';` +
    `if(${on}) el.classList.add('demo-click'); return 'ok';})()`);
}

await send('Runtime.enable');
await send('Page.enable');

// ---------- 1. 从空状态开始 ----------
await evaluate(`(()=>{ if (typeof closeModal === 'function') closeModal(false); ` +
  `els.modalWrap.hidden = true; ` +
  `[...state.roots].forEach(r=>removeRoot(r)); auditState.report=null; ` +
  `return state.roots.length; })()`);
await sleep(400);
await frame(700);

// ---------- 2. 在输入框里打出目录 ----------
await evaluate(`(()=>{ const el=els.rootInput; el.value='D:\\\\Projects\\\\_git-tracer-demo'; el.focus(); return el.value; })()`);
await sleep(300);
await frame(600);

// ---------- 3. 回车 → 扫描中 ----------
await evaluate(`(()=>{ const el=els.rootInput; el.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true})); return 'enter'; })()`);
await sleep(120);
await frame(320);
await waitFor('state.scan && state.repos.length > 0', 20000);
await sleep(700);
await frame(1000);

// ---------- 4. 打开巡检面板 ----------
await hilite('#auditBtn');
await sleep(260);
await frame(520);
await evaluate(`openAuditPanel(); 'ok'`);
await sleep(420);
await hilite(null, false);
await frame(900);

// ---------- 5. 跑巡检 ----------
await evaluate(`(()=>{ const d=document.getElementById('auditDepth'); if(d){d.value='3';} return 'depth'; })()`);
await hilite('#auditRunBtn');
await sleep(260);
await frame(460);
await evaluate(`runAudit(); 'started'`);
await sleep(260);
await frame(460);
await waitFor("document.querySelector('.audit-verdict') !== null || document.querySelector('.audit-empty.err') !== null", 60000);
await hilite(null, false);
await sleep(500);
await frame(1800);

// ---------- 6. 往下滚一点，露出「工作区有活」那几条 ----------
await evaluate(`(()=>{ const b=document.querySelector('.modal-body'); if(b) b.scrollTop = 300; return b?b.scrollTop:'none'; })()`);
await sleep(420);
await frame(1500);
await evaluate(`(()=>{ const b=document.querySelector('.modal-body'); if(b) b.scrollTop = 0; return 'top'; })()`);
await sleep(300);

fs.writeFileSync(path.join(outdir, 'timing.json'), JSON.stringify(timing));
console.log(`[done] ${n} 帧 / ${(timing.reduce((a, b) => a + b, 0) / 1000).toFixed(1)}s → ${outdir}`);
ws.close();
process.exit(0);
