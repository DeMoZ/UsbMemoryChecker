// Layout-check harness. It is evaluated inside the composed page (index.html + i18n.js) and builds fake Rust
// messages for named display states. It never sends anything to Rust and never starts a task:
//  - real messages from Rust are swallowed while the harness is loaded,
//  - `send` is replaced by a recorder (`window.__sent`).
// Usage: __umcScenario(name, lang, opts) -> 'ok' (throws on a JS error). `name` is one of __umcScenarios.
(function () {
  if (window.__umcScenario) return;
  const realHandler = window.onRustMessage;
  window.onRustMessage = function () { /* messages from the app are ignored while the harness is loaded */ };
  window.__sent = [];
  window.send = function (action, data) { window.__sent.push({ action, ...(data || {}) }); };
  // the page's own animations and transitions would make measurements and snapshots depend on timing
  const st = document.createElement('style');
  st.id = 'umc-check-style';
  st.textContent = '*,*::before,*::after{animation:none!important;transition:none!important}';
  document.head.appendChild(st);

  const R = msg => realHandler(msg);
  window.__rust = R;   // multi-step tests (ipc.mjs) feed messages as Rust would
  const GB = 1e9, MiB = 1048576;
  const decl = 125829120000, part = 8303673344;
  const limitSz = Math.floor((part - 2 * MiB) / MiB) * MiB;

  const vol = (mount, label, fs, total, free) => ({ mount, label, fs, total, free, writable: true });
  const mkDisk = (id, name, kind, bus, size, volumes) => ({ id, device: '/dev/r' + id, name, kind, bus, size, blockSize: 512, volumes, partitions: [{ offset: MiB, size: size - MiB, id: 'p' + id }], tableId: '' });
  const dFake = () => mkDisk('disk6', 'SanDisk Ultra Flair', 'usb', 'USB', decl, [vol('/Volumes/USB', 'USB', 'exFAT', decl - MiB, decl - 2.1 * GB)]);
  const dPart = () => mkDisk('disk6', 'SanDisk Ultra Flair', 'usb', 'USB', decl, [vol('/Volumes/USB', 'USB', 'exFAT', part, part - 30 * MiB)]);
  const dSd = () => mkDisk('disk7', 'Generic SD/MMC Card Reader', 'sd', 'Secure Digital', 63864569856, [vol('/Volumes/CAMERA', 'CAMERA', 'exFAT', 63860000000, 41200000000)]);
  const d2tb = () => mkDisk('disk8', 'USB Flash Disk 2TB', 'usb', 'USB', 2000398934016, [vol('/Volumes/NO NAME', 'NO NAME', 'FAT32', 2000390000000, 1999900000000)]);
  const dUnmounted = () => mkDisk('disk9', 'Kingston DataTraveler', 'usb', 'USB', 31914983424, []);

  const plan = (def, limit, whole) => ({ default: def, limit, whole });
  const view = o => ({ state: 'unknown', verified: false, value: 0, declared: decl, partition: null, layoutKey: 'k1', lossReal: null, unused: 0,
    check: null, remembered: false, repair: plan('whole', null, decl), ...o });
  const now = () => Math.floor(Date.now() / 1000);
  const quick = (o = {}) => ({ mode: 'quick', scope: 'device', verdict: 'fake', real: part, declared: decl, volume: 0, tested: 0, bad: 0, stoppedEarly: false,
    writeError: null, points: 1024, seconds: 71, restoreFailures: 0, cutShort: false, nearlyFull: false, at: now() - 3600, ...o });
  const full = (o = {}) => quick({ mode: 'full', points: 0, seconds: 1460, tested: part - 40 * MiB, volume: part, ...o });

  const fakeView = (extra = {}) => view({ state: 'fake', value: part, lossReal: part, check: quick(), repair: plan('limit', limitSz, decl), ...extra });
  const partView = (state, o = {}) => view({ state, value: part, partition: part, unused: decl - part - MiB, lossReal: part, repair: plan('limit', limitSz, decl), ...o });
  const speedRes = (mb, w, r) => ({ mb, speed: { write: { avg: w, max: w * 1.3, min: w * 0.6, seconds: mb / w }, read: { avg: r, max: r * 1.1, min: r * 0.8, seconds: mb / r }, badBlocks: 0 } });

  // name -> {tab, disks, sel, cap, run: (after the disks message) ...}
  const S = {};
  const def = (name, f) => { S[name] = f; };
  const only = (cap, tab, extra = {}) => ({ disks: [dFake(), dSd(), d2tb()], sel: 'disk6', cap: { disk6: cap }, tab, ...extra });
  const partDisks = () => [dPart(), dSd(), d2tb()];
  const mkSeries = (n, wy, ry, rn) => {
    const w = [], r = [];
    for (let i = 0; i <= n; i++) w.push({ x: i / 100, y: wy + 8 * Math.sin(i / 5) + (i > 40 ? -14 : 0) + (i % 7) });
    for (let i = 0; i <= (rn ?? 0); i++) r.push({ x: i / 100, y: ry + 6 * Math.sin(i / 3) });
    return { write: w, read: r };
  };

  def('nodrives', () => ({ disks: [], sel: null, cap: {}, tab: 'speed' }));
  def('searching', () => ({ disks: [], sel: null, cap: {}, tab: 'speed', noScan: true }));
  def('drives12', () => ({ disks: [dFake(), dSd(), d2tb(), dUnmounted(), ...Array.from({ length: 8 }, (_, i) => mkDisk('disk' + (20 + i), 'Card reader ' + (i + 1), 'sd', 'Secure Digital', 31914983424, [vol('/Volumes/CARD' + i, 'CARD' + i, 'exFAT', 31900000000, 20e9)]))], sel: 'disk6', cap: { disk6: view() }, tab: 'speed' }));
  def('idle', () => only(view(), 'speed'));
  def('speedRun', () => only(view(), 'speed', { task: { kind: 'speed', disk: 'disk6', fraction: 0.35, stage: 'prWrite', mbps: 31.4 }, series: { disk: 'disk6', kind: 'speed', ...mkSeries(35, 36, 0, -1) } }));
  def('speedDone', () => only(view(), 'speed', { results: [[100, 41, 132], [1000, 38.6, 128.4], [5000, 22.1, 127.9]], series: { disk: 'disk6', kind: 'speed', ...mkSeries(100, 36, 125, 100) } }));
  def('speedDone8', () => only(view(), 'speed', { results: [[100, 41, 132], [1000, 38.6, 128.4], [5000, 22.1, 127.9], [100, 40, 131], [1000, 39, 127], [100, 42, 133], [1000, 37, 129], [5000, 21, 126]], series: { disk: 'disk6', kind: 'speed', ...mkSeries(100, 36, 125, 100) } }));
  def('speedBad', () => only(view(), 'speed', { results: [[100, 41, 132], [1000, 38.6, 128.4], [5000, 22.1, 127.9]], bad: 3, series: { disk: 'disk6', kind: 'speed', ...mkSeries(100, 36, 125, 100) } }));
  def('speedRunRead', () => only(view(), 'speed', { task: { kind: 'speed', disk: 'disk6', fraction: 0.8, stage: 'prRead', mbps: 128.4 }, series: { disk: 'disk6', kind: 'speed', ...mkSeries(60, 36, 125, 30) } }));
  def('speedOther', () => only(view(), 'speed', { task: { kind: 'speed', disk: 'disk7', fraction: 0.2, stage: 'prWrite', mbps: 30 } }));
  def('speedUnmounted', () => ({ disks: [dUnmounted(), dSd()], sel: 'disk9', cap: { disk9: view({ declared: 31914983424, repair: plan('whole', null, 31914983424) }) }, tab: 'speed' }));

  def('capUnknown', () => only(view(), 'capacity'));
  def('capGenuine', () => only(view({ state: 'genuine', value: decl, check: quick({ verdict: 'genuine', real: decl, points: 1024 }) }), 'capacity'));
  def('capFake', () => only(fakeView(), 'capacity'));
  def('capDamaged', () => only(view({ state: 'damaged', value: part, lossReal: part, check: quick({ verdict: 'damaged', bad: 12 }), repair: plan('limit', limitSz, decl) }), 'capacity'));
  def('capBroken', () => only(view({ state: 'broken', value: 0, lossReal: 0, check: quick({ verdict: 'broken', real: 0 }), repair: plan('none', null, decl) }), 'capacity'));
  def('capFull2tb', () => ({ disks: [dFake(), dSd(), d2tb()], sel: 'disk8', tab: 'capacity', mode: 'full', cap: { disk8: view({ declared: d2tb().size, state: 'fake', value: 31.9 * GB, lossReal: 31.9 * GB,
    check: full({ declared: d2tb().size, real: 31.9 * GB, tested: 64.2 * GB, bad: 32.3 * GB, volume: d2tb().size, stoppedEarly: true, seconds: 2140, at: now() - 60 }), repair: plan('limit', 31.5 * GB, d2tb().size) }) } }));
  def('capRun', () => only(view(), 'capacity', { mode: 'full', task: { kind: 'full', disk: 'disk6', fraction: 0.62, stage: 'prVerifyEarly', mbps: 88.2 }, series: { disk: 'disk6', kind: 'full', ...mkSeries(50, 30, 88, 62) } }));
  def('capQuickRun', () => only(view(), 'capacity', { task: { kind: 'probe', disk: 'disk6', fraction: 0.4, stage: 'prCoarse' } }));
  def('capRunRefine', () => only(view(), 'capacity', { task: { kind: 'probe', disk: 'disk6', fraction: 0.55, stage: 'prRefine' } }));
  def('capRunRestore', () => only(view(), 'capacity', { task: { kind: 'probe', disk: 'disk6', fraction: 0.9, stage: 'prRestore' } }));
  def('capOther', () => only(view(), 'capacity', { mode: 'full', task: { kind: 'full', disk: 'disk7', fraction: 0.3, stage: 'prFill', mbps: 40 } }));
  def('capRepaired', () => ({ disks: partDisks(), sel: 'disk6', tab: 'capacity', cap: { disk6: partView('repaired', { verified: true, check: full({ scope: 'partition', verdict: 'genuine' }), remembered: true }) } }));
  def('capPartOk', () => ({ disks: partDisks(), sel: 'disk6', tab: 'capacity', mode: 'full', cap: { disk6: partView('partitionOk', { check: full({ scope: 'partition', verdict: 'genuine', tested: part - 40 * MiB }), remembered: true, lossReal: null }) } }));
  def('capPartWriteError', () => ({ disks: partDisks(), sel: 'disk6', tab: 'capacity', mode: 'full', cap: { disk6: partView('repaired', { check: full({ scope: 'partition', verdict: 'genuine', writeError: 'Input/output error (os error 5)', cutShort: true }), remembered: true }) } }));
  def('capCutShort', () => ({ disks: partDisks(), sel: 'disk6', tab: 'capacity', mode: 'full', cap: { disk6: partView('partitionOk', { check: full({ scope: 'partition', verdict: 'genuine', cutShort: true, restoreFailures: 2 }), remembered: true }) } }));
  def('capNearlyFull', () => ({ disks: partDisks(), sel: 'disk6', tab: 'capacity', mode: 'full', cap: { disk6: partView('partitionOk', { check: full({ scope: 'partition', verdict: 'genuine', nearlyFull: true, tested: 20 * MiB }) }) } }));
  def('capLimited', () => ({ disks: partDisks(), sel: 'disk6', tab: 'capacity', cap: { disk6: partView('limited', { lossReal: null }) } }));
  def('capUnconfirmed', () => only(view({ state: 'unconfirmed', repair: plan('none', null, decl) }), 'capacity'));

  def('repNoPlan', () => only(view({ repair: plan('none', null, decl) }), 'repair'));
  def('repUnknown', () => only(view(), 'repair'));
  def('repGenuine', () => only(view({ state: 'genuine', value: decl, check: quick({ verdict: 'genuine', real: decl }) }), 'repair'));
  def('repFake', () => only(fakeView(), 'repair'));
  def('repFakeWhole', () => only(fakeView(), 'repair', { whole: true, confirm: true }));
  def('repDamaged', () => only(view({ state: 'damaged', value: part, lossReal: part, check: quick({ verdict: 'damaged' }), repair: plan('limit', limitSz, decl) }), 'repair', { whole: true }));
  def('repRepaired', () => ({ disks: partDisks(), sel: 'disk6', tab: 'repair', cap: { disk6: partView('repaired', { check: quick() }) } }));
  const offered = (ageMs, seen, extra = {}) => ({ disks: partDisks(), sel: 'disk6', tab: 'repair', cap: { disk6: partView('repaired', { check: quick() }) },
    repaired: { size: part, ageMs, seen, ...extra } });
  def('repRepairedOffer', () => offered(60000, true));
  def('repRepairedOfferWhole', () => ({ ...offered(60000, true), whole: true, confirm: true }));
  def('repOfferWaiting', () => offered(500, false, { oldKey: true }));
  def('repOfferNoMount', () => ({ ...offered(60000, false), disks: [{ ...dPart(), volumes: [] }, dSd(), d2tb()] }));
  def('repLimited', () => ({ disks: partDisks(), sel: 'disk6', tab: 'repair', cap: { disk6: partView('limited', { lossReal: null }) } }));
  def('repLimitedWhole', () => ({ disks: partDisks(), sel: 'disk6', tab: 'repair', cap: { disk6: partView('limited', { lossReal: null }) }, whole: true, confirm: true }));
  def('repPartOk', () => ({ disks: partDisks(), sel: 'disk6', tab: 'repair', cap: { disk6: partView('partitionOk', { check: full({ scope: 'partition', verdict: 'genuine' }) }) } }));
  def('repPartOkWhole', () => ({ disks: partDisks(), sel: 'disk6', tab: 'repair', cap: { disk6: partView('partitionOk', { check: full({ scope: 'partition', verdict: 'genuine' }) }) }, whole: true, confirm: true }));
  def('repRun', () => only(fakeView(), 'repair', { task: { kind: 'fix', disk: 'disk6', fraction: 0.5, stage: 'prFixing' } }));
  def('repBroken', () => only(view({ state: 'broken', value: 0, lossReal: 0, check: quick({ verdict: 'broken', real: 0 }), repair: plan('none', null, decl) }), 'repair'));
  def('repUnconfirmed', () => only(view({ state: 'unconfirmed', repair: plan('none', null, decl) }), 'repair'));

  // overlays on top of a base state
  def('update', () => ({ ...only(view(), 'speed'), update: true }));
  def('toast', () => ({ ...only(fakeView(), 'capacity'), toast: 'errFullInconclusive' }));
  def('toastRescan', () => ({ ...only(view(), 'speed'), toast: 'rescanSlow', toastKind: 'info' }));
  def('modalDonate', () => ({ ...only(view(), 'speed'), modal: 'donate' }));
  def('modalUpdateChecking', () => ({ ...only(view(), 'speed'), modal: 'update', upd: { checking: true, status: null } }));
  def('modalUpdateDownloading', () => ({ ...only(view(), 'speed'), modal: 'update', writable: true, upd: { downloading: true, status: { key: 'updDownloading', args: { v: '0.2.0' }, kind: 'new' } } }));
  def('modalUpdateInstall', () => ({ ...only(view(), 'speed'), modal: 'update', writable: true }));
  def('toastHelper', () => ({ ...only(fakeView(), 'capacity'), toast: 'errHelper', toastArgs: { err: 'diskutil partitionDisk disk6 failed: Could not unmount disk, a program is still using it (error code 5), and the drive was left unchanged' } }));
  def('toastNoSpaceFull', () => ({ ...only(view(), 'speed'), toast: 'errNoSpaceFull', toastArgs: { free: '12 MB' } }));
  def('modalUpdate', () => ({ ...only(view(), 'speed'), modal: 'update' }));
  def('modalRepair', () => ({ ...only(fakeView(), 'repair'), whole: true, confirm: true, modal: 'repair' }));
  def('modalQuit', () => ({ ...only(view(), 'speed'), modal: 'quit' }));

  window.__umcScenarios = Object.keys(S);

  function reset() {
    for (const id of ['donateModal', 'updateModal', 'repairModal', 'quitModal']) { const el = document.getElementById(id); if (el) el.classList.remove('open'); }
    document.getElementById('toasts').innerHTML = '';
    task = null; pendingVerify = null; askedRepair = null; lastRunKey = '';
    for (const p of ['speed', 'capacity', 'repair']) { const el = document.getElementById('prog-' + p); el.innerHTML = ''; el.classList.remove('on'); delete el.dataset.kind; delete el.dataset.stage; }
    fs = 'exfat'; speedMb = 1000; tab = 'speed';
    document.getElementById('repLabel').value = 'USB';
    document.getElementById('langSelect').value = '';
    series = { write: [], read: [] }; seriesDisk = null; seriesKind = null;
    for (const k of Object.keys(results)) delete results[k];
    capacity = {}; disks = []; disksLoaded = false; selected = null;
    if (rescanPending) clearRescan();
    document.getElementById('repWhole').checked = false;
    document.getElementById('repConfirm').checked = false;
    update = { checking: false, downloading: false, available: null, writable: false, status: null };
  }

  window.__umcScenario = function (name, lang, opts) {
    const f = S[name];
    if (!f) throw new Error('unknown scenario ' + name);
    const c = f();
    const o = { platform: 'macos', ...(opts || {}) };
    reset();
    R({ type: 'init', lang, language: lang, languages: ['en', 'ru', 'es', 'de', 'zh'], systemLang: 'en', version: '0.1.2', canUpdate: true,
      platform: o.platform, fat32Max: 32e9, speedSizes: [100, 1000, 5000], speedMb: 1000, capacityMode: c.mode || 'quick', autoUpdate: true });
    const wr = c.writable === undefined ? c.modal !== 'update' : c.writable;
    R({ type: 'update', checking: false, downloading: false, available: c.update || c.modal === 'update' ? { version: '0.2.0', notes: Array.from({ length: 12 }, (_, i) => `- change number ${i + 1} of the new version`).join('\n') } : null, writable: wr, status: null, ...(c.upd || {}) });
    if (!c.noScan) R({ type: 'disks', disks: c.disks, capacity: c.cap, scanned: true, rescanned: 0 });
    if (c.sel) selectDrive(c.sel);
    if (c.series) { series = { write: c.series.write, read: c.series.read }; seriesDisk = c.series.disk; seriesKind = c.series.kind; }
    for (const [mb, w, r] of c.results || []) { const x = speedRes(mb, w, r); if (c.bad) x.speed.badBlocks = c.bad; res('disk6').speed.push(x); }
    if (c.repaired) {
      const r = res('disk6');
      r.repaired = c.repaired.size; r.repairedAt = Date.now() - c.repaired.ageMs; r.offer = true; r.unmountedSeen = !!c.repaired.seen; r.diskSize = decl;
      r.oldVolumes = c.repaired.oldKey ? [volumeKey(c.disks[0].volumes[0])] : [];
    }
    if (c.task) R({ type: 'task', kind: c.task.kind, disk: c.task.disk, fraction: c.task.fraction, stage: c.task.stage, mbps: c.task.mbps });
    showTab(c.tab);
    if (c.whole) document.getElementById('repWhole').checked = true;
    if (c.confirm) document.getElementById('repConfirm').checked = true;
    renderAll();
    showTab(c.tab);
    if (c.toast) toast(t(c.toast, c.toastArgs || {}), c.toastKind || 'err', 600000);
    if (c.modal === 'donate') {
      openDonate();
      R({ type: 'donations', loading: true, offline: true, info: { message: 'Thank you for supporting the development of this free tool.', methods: [
        { id: 'cloudtips', title: 'CloudTips', description: 'Cards and SBP, for Russia', url: 'https://example.invalid/a' },
        { id: 'tribute', title: 'Tribute', description: 'Cards worldwide', url: 'https://example.invalid/b' }] } });
    } else if (c.modal === 'update') openUpdates();
    else if (c.modal === 'repair') askRepair();
    else if (c.modal === 'quit') R({ type: 'confirmQuit' });
    (window.drawCharts || window.drawChart)();
    return 'ok';
  };
})();
