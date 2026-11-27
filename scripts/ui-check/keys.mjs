// Keyboard and dialog behaviour check of the current page (WKWebView, harness recorder): dialogs keep focus,
// Escape closes dialogs and toasts, arrow keys work in tabs, drive list and radio groups, and skip disabled options.
//   node scripts/ui-check/keys.mjs
import { readFileSync, writeFileSync, mkdtempSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../..');
const tmp = mkdtempSync(join(tmpdir(), 'umc-keys-'));
process.on('exit', () => { try { rmSync(tmp, { recursive: true, force: true }); } catch {} });
const page = readFileSync(join(root, 'src/ui/index.html'), 'utf8').replace('/*I18N*/', () => readFileSync(join(root, 'src/ui/i18n.js'), 'utf8'))
  .replace('</body>', () => `<script>\n${readFileSync(join(here, 'harness.js'), 'utf8')}\n</script>\n</body>`);
writeFileSync(join(tmp, 'page.html'), page);
const H = `const q = s => document.querySelector(s), qa = s => [...document.querySelectorAll(s)];
const key = (el, k, o = {}) => { el.dispatchEvent(new KeyboardEvent('keydown', { key: k, bubbles: true, cancelable: true, ...o })); };
const ae = () => document.activeElement; const id = el => el && (el.id || el.dataset.tab || el.dataset.mode || el.dataset.id || el.dataset.fs || el.dataset.mb || el.textContent.trim().slice(0, 12));`;
const tests = {
  'repair dialog: Cancel focused, Escape closes, focus returns': ['repFake', `change = (el, v) => { el.checked = v; el.dispatchEvent(new Event('change')); }; change(q('#repConfirm'), true); q('#repBtn').focus(); q('#repBtn').click();
    const r = []; r.push(id(ae()) === 'Cancel'); r.push(q('#repairModal').classList.contains('open')); key(ae(), 'Escape'); r.push(!q('#repairModal').classList.contains('open')); r.push(ae() === q('#repBtn')); return r;`],
  'dialog Tab trap': ['modalQuit', `const f = qa('#quitModal button'); f[f.length - 1].focus(); key(ae(), 'Tab'); const a = ae() === f[0]; f[0].focus(); key(ae(), 'Tab', { shiftKey: true }); return [a, ae() === f[f.length - 1], q('#quitModal .modal').getAttribute('aria-modal') === 'true'];`],
  'close button has a name': ['modalDonate', `return [qa('.modal-close').every(b => b.getAttribute('aria-label') === 'Close')];`],
  'toast closes on Escape and click': ['toast', `const n = qa('.toast').length; key(document.body, 'Escape'); const a = qa('.toast').length === n - 1; toast('x', 'err'); qa('.toast').pop().click(); return [n === 1, a, qa('.toast').length === 0, q('#toasts').children.length === 0];`],
  'tabs arrows': ['idle', `q('#tab-speed').focus(); key(ae(), 'ArrowRight'); const a = id(ae()) === 'tab-capacity' && q('#panel-capacity').classList.contains('active'); key(ae(), 'End'); const b = id(ae()) === 'tab-repair'; key(ae(), 'Home'); return [a, b, id(ae()) === 'tab-speed'];`],
  'drive list arrows': ['idle', `qa('#driveList button')[0].focus(); key(ae(), 'ArrowDown'); const a = id(ae()) === 'disk7' && selected === 'disk7'; key(ae(), 'End'); const b = selected === 'disk8'; key(ae(), 'ArrowUp'); return [a, b, selected === 'disk7'];`],
  'drive change clears full-size tick': ['repFake', `q('#repWhole').checked = true; qa('#driveList button')[0].focus(); key(ae(), 'ArrowDown'); return [!q('#repWhole').checked];`],
  'size radios skip disabled': ['speedUnmounted', `return [qa('#sizeSeg button').every(b => b.disabled), qa('#sizeSeg button').filter(b => b.tabIndex === 0).length <= 1];`],
  'size radios arrows': ['idle', `const b = qa('#sizeSeg button'); b[1].focus(); key(ae(), 'ArrowRight'); return [speedMb === 5000, id(ae()) === '5000', key(ae(), 'ArrowRight'), speedMb === 100].filter(x => x !== undefined);`],
  'mode radios': ['capUnknown', `q('.mode[data-mode=quick]').focus(); key(ae(), 'ArrowRight'); return [capMode === 'full', id(ae()) === 'full'];`],
  'fs radios skip disabled FAT32': ['repFakeWhole', `const f = q('#fsSeg [data-fs=fat32]'); q('#fsSeg [data-fs=exfat]').focus(); key(ae(), 'ArrowRight'); return [f.disabled === true, fs === 'exfat'];`],
};
const names = Object.keys(tests);
const jobs = names.map(n => ({ js: `(function(){ ${H} try { __umcScenario(${JSON.stringify(tests[n][0])}, 'en'); const r = (function(){ ${tests[n][1]} })(); return JSON.stringify(r); } catch (e) { return JSON.stringify('ERR ' + e.message); } })()`, theme: 'dark', width: 1180, height: 760, fresh: true }));
writeFileSync(join(tmp, 'jobs.json'), JSON.stringify({ html: join(tmp, 'page.html'), scale: 1, jobs }));
const bin = join(tmp, 'render');
const c = spawnSync('swiftc', ['-O', join(here, 'render.swift'), '-o', bin], { encoding: 'utf8' });
if (c.status !== 0) { console.error(c.stderr); process.exit(2); }
const r = spawnSync(bin, [join(tmp, 'jobs.json')], { encoding: 'utf8' });
let bad = 0;
for (const l of r.stdout.split('\n')) if (l.startsWith('{')) {
  const o = JSON.parse(l); const v = o.error ? o.error : o.r;
  const ok = Array.isArray(v) && v.every(x => x === true);
  if (!ok) bad++;
  console.log((ok ? 'ok   ' : 'FAIL ') + names[o.i] + (ok ? '' : '  ' + JSON.stringify(v)));
}
console.log(bad ? `${bad} failed` : 'all keyboard checks pass');
process.exit(bad ? 1 : 0);
