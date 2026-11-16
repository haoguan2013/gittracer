// 极简 CDP 驱动：用 Node 内置 WebSocket 连上 WebView2 的远程调试端口，
// 按 JSON 描述的步骤执行 eval / 截图 / 等待，并把页面里的 console 与异常打印出来。
//
// 用法：node tools/cdp.mjs <port> <steps.json>
//   steps.json: [{"eval": "expr"}, {"wait": 500}, {"shot": "out.png"}, {"title": "..."}]
'use strict';
import fs from 'node:fs';

const port = Number(process.argv[2] || 9223);
const stepsPath = process.argv[3];
const steps = JSON.parse(fs.readFileSync(stepsPath, 'utf8'));

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
const logs = [];

ws.addEventListener('message', (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg);
    pending.delete(msg.id);
    return;
  }
  if (msg.method === 'Runtime.consoleAPICalled') {
    const text = (msg.params.args || [])
      .map((a) => (a.value !== undefined ? String(a.value) : a.description || a.type))
      .join(' ');
    logs.push(`[console.${msg.params.type}] ${text}`);
  } else if (msg.method === 'Runtime.exceptionThrown') {
    const d = msg.params.exceptionDetails;
    logs.push(`[exception] ${d.text} ${d.exception?.description || d.exception?.value || ''}`);
  } else if (msg.method === 'Log.entryAdded') {
    const e = msg.params.entry;
    logs.push(`[log.${e.level}] ${e.text} ${e.url || ''}`);
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
  return r.result;
};

/// 轮询直到表达式为真（或超时），用来等异步渲染完成
async function waitFor(expression, timeoutMs = 20000) {
  const t0 = Date.now();
  for (;;) {
    const r = await evaluate(`(() => { try { return !!(${expression}); } catch (e) { return false; } })()`);
    if (r?.result?.value) return true;
    if (Date.now() - t0 > timeoutMs) {
      console.log(`[waitFor] 超时：${expression}`);
      return false;
    }
    await new Promise((res) => setTimeout(res, 120));
  }
}

await send('Runtime.enable');
await send('Log.enable');
await send('Page.enable');

const results = [];
for (const step of steps) {
  if (step.title) console.log(`\n### ${step.title}`);
  if (typeof step.wait === 'string') await waitFor(step.wait);
  else if (step.wait) await new Promise((r) => setTimeout(r, step.wait));
  if (step.waitFor) await waitFor(step.waitFor);
  if (step.eval) {
    const r = { result: await evaluate(step.eval) };
    const v = r.result?.result;
    const out = r.result?.exceptionDetails
      ? `EXCEPTION: ${r.result.exceptionDetails.text} ${r.result.exceptionDetails.exception?.description || ''}`
      : v?.value !== undefined
        ? typeof v.value === 'string'
          ? v.value
          : JSON.stringify(v.value)
        : v?.description ?? String(v?.type);
    results.push(out);
    console.log(out);
  }
  if (step.shot) {
    const params = { format: 'png', captureBeyondViewport: false };
    if (step.clip) params.clip = { ...step.clip, scale: step.clip.scale || 1 };
    const r = await send('Page.captureScreenshot', params);
    if (r.result?.data) {
      fs.writeFileSync(step.shot, Buffer.from(r.result.data, 'base64'));
      console.log(`[shot] ${step.shot}`);
    } else {
      console.log(`[shot] FAILED ${JSON.stringify(r.error || r)}`);
    }
  }
}

if (logs.length) {
  console.log('\n### page logs');
  for (const l of logs) console.log(l);
}

ws.close();
process.exit(0);
