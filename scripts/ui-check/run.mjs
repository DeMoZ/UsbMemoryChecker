// Layout check: renders every display state of the window in every language and theme at the given sizes and
// reports page scrolling, panes that scroll or clip, and controls that are cut off. It never starts a task and
// never sends anything to Rust (the harness replaces `send`). Engines: WKWebView (macOS, render.swift) and headless
// Chrome/Edge (the WebView2 engine).
//   node scripts/ui-check/run.mjs [--sizes 1180x760,1600x1000,880x600] [--langs en,ru,es,de,zh] [--states a,b]
//        [--themes dark,light] [--engine webkit|chromium|both] [--out <dir for PNGs>] [--page <index.html>]
//        [--verbose] [--json <report file>]
// Exit code 1 when any case fails.
import { readFileSync, writeFileSync, mkdtempSync, mkdirSync, existsSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '../..');
const argv = process.argv.slice(2);
const opt = (name, def) => { const i = argv.indexOf('--' + name); return i < 0 ? def : (argv[i + 1] && !argv[i + 1].startsWith('--') ? argv[i + 1] : true); };

const sizes = String(opt('sizes', '1180x760,1600x1000,880x600')).split(',').map(s => s.split('x').map(Number));
const langs = String(opt('langs', 'en,ru,es,de,zh')).split(',');
const themes = String(opt('themes', 'dark,light')).split(',');
const engineOpt = String(opt('engine', process.platform === 'darwin' ? 'both' : 'chromium'));
const engines = engineOpt === 'both' ? ['webkit', 'chromium'] : [engineOpt];
const outDir = opt('out', null);
const pagePath = resolve(String(opt('page', join(root, 'src/ui/index.html'))));
const verbose = !!opt('verbose', false);
const jsonOut = opt('json', null);

const tmp = mkdtempSync(join(tmpdir(), 'umc-ui-check-'));
process.on('exit', () => { try { rmSync(tmp, { recursive: true, force: true }); } catch {} });

// the page as main.rs::page() builds it, plus the harness and the measurement
const i18n = readFileSync(join(root, 'src/ui/i18n.js'), 'utf8');
const harness = readFileSync(join(here, 'harness.js'), 'utf8');
const measure = readFileSync(join(here, 'measure.js'), 'utf8');
const composed = readFileSync(pagePath, 'utf8').replace('/*I18N*/', () => i18n)
  .replace('</body>', () => `<script>\n${harness}\n${measure}\n</script>\n</body>`);
const pageFile = join(tmp, 'page.html');
writeFileSync(pageFile, composed);

// every state the harness knows, unless --states narrows it down
const scenarios = (() => {
  const m = readFileSync(join(here, 'harness.js'), 'utf8');
  return [...m.matchAll(/^\s*def\('(\w+)'/gm)].map(x => x[1]);
})();
const states = opt('states', null) ? String(opt('states')).split(',') : scenarios;
for (const s of states) if (!scenarios.includes(s)) { console.error('unknown state ' + s + '. Known: ' + scenarios.join(', ')); process.exit(2); }

// panes that may scroll inside: the drive list, the results table, the release notes; at small sizes the work card
// and a dialog that is taller than the window
const allowFor = (w, h) => ['.drives', '.hist', '.notes'].concat(w < 1100 || h < 700 ? ['.panel', '.modal'] : []);

function jobsFor(engine) {
  const jobs = [];
  for (const theme of themes) for (const [w, h] of sizes) {
    for (const lang of langs) {
    let first = true;
    for (const state of states) {
      const spec = { name: state, lang, allow: allowFor(w, h) };
      const tag = `${state}-${lang}-${theme}-${w}x${h}`;
      jobs.push({ engine, theme, w, h, state, lang, fresh: first, spec, out: outDir ? join(String(outDir), `${engine}-${tag}.png`) : undefined });
      first = !!opt('fresh-each', false);
    }
    }
  }
  return jobs;
}

function runWebkit(jobs) {
  const bin = join(tmp, 'render');
  const c = spawnSync('swiftc', ['-O', join(here, 'render.swift'), '-o', bin], { encoding: 'utf8' });
  if (c.status !== 0) { console.error(c.stderr); process.exit(2); }
  const cfg = { html: pageFile, scale: 2, jobs: jobs.map(j => ({ js: `JSON.stringify(__umcRun(${JSON.stringify(j.spec)}))`, theme: j.theme, width: j.w, height: j.h, fresh: j.fresh, out: j.out })) };
  writeFileSync(join(tmp, 'jobs.json'), JSON.stringify(cfg));
  const r = spawnSync(bin, [join(tmp, 'jobs.json')], { encoding: 'utf8', maxBuffer: 1 << 28, timeout: 1800000 });
  if (r.error) { console.error('renderer failed: ' + r.error.message); process.exit(2); }
  const results = new Array(jobs.length).fill(null);
  for (const line of r.stdout.split('\n')) {
    if (!line.startsWith('{')) continue;
    const o = JSON.parse(line);
    results[o.i] = o.error ? { error: o.error } : o.r;
  }
  return results;
}

function chromeBinary() {
  for (const p of ['/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
    '/usr/bin/google-chrome', '/usr/bin/chromium', '/usr/bin/microsoft-edge',
    'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe', 'C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe']) if (existsSync(p)) return p;
  return null;
}

function runChromium(jobs) {
  const bin = chromeBinary();
  if (!bin) { console.error('no Chrome or Edge found'); process.exit(2); }
  const results = new Array(jobs.length).fill(null);
  for (const theme of themes) {
    const idx = jobs.map((j, i) => [j, i]).filter(([j]) => j.theme === theme);
    const list = idx.map(([j]) => ({ w: j.w, h: j.h, fresh: j.fresh, spec: j.spec }));
    // the page runs in an iframe of the exact size, so the viewport is the one asked for
    const driver = `<!doctype html><meta charset=utf-8><body style="margin:0"><iframe id=f style="border:0;position:absolute;left:0;top:0"></iframe><pre id=out></pre><script>
const jobs=${JSON.stringify(list)}; const out=[];
const f=document.getElementById('f');
const wait=ms=>new Promise(r=>setTimeout(r,ms));
(async()=>{
  for(const j of jobs){
    f.style.width=j.w+'px'; f.style.height=j.h+'px';
    if(j.fresh){ await new Promise(r=>{f.onload=r; f.src='page.html?'+Math.random();}); await wait(50); }
    let r; try{ r=f.contentWindow.__umcRun(j.spec); r.dark=f.contentWindow.matchMedia('(prefers-color-scheme: dark)').matches; }catch(e){ r={error:String(e)}; }
    out.push(r); await wait(0);
  }
  document.getElementById('out').textContent=JSON.stringify(out); document.title='done';
})();
</script>`;
    writeFileSync(join(tmp, 'driver.html'), driver);
    const args = ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--allow-file-access-from-files', '--hide-scrollbars=false',
      '--user-data-dir=' + mkdtempSync(join(tmp, 'chrome-')), '--virtual-time-budget=' + (60000 + list.length * 400),
      ...(theme === 'dark' ? ['--force-dark-mode'] : []), ...(opt('reduced-motion', false) ? ['--force-prefers-reduced-motion'] : []), '--dump-dom', 'file://' + join(tmp, 'driver.html')];
    const r = spawnSync(bin, args, { encoding: 'utf8', maxBuffer: 1 << 28, timeout: 900000 });
    const m = /<pre id="out">([\s\S]*?)<\/pre>/.exec(r.stdout || '');
    if (!m) { console.error('Chromium gave no report (' + theme + '):\n' + (r.stderr || '').slice(0, 800)); process.exit(2); }
    const data = JSON.parse(m[1].replace(/&quot;/g, '"').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&'));
    idx.forEach(([, i], k) => {
      results[i] = data[k];
      if (data[k] && data[k].dark !== undefined && data[k].dark !== (theme === 'dark')) results[i] = { error: 'Chromium did not apply the ' + theme + ' colour scheme' };
    });
    if (outDir) console.error('note: PNG snapshots come from the WebKit engine only');
  }
  return results;
}

function problems(r) {
  if (!r) return ['no result'];
  if (r.error) return ['error: ' + String(r.error).split('\n')[0]];
  const p = [];
  if (r.page.overflow) p.push(`page scrolls (${r.page.scrollH}>${r.page.clientH})`);
  for (const s of r.scrollers.filter(s => !s.allowed)) p.push(`scrolls inside ${s.el} (${s.scrollH}>${s.clientH})`);
  for (const c of r.clipped) p.push(`clipped ${c.el} (${c.scrollH}>${c.clientH})`);
  for (const c of r.hclip || []) p.push(`sideways overflow ${c.el} "${c.text}" (${c.scrollW}>${c.clientW})`);
  for (const c of r.notip || []) p.push(`ellipsis without tooltip ${c.el} "${c.text}"`);
  for (const b of r.below) p.push(`out of reach: ${b.el} "${b.text}" by ${b.by}`);
  for (const s of r.side) p.push(`sideways: ${s.el} by ${s.by}`);
  if (r.sent) p.push(`${r.sent} message(s) sent to Rust`);
  return p;
}

const report = [];
let failed = 0, total = 0;
const stat = {};
for (const engine of engines) {
  const jobs = jobsFor(engine);
  const res = engine === 'webkit' ? runWebkit(jobs) : runChromium(jobs);
  jobs.forEach((j, i) => {
    const p = problems(res[i]);
    total++;
    const key = `${engine} ${j.w}x${j.h}`;
    const s = stat[key] || (stat[key] = { cases: 0, fail: 0, pageScroll: 0 });
    s.cases++;
    if (res[i] && res[i].page && res[i].page.overflow) s.pageScroll++;
    if (p.length) { s.fail++; failed++; report.push({ engine, state: j.state, lang: j.lang, theme: j.theme, size: `${j.w}x${j.h}`, problems: p }); }
  });
}
console.log('engine size        cases  failing  page-scroll');
for (const [k, s] of Object.entries(stat)) console.log(k.padEnd(19), String(s.cases).padStart(5), String(s.fail).padStart(8), String(s.pageScroll).padStart(12));
const shown = verbose ? report : report.slice(0, 25);
for (const f of shown) console.log(`FAIL ${f.engine} ${f.size} ${f.state}/${f.lang}/${f.theme}: ${f.problems.slice(0, 3).join('; ')}`);
if (report.length > shown.length) console.log(`... ${report.length - shown.length} more (use --verbose or --json)`);
if (jsonOut) writeFileSync(String(jsonOut), JSON.stringify(report, null, 1));
console.log(failed ? `${failed} of ${total} cases fail` : `all ${total} cases pass`);
process.exit(failed ? 1 : 0);
