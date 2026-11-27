// IPC equivalence check: the same scripted clicks run on an old page and on the current one (WKWebView, harness
// recorder, nothing reaches Rust). The lists of messages `send` was called with must be identical.
//   node scripts/ui-check/ipc.mjs [old index.html (default: git show HEAD:src/ui/index.html)]
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../..');
const tmp = mkdtempSync(join(tmpdir(), 'umc-ipc-'));
process.on('exit', () => { try { rmSync(tmp, { recursive: true, force: true }); } catch {} });
const oldHtml = process.argv[2] ? readFileSync(resolve(process.argv[2]), 'utf8')
  : spawnSync('git', ['show', 'HEAD:src/ui/index.html'], { cwd: root, encoding: 'utf8', maxBuffer: 1 << 26 }).stdout;
const newHtml = readFileSync(join(root, 'src/ui/index.html'), 'utf8');
const i18n = readFileSync(join(root, 'src/ui/i18n.js'), 'utf8');
const extra = readFileSync(join(here, 'harness.js'), 'utf8') + '\n' + readFileSync(join(here, 'measure.js'), 'utf8');
const compose = h => h.replace('/*I18N*/', () => i18n).replace('</body>', () => `<script>\n${extra}\n</script>\n</body>`);

// each step list runs after the scenario is set up; helpers: q, qa, click, modalBtns
const H = `const q = s => document.querySelector(s), qa = s => [...document.querySelectorAll(s)];
const click = el => { if (!el) throw new Error('no element'); el.click(); };
const change = (el, v) => { if (v !== undefined) el.checked = v; el.dispatchEvent(new Event('change')); };
const mb = id => qa('#' + id + ' .modal-actions button');
const out = []; const R = window.__rust; const P = (n, f) => { try { out.push('ok:' + n + '=' + !!f()); } catch (e) { out.push('ok:' + n + '=false'); } };`;
const cases = {
  'sizes + start': ['idle', `click(qa('#sizeSeg button')[0]); click(qa('#sizeSeg button')[2]); click(q('#speedBtn'));`],
  'stop speed': ['speedRun', `click(q('#prog-speed button'));`],
  'quick + full check': ['capUnknown', `click(q('#capBtn')); click(q('.mode[data-mode=full]')); click(q('#capBtn')); click(q('.mode[data-mode=quick]'));`],
  'stop full': ['capRun', `click(q('#prog-capacity button'));`],
  'select drive': ['idle', `click(qa('#driveList button')[1]); click(qa('#driveList button')[0]);`],
  'repair default, cancel, confirm': ['repFake', `q('#repConfirm').checked = true; change(q('#repConfirm')); click(q('#repBtn')); click(mb('repairModal')[0]); click(q('#repBtn')); click(mb('repairModal')[1]);`],
  'repair full size': ['repFake', `change(q('#repWhole'), true); change(q('#repConfirm'), true); click(q('#repBtn')); click(mb('repairModal')[1]);`],
  'repair not confirmed': ['repFake', `click(q('#repBtn'));`],
  'repair plan none': ['repBroken', `change(q('#repConfirm'), true); click(q('#repBtn'));`],
  'repair limited whole': ['repLimited', `change(q('#repWhole'), true); change(q('#repConfirm'), true); click(q('#repBtn')); click(mb('repairModal')[1]);`],
  'fat32 and label': ['repFake', `click(q('#fsSeg [data-fs=exfat]')); q('#repLabel').value = 'abc d'; q('#repLabel').dispatchEvent(new Event('input')); change(q('#repConfirm'), true); click(q('#repBtn')); click(mb('repairModal')[1]);`],
  'verify repaired': ['repRepairedOffer', `click(q('#repVerifyBtn'));`],
  'verify waiting': ['repOfferWaiting', `click(q('#repVerifyBtn'));`],
  'reload twice': ['idle', `click(q('#rescanBtn')); click(q('#rescanBtn'));`],
  'language': ['idle', `const s = q('#langSelect'); s.value = 'ru'; s.dispatchEvent(new Event('change'));`],
  'update dialog': ['update', `click(q('#updBtn')); qa('#updActions button').forEach(b => b.click());`],
  'update dialog current': ['idle', `click(q('#updBtn')); qa('#updActions button').forEach(b => b.click()); change(q('#autoUpdate'), false);`],
  'donate': ['idle', `click(q('[onclick="openDonate()"]')); window.onRustMessage; qa('#donateMethods button').forEach(b => b.click());`],
  'quit dialog': ['modalQuit', `mb('quitModal')[0].click(); `],
  'quit confirm': ['modalQuit', `mb('quitModal')[1].click();`],
  // ---- multi-step sequences: messages from Rust (as the app sends them) between the clicks
  'speed, done, speed again, Stop': ['idle', `const sp = { kind: 'speed', disk: 'disk6', ok: true, result: { mb: 1000, speed: { write: { avg: 40, max: 50, min: 30, seconds: 25 }, read: { avg: 120, max: 140, min: 100, seconds: 8 }, badBlocks: 0 } } };
    click(q('#speedBtn')); R({ type: 'task', kind: 'speed', disk: 'disk6', fraction: 0.2, stage: 'prWrite', mbps: 30 }); out.push('ok:card1=' + !!q('#prog-speed .stage-t'));
    R({ type: 'taskDone', ...sp }); out.push('ok:card-gone=' + !q('#prog-speed button'), 'ok:btn=' + (q('#speedBtn').style.display !== 'none'));
    click(q('#speedBtn')); R({ type: 'task', kind: 'speed', disk: 'disk6', fraction: 0.1, stage: 'prWrite', mbps: 20 }); out.push('ok:card2=' + !!q('#prog-speed .stage-t')); click(q('#prog-speed button'));`],
  'quick, done, quick again, Stop': ['capUnknown', `click(q('#capBtn')); R({ type: 'task', kind: 'probe', disk: 'disk6', fraction: 0.3, stage: 'prCoarse' }); out.push('ok:card1=' + !!q('#prog-capacity .stage-t'));
    R({ type: 'taskDone', kind: 'probe', disk: 'disk6', ok: true, result: { check: { verdict: 'genuine' } } }); out.push('ok:gone=' + !q('#prog-capacity button'));
    click(q('#capBtn')); R({ type: 'task', kind: 'probe', disk: 'disk6', fraction: 0.1, stage: 'prCoarse' }); out.push('ok:card2=' + !!q('#prog-capacity .stage-t')); click(q('#prog-capacity button'));`],
  'full, done, full again (chart column)': ['capUnknown', `click(q('.mode[data-mode=full]')); click(q('#capBtn')); R({ type: 'task', kind: 'full', disk: 'disk6', fraction: 0.3, stage: 'prFill', mbps: 50 }); P('chart1', () => q('#chartCapBox').style.display === 'block');
    R({ type: 'taskDone', kind: 'full', disk: 'disk6', ok: true, result: { check: { verdict: 'genuine', scope: 'device' } } }); P('chart-gone', () => q('#chartCapBox').style.display === 'none');
    click(q('#capBtn')); R({ type: 'task', kind: 'full', disk: 'disk6', fraction: 0.1, stage: 'prFill' }); P('chart2', () => q('#chartCapBox').style.display === 'block'); P('card', () => q('#prog-capacity .stage-t'));`],
  'task, drive switch and back, Stop': ['speedRun', `click(qa('#driveList button')[1]); out.push('ok:away=' + !q('#prog-speed button')); click(qa('#driveList button')[0]); out.push('ok:back=' + !!q('#prog-speed .stage-t')); R({ type: 'task', kind: 'speed', disk: 'disk6', fraction: 0.5, stage: 'prRead' }); click(q('#prog-speed button'));`],
  'repair, done, offer, verify': ['repFake', `change(q('#repConfirm'), true); click(q('#repBtn')); click(mb('repairModal')[1]); R({ type: 'task', kind: 'fix', disk: 'disk6', fraction: 0.4, stage: 'prFixing' }); P('prog', () => q('#prog-repair').classList.contains('on')); P('box', () => q('#repConfirmBox').style.display === 'none');
    R({ type: 'taskDone', kind: 'fix', disk: 'disk6', ok: true, result: { size: 8303673344 } }); out.push('confirm=' + q('#repConfirm').checked + ' done=' + q('#repDone').classList.contains('on') + ' offer=' + q('#repVerify').classList.contains('on'));
    const d0 = disks[0]; R({ type: 'disks', disks: [{ ...d0, volumes: [{ mount: '/Volumes/USB', label: 'USB', fs: 'exFAT', total: 8e9, free: 7e9, writable: true }] }, ...disks.slice(1)], capacity, scanned: true, rescanned: 0 });
    const r = res('disk6'); r.repairedAt = Date.now() - 60000; r.unmountedSeen = true; renderAll(); out.push('verifyEnabled=' + !q('#repVerifyBtn').disabled); click(q('#repVerifyBtn'));`],
  'repair done twice, second progress card': ['repFake', `const go = () => { change(q('#repConfirm'), true); click(q('#repBtn')); click(mb('repairModal')[1]); R({ type: 'task', kind: 'fix', disk: 'disk6', fraction: 0.4, stage: 'prFixing' }); out.push('ok:prog=' + q('#prog-repair').classList.contains('on'), 'ok:stage=' + !!q('#prog-repair .stage-t')); R({ type: 'taskDone', kind: 'fix', disk: 'disk6', ok: true, result: { size: 8303673344 } }); }; go(); go();`],
  'tick reset: plan limit changes': ['repFake', `change(q('#repWhole'), true); const c = JSON.parse(JSON.stringify(capacity)); c.disk6.repair.limit = 7e9; R({ type: 'disks', disks, capacity: c, scanned: true, rescanned: 0 }); out.push('whole=' + q('#repWhole').checked);`],
  'tick reset: layout changes': ['repFake', `change(q('#repWhole'), true); const c = JSON.parse(JSON.stringify(capacity)); c.disk6.layoutKey = 'other'; R({ type: 'disks', disks, capacity: c, scanned: true, rescanned: 0 }); out.push('whole=' + q('#repWhole').checked);`],
  'tick reset: state changes': ['repFake', `change(q('#repWhole'), true); const c = JSON.parse(JSON.stringify(capacity)); c.disk6.state = 'damaged'; R({ type: 'disks', disks, capacity: c, scanned: true, rescanned: 0 }); out.push('whole=' + q('#repWhole').checked);`],
  'tick kept: same plan': ['repFake', `change(q('#repWhole'), true); R({ type: 'disks', disks, capacity: JSON.parse(JSON.stringify(capacity)), scanned: true, rescanned: 0 }); out.push('whole=' + q('#repWhole').checked);`],
  'tick cleared on drive change': ['repFake', `change(q('#repWhole'), true); click(qa('#driveList button')[1]); click(qa('#driveList button')[0]); out.push('whole=' + q('#repWhole').checked);`],
  'tick kept on same drive': ['repFake', `change(q('#repWhole'), true); click(qa('#driveList button')[0]); out.push('whole=' + q('#repWhole').checked);`],
  'disabled states': ['repRun', `out.push('btn=' + q('#repBtn').disabled, 'whole=' + q('#repWhole').disabled); click(q('#repBtn'));`],
  'other drive task': ['speedOther', `out.push('start=' + q('#speedBtn').disabled); click(q('#speedBtn'));`],
};
const names = Object.keys(cases);
const job = ([sc, steps]) => `(function(){ ${H} try { __umcScenario(${JSON.stringify(sc)}, 'en'); __sent.length = 0; ${steps} } catch (e) { out.push('ERR ' + e.message); } return JSON.stringify({ sent: __sent.map(m => JSON.stringify(m)), out }); })()`;
const run = (html, tag) => {
  writeFileSync(join(tmp, tag + '.html'), compose(html));
  const jobs = names.map((n, i) => ({ js: job(cases[n]), theme: 'dark', width: 1180, height: 760, fresh: true }));
  writeFileSync(join(tmp, tag + '.json'), JSON.stringify({ html: join(tmp, tag + '.html'), scale: 1, jobs }));
  const bin = join(tmp, 'render');
  const c = spawnSync('swiftc', ['-O', join(here, 'render.swift'), '-o', bin], { encoding: 'utf8' });
  if (c.status !== 0) { console.error(c.stderr); process.exit(2); }
  const r = spawnSync(bin, [join(tmp, tag + '.json')], { encoding: 'utf8', maxBuffer: 1 << 26, timeout: 600000 });
  const res = [];
  for (const l of r.stdout.split('\n')) if (l.startsWith('{')) { const o = JSON.parse(l); res[o.i] = o.error ? { err: o.error } : o.r; }
  return res;
};
const a = run(oldHtml, 'old'), b = run(newHtml, 'new');
// "ok:" probes look at the new page's own markup: they must be true there and are not compared with the old page
const split = r => r && r.out ? { sent: r.sent, out: r.out.filter(x => !String(x).startsWith('ok:')), ok: r.out.filter(x => String(x).startsWith('ok:')) } : r;
let bad = 0, total = 0;
names.forEach((n, i) => {
  const sa = split(a[i]), sb = split(b[i]);
  const same = JSON.stringify([sa.sent, sa.out]) === JSON.stringify([sb.sent, sb.out]) && (sb.ok || []).every(x => x.endsWith('=true'));
  const cnt = a[i] && a[i].sent ? a[i].sent.length : 0;
  total += cnt;
  if (!same || (b[i] && b[i].out && b[i].out.some(x => String(x).startsWith('ERR')))) { bad++; console.log('DIFF ' + n + '\n old: ' + JSON.stringify(sa) + '\n new: ' + JSON.stringify(sb)); }
  else console.log('same ' + String(cnt).padStart(2) + ' msg  ' + n + (sa.out.length ? '  ' + sa.out.join(' ') : '') + (sb.ok && sb.ok.length ? '  [' + sb.ok.length + ' ok probes]' : ''));
});
console.log(bad ? `${bad} of ${names.length} differ` : `identical: ${names.length} scripts, ${total} messages`);
process.exit(bad ? 1 : 0);
