// Contrast check of the colour tokens in src/ui/index.html (dark = :root, light = the prefers-color-scheme: light
// block). Pairs and minimums follow the window-appearance spec "Contrast". Prints every ratio, exits 1 on a miss.
//   node scripts/ui-check/contrast.mjs [path/to/index.html]
import { readFileSync } from 'node:fs';
import { dirname, resolve, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const html = readFileSync(process.argv[2] ? resolve(process.argv[2]) : join(root, 'src/ui/index.html'), 'utf8');
const style = html.slice(html.indexOf('<style>'), html.indexOf('</style>'));

function tokens(block) {
  const t = {};
  for (const m of block.matchAll(/--([\w-]+):\s*(#[0-9a-fA-F]{6})\b/g)) t[m[1]] = m[2];
  return t;
}
const darkBlock = /:root\s*\{([\s\S]*?)\n\}/.exec(style);
const lightBlock = /@media\s*\(prefers-color-scheme:\s*light\)\s*\{\s*:root\s*\{([\s\S]*?)\}\s*\}/.exec(style);
if (!darkBlock || !lightBlock) { console.error('token blocks not found'); process.exit(1); }
const dark = tokens(darkBlock[1]);
const themes = { dark, light: { ...dark, ...tokens(lightBlock[1]) } };

const lum = h => {
  const c = [1, 3, 5].map(i => parseInt(h.slice(i, i + 2), 16) / 255).map(v => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
};
const ratio = (a, b) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x + 0.05) / (y + 0.05); };

// [foreground, background, minimum]
const pairs = [];
for (const fg of ['text', 'text-2', 'text-3']) for (const bg of ['s1', 's2', 's3']) pairs.push([fg, bg, 4.5]);
for (const fg of ['signal', 'violet', 'ok', 'warn', 'bad']) for (const bg of ['s1', 's2']) pairs.push([fg, bg, fg === 'violet' ? 3 : 4.5]);
pairs.push(['signal-ink', 'signal', 4.5], ['bad-ink', 'bad', 4.5], ['edge', 's1', 3], ['edge', 's2', 3]);

let fails = 0;
for (const [name, t] of Object.entries(themes)) {
  console.log('-- ' + name);
  for (const [fg, bg, min] of pairs) {
    if (!t[fg] || !t[bg]) { console.log(`MISSING ${fg} on ${bg}`); fails++; continue; }
    const r = ratio(t[fg], t[bg]);
    const ok = r >= min;
    if (!ok) fails++;
    console.log(`${ok ? 'ok  ' : 'FAIL'} ${fg.padEnd(10)} on ${bg.padEnd(6)} ${r.toFixed(2).padStart(6)}  (min ${min})`);
  }
}
console.log(fails ? `${fails} pair(s) fail` : 'all pairs pass');
process.exit(fails ? 1 : 0);
