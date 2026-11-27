// Layout measurement for the composed page. __umcMeasure(allowSelectors) returns a plain object:
//  page:      scroll overflow of the page itself (must never overflow, at any size)
//  scrollers: elements that scroll inside (overflow auto/scroll with hidden content), with `allowed` = matches the
//             allowed list (the drive list, the results table, the work card at small sizes, ...)
//  clipped:   elements whose content is cut off by overflow:hidden and that are not allowed
//  below:     visible controls, progress cards, verdicts, notes and offers that lie outside the window or outside
//             a non-scrolling parent, and are not inside an allowed scroller
//  side:      the same horizontally
(function () {
  const CONTROLS = 'button, input, select, textarea, [role=radio], [role=tab], .verdict, .progress, .note, .done, .confirm-box, .chart, .stats, .modal, .toast, label.tick';
  const name = el => (el.id ? '#' + el.id : el.tagName.toLowerCase()) + ([...el.classList].length ? '.' + [...el.classList].join('.') : '');
  const visible = el => {
    const cs = getComputedStyle(el);
    if (cs.display === 'none' || cs.visibility === 'hidden') return false;
    for (let p = el; p && p !== document.body; p = p.parentElement) if (getComputedStyle(p).display === 'none') return false;
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  };
  window.__umcMeasure = function (allow) {
    const allowSel = (allow && allow.length ? allow : ['.drives', '.hist']).join(',');
    const isAllowed = el => !!allowSel && el.matches(allowSel);
    const out = { vw: innerWidth, vh: innerHeight };
    const se = document.scrollingElement;
    out.page = { scrollH: se.scrollHeight, clientH: se.clientHeight, scrollW: se.scrollWidth, clientW: se.clientWidth,
      overflow: se.scrollHeight > se.clientHeight + 1 || se.scrollWidth > se.clientWidth + 1 || document.body.scrollHeight > document.body.clientHeight + 1 };
    out.scrollers = []; out.clipped = []; out.hclip = []; out.notip = [];
    const hasTip = el => !!el.closest('[title], [aria-label]');
    for (const el of document.querySelectorAll('body *')) {
      if (['CANVAS', 'SVG', 'svg', 'use', 'path', 'SELECT', 'INPUT', 'TEXTAREA', 'OPTION'].includes(el.tagName) || el.closest('svg')) continue;
      if (!visible(el)) continue;
      const cs = getComputedStyle(el);
      // sideways: content wider than the box. Text that ends in an ellipsis is allowed only with the full text in a tooltip.
      if (el.scrollWidth > el.clientWidth + 1 && el.clientWidth > 0) {
        if (cs.textOverflow === 'ellipsis') { if (!hasTip(el)) out.notip.push({ el: name(el), text: (el.textContent || '').trim().slice(0, 30) }); }
        else out.hclip.push({ el: name(el), scrollW: el.scrollWidth, clientW: el.clientWidth, text: (el.textContent || '').trim().slice(0, 30) });
      }
      if (el.scrollHeight <= el.clientHeight + 1) continue;
      const scrolls = /(auto|scroll)/.test(cs.overflowY);
      const clips = /(hidden|clip)/.test(cs.overflowY);
      if (scrolls) out.scrollers.push({ el: name(el), scrollH: el.scrollHeight, clientH: el.clientHeight, allowed: isAllowed(el) });
      else if (clips && cs.textOverflow !== 'ellipsis' && !isAllowed(el)) out.clipped.push({ el: name(el), scrollH: el.scrollHeight, clientH: el.clientHeight });
    }
    if (document.body.scrollHeight > document.body.clientHeight + 1) out.clipped.push({ el: 'body', scrollH: document.body.scrollHeight, clientH: document.body.clientHeight });
    out.below = []; out.side = [];
    for (const el of document.querySelectorAll(CONTROLS)) {
      if (!visible(el)) continue;
      const r = el.getBoundingClientRect();
      // a control inside an allowed scroller is reachable by scrolling it
      let reach = true, why = null;
      for (let p = el.parentElement; p && p !== document.documentElement; p = p.parentElement) {
        const cs = getComputedStyle(p);
        if (isAllowed(p) && /(auto|scroll)/.test(cs.overflowY)) { why = null; break; }
        if (cs.overflowY !== 'visible' || cs.overflowX !== 'visible') {
          const pr = p.getBoundingClientRect();
          if (r.bottom > pr.bottom + 1 || r.top < pr.top - 1) { reach = false; why = why || name(p); }
          if (r.right > pr.right + 1 || r.left < pr.left - 1) { out.side.push({ el: name(el), by: name(p) }); }
        }
      }
      if (!reach && why) out.below.push({ el: name(el), text: (el.innerText || el.value || '').trim().slice(0, 40).replace(/\s+/g, ' '), by: why, bottom: Math.round(r.bottom) });
      else if (why === null && r.bottom > innerHeight + 1 && !el.closest('.drives, .hist') && !(allowSel && [...document.querySelectorAll(allowSel)].some(a => a.contains(el) && /(auto|scroll)/.test(getComputedStyle(a).overflowY)))) {
        out.below.push({ el: name(el), text: (el.innerText || el.value || '').trim().slice(0, 40).replace(/\s+/g, ' '), by: 'window', bottom: Math.round(r.bottom) });
      }
      if (r.right > innerWidth + 1 || r.left < -1) out.side.push({ el: name(el), by: 'window' });
    }
    return out;
  };
  // one call: run a scenario and measure it. spec: {name, lang, allow, opts}
  window.__umcRun = function (spec) {
    const res = { name: spec.name, lang: spec.lang, vw: innerWidth, vh: innerHeight };
    window.__sent.length = 0;
    try {
      window.__umcScenario(spec.name, spec.lang, spec.opts);
      Object.assign(res, window.__umcMeasure(spec.allow));
    } catch (e) { res.error = String(e && e.message || e) + " @" + String(e && e.stack || "").split("\n").slice(0, 2).join(" | "); }
    // opening the Support dialog asks Rust for the list: expected. Anything that starts or stops a task is not.
    res.sent = window.__sent.filter(m => ['speed', 'capacity', 'fix', 'verifyRepaired', 'cancel', 'quit'].includes(m.action)).length;
    return res;
  };
})();
