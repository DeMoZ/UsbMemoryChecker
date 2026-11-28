//! UsbMemoryChecker — checks USB flash drives and memory cards: read/write speed, real capacity (finds fake
//! drives that report more than they hold) and repairs a fake drive so it shows only its real size.
//!
//! Window: tao + wry (system WebView), page in `ui/index.html`. Threads: the WebView lives only in the main
//! thread, so everything sent to the window goes as `UserEvent::Js` through the `EventLoopProxy`. One test runs
//! at a time (`State::task`) in its own thread; the quick probe and the repair run in an elevated copy of the
//! program (`helper`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bench;
mod capacity;
mod cli;
mod dio;
mod disks;
mod donations;
mod fullcheck;
mod helper;
mod i18n;
mod net;
mod pattern;
mod pipe;
mod probe;
mod rawdev;
mod report;
mod settings;
mod update;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{Value, json};
use settings::Settings;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::window::WindowBuilder;
use update::{Release, format_version};
use wry::WebViewBuilder;

#[derive(Debug)]
enum UserEvent {
    /// Script for the WebView (runs in the main thread).
    Js(String),
    DragWindow,
    Quit,
}

/// Sends messages to the window from any thread.
#[derive(Clone)]
struct Ui(EventLoopProxy<UserEvent>);

impl Ui {
    fn send(&self, msg: Value) {
        let script = format!("window.onRustMessage&&window.onRustMessage({msg});");
        let _ = self.0.send_event(UserEvent::Js(script));
    }

    /// The window translates: a key of the `I18N` dictionary and its parameters.
    fn error(&self, key: &str, args: Value) {
        self.send(json!({"type": "error", "key": key, "args": args}));
    }
}

#[derive(Default)]
struct UpdateState {
    checking: bool,
    manual: bool,
    downloading: bool,
    available: Option<Release>,
}

#[derive(Clone)]
struct Task {
    kind: &'static str,
    disk: String,
}

struct State {
    settings: Settings,
    lang: &'static str,
    donations: Vec<u8>,
    donations_loading: bool,
    updates: UpdateState,
    disks: Vec<disks::Disk>,
    /// The first scan of drives is done (until then the window says "looking for drives").
    scanned: bool,
    task: Option<Task>,
    /// Counts task starts and ends: a drive scan that overlapped one is stale and not applied.
    task_gen: u64,
    cancel: Arc<AtomicBool>,
    /// What is known about each connected drive, by `diskN`.
    capacity: HashMap<String, capacity::Entry>,
    /// What the last quick check of a drive asks the user to do, by `diskN`. In memory only.
    notices: HashMap<String, Notice>,
    /// The last report written in this run of the program, by `diskN`: the declared size of the drive and the file.
    reports: HashMap<String, (u64, PathBuf)>,
    /// `capacity.json`: the knowledge of drives that are not connected now.
    store: capacity::Store,
    /// When this run of the program started (a check made before is "remembered").
    started: u64,
    /// Ask the disk watcher to look now; the number is the window's Reload request (0 = no request).
    rescan: Sender<u64>,
}

type Shared = Arc<Mutex<State>>;

fn lock(state: &Shared) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Deserialize)]
struct Request {
    action: String,
    #[serde(default)]
    disk: Option<String>,
    #[serde(default)]
    mb: Option<u64>,
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    fs: Option<helper::Fs>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    value: Option<bool>,
    #[serde(default)]
    lang: Option<String>,
    /// `rescan`: the number of the Reload request the answer has to carry.
    #[serde(default)]
    seq: Option<u64>,
    /// `fix`: which plan the window showed (`limit` or `whole`) and the partition size it showed.
    #[serde(default)]
    plan: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    /// `fix`: the user ticked "use the full declared size".
    #[serde(default)]
    whole: Option<bool>,
    /// `fix` with `whole`: the state and limit of the drive the window showed when the box was ticked.
    #[serde(default)]
    shown_state: Option<String>,
    #[serde(default)]
    shown_limit: Option<u64>,
    /// `fix`: the key of the drive's layout the window showed.
    #[serde(default)]
    layout: Option<String>,
}

fn page() -> String {
    include_str!("ui/index.html").replace("/*I18N*/", include_str!("ui/i18n.js"))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // elevated copy: does the job from the folder and exits, no window
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--helper") {
        let dir = args.get(i + 1).map(PathBuf::from).unwrap_or_default();
        std::process::exit(helper::main(&dir));
    }
    if args.get(1).is_some_and(|a| a == "--cli") {
        std::process::exit(cli::main(&args[2..]));
    }

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let ui = Ui(event_loop.create_proxy());
    let settings = Settings::load();
    let (rescan_tx, rescan_rx) = channel::<u64>();
    let state: Shared = Arc::new(Mutex::new(State {
        lang: i18n::resolve(&settings.language),
        settings,
        donations: donations::cached(),
        donations_loading: false,
        updates: UpdateState::default(),
        disks: Vec::new(),
        scanned: false,
        task: None,
        task_gen: 0,
        cancel: Arc::new(AtomicBool::new(false)),
        capacity: HashMap::new(),
        notices: HashMap::new(),
        reports: HashMap::new(),
        store: capacity::Store::open(capacity::Store::default_path()),
        started: settings::now(),
        rescan: rescan_tx,
    }));
    {
        let (state, ui) = (Arc::clone(&state), ui.clone());
        std::thread::spawn(move || disk_watcher(rescan_rx, state, ui));
    }

    #[allow(unused_mut)]
    let mut builder = WindowBuilder::new()
        .with_title("UsbMemoryChecker")
        .with_inner_size(tao::dpi::LogicalSize::new(1180.0, 760.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(880.0, 600.0));
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::WindowBuilderExtMacOS;
        builder = builder.with_titlebar_transparent(true).with_fullsize_content_view(true).with_title_hidden(true);
    }
    let window = builder.build(&event_loop)?;
    // tao leaves the window class without an icon: take the .exe icon (resource 1, see build.rs) for the title bar
    // (small) and the taskbar and Alt+Tab (big) at the display's scale; without the resource the window stays as is
    #[cfg(windows)]
    {
        use tao::platform::windows::{IconExtWindows, WindowExtWindows};
        let px = |dip: f64| {
            let p = (dip * window.scale_factor()).round() as u32;
            Some(tao::dpi::PhysicalSize::new(p, p))
        };
        window.set_window_icon(tao::window::Icon::from_resource(1, px(16.0)).ok());
        window.set_taskbar_icon(tao::window::Icon::from_resource(1, px(32.0)).ok());
    }

    let ipc_state = Arc::clone(&state);
    let ipc_ui = ui.clone();
    let ipc_proxy = event_loop.create_proxy();
    let webview = WebViewBuilder::new()
        .with_html(page())
        .with_ipc_handler(move |msg| match serde_json::from_str::<Request>(msg.body()) {
            Ok(req) => handle_request(req, &ipc_state, &ipc_ui, &ipc_proxy),
            Err(e) => eprintln!("bad message from the window: {e}"),
        })
        .build(&window)?;

    // for screenshots and manual checks: a script to run in the page after it starts
    if let Ok(script) = std::env::var("UMC_DEV_SCRIPT") {
        let ui = ui.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(2500));
            let _ = ui.0.send_event(UserEvent::Js(script));
        });
    }

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                if lock(&state).task.is_some() {
                    ui.send(json!({"type": "confirmQuit"}));
                } else {
                    *control_flow = ControlFlow::Exit;
                }
            }
            Event::UserEvent(user_event) => match user_event {
                UserEvent::Js(script) => {
                    let _ = webview.evaluate_script(&script);
                }
                UserEvent::DragWindow => {
                    let _ = window.drag_window();
                }
                UserEvent::Quit => *control_flow = ControlFlow::Exit,
            },
            _ => {}
        }
    });
}

fn init_msg(st: &State) -> Value {
    json!({
        "type": "init",
        "lang": st.lang,
        "language": st.settings.language,
        "languages": i18n::LANGUAGES,
        "systemLang": i18n::system_language(),
        "version": env!("CARGO_PKG_VERSION"),
        "repo": update::REPO,
        "platform": if cfg!(windows) { "windows" } else { "macos" },
        "autoUpdate": st.settings.auto_update,
        "canUpdate": update::install_target().is_some(),
        "updateFailed": std::env::args().any(|a| a == "--update-failed"),
        "speedMb": st.settings.speed_mb,
        "speedSizes": settings::SPEED_SIZES,
        "capacityMode": st.settings.capacity_mode,
        "fat32Max": helper::FAT32_MAX,
    })
}

fn handle_request(req: Request, state: &Shared, ui: &Ui, proxy: &EventLoopProxy<UserEvent>) {
    match req.action.as_str() {
        "ready" => {
            let st = lock(state);
            ui.send(init_msg(&st));
            send_disks(&st, ui, json!({}));
            if let Some(t) = &st.task {
                ui.send(json!({"type": "task", "kind": t.kind, "disk": t.disk, "fraction": 0.0, "stage": "prStart"}));
            }
            let auto = st.settings.auto_update
                && update::install_target().is_some()
                && settings::now().saturating_sub(st.settings.last_update_check) >= 86_400;
            let _ = st.rescan.send(0);
            drop(st);
            if auto {
                check_updates(state, ui, false);
            }
        }
        "rescan" => {
            let _ = lock(state).rescan.send(req.seq.unwrap_or(0));
        }
        "drag" => {
            let _ = proxy.send_event(UserEvent::DragWindow);
        }
        "speed" => {
            let mb = req.mb.filter(|m| settings::SPEED_SIZES.contains(m)).unwrap_or(1000);
            {
                let mut st = lock(state);
                st.settings.speed_mb = mb;
                st.settings.save();
            }
            start_task(state, ui, "speed", req.disk, move |ctx| run_speed(ctx, mb));
        }
        "capacity" => {
            let full = req.mode.as_deref() == Some("full");
            {
                let mut st = lock(state);
                st.settings.capacity_mode = if full { "full" } else { "quick" }.into();
                st.settings.save();
            }
            if full {
                start_task(state, ui, "full", req.disk, run_full);
            } else {
                start_task(state, ui, "probe", req.disk, run_probe);
            }
        }
        // the full check of a freshly repaired drive (offered by the window); does not change the saved check mode
        "verifyRepaired" => start_task(state, ui, "full", req.disk, run_full),
        "fix" => {
            let (fs, label) = (req.fs.unwrap_or(helper::Fs::Exfat), req.label.unwrap_or_default());
            let (plan, size) = (req.plan.unwrap_or_default(), req.size.unwrap_or(0));
            let layout = req.layout;
            let shown = req.whole.unwrap_or(false).then(|| (req.shown_state.unwrap_or_default(), req.shown_limit));
            start_task(state, ui, "fix", req.disk, move |ctx| {
                let explicit = shown.as_ref().map(|(state, limit)| capacity::Shown { state, limit: *limit });
                run_fix(ctx, fs, label, &plan, size, explicit, layout.as_deref())
            });
        }
        "cancel" => lock(state).cancel.store(true, Ordering::Relaxed),
        "quit" => {
            // stop the test, give it a moment to clean up (test files, restore of the probe), then exit
            let st = lock(state);
            st.cancel.store(true, Ordering::Relaxed);
            drop(st);
            let (state, proxy) = (Arc::clone(state), proxy.clone());
            std::thread::spawn(move || {
                let start = Instant::now();
                while lock(&state).task.is_some() && start.elapsed() < Duration::from_secs(15) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                let _ = proxy.send_event(UserEvent::Quit);
            });
        }
        // both open something fixed: the window sends no address and no path
        "open_privacy" => {
            if cfg!(target_os = "macos") {
                let ui = ui.clone();
                std::thread::spawn(move || {
                    if let Err(e) = net::open_privacy_settings() {
                        ui.error("errBrowser", json!({"err": e.to_string()}));
                    }
                });
            }
        }
        "reveal_report" => {
            if let Some(path) = report_to_reveal(&lock(state), req.disk.as_deref()) {
                std::thread::spawn(move || {
                    if let Err(e) = net::reveal(&path) {
                        eprintln!("could not show the report: {e}");
                    }
                });
            }
        }
        "donate_open" => open_donations(state, ui),
        "open_url" => {
            if let Some(url) = req.url
                && let Err(e) = net::open_url(&url)
            {
                ui.error("errBrowser", json!({"err": e.to_string()}));
            }
        }
        "update_check" => check_updates(state, ui, true),
        "update_install" => install_update(state, ui, proxy),
        "update_skip" => {
            let mut st = lock(state);
            if let Some(r) = &st.updates.available {
                st.settings.skipped_version = format_version(r.version);
                st.settings.save();
            }
        }
        "update_page" => {
            let url = lock(state).updates.available.as_ref().map(|r| r.page_url.clone());
            let url = url
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| format!("https://github.com/{}/releases", update::REPO));
            let _ = net::open_url(&url);
        }
        "set_language" => {
            let mut st = lock(state);
            let setting = req.lang.unwrap_or_default();
            st.settings.language = i18n::supported(&setting).map(str::to_string).unwrap_or_default();
            st.lang = i18n::resolve(&st.settings.language);
            st.settings.save();
            ui.send(json!({"type": "lang", "lang": st.lang, "language": st.settings.language}));
        }
        "set_auto_update" => {
            let mut st = lock(state);
            st.settings.auto_update = req.value.unwrap_or(true);
            st.settings.save();
        }
        other => eprintln!("unknown action from the window: {other}"),
    }
}

// ---------------------------------------------------------------- drives

/// The `disks` message: the drive list with the capacity view of every drive (the window only displays it).
/// `extra` fields (`rescanned`, `stale`, `error`) are merged in.
fn send_disks(st: &State, ui: &Ui, extra: Value) {
    let views: serde_json::Map<String, Value> = st
        .disks
        .iter()
        .map(|d| {
            let busy = st.task.as_ref().is_some_and(|t| t.disk == d.id);
            let view = capacity::view_entry(st.capacity.get(&d.id), d, st.started, busy);
            (d.id.clone(), serde_json::to_value(view).unwrap_or(Value::Null))
        })
        .collect();
    let mut msg = json!({"type": "disks", "disks": st.disks, "scanned": st.scanned, "capacity": views,
        "notices": notices_of(st), "reports": reports_of(st)});
    if let (Some(m), Some(e)) = (msg.as_object_mut(), extra.as_object()) {
        m.extend(e.clone());
    }
    ui.send(msg);
}

/// The notices of the connected drives, for the window.
fn notices_of(st: &State) -> serde_json::Map<String, Value> {
    st.disks
        .iter()
        .filter_map(|d| {
            let notice = st.notices.get(&d.id).filter(|n| n.declared == d.size)?;
            Some((d.id.clone(), notice.message()))
        })
        .collect()
}

/// Which connected drives have a report to show.
fn reports_of(st: &State) -> serde_json::Map<String, Value> {
    st.disks
        .iter()
        .filter(|d| st.reports.get(&d.id).is_some_and(|(declared, _)| *declared == d.size))
        .map(|d| (d.id.clone(), Value::Bool(true)))
        .collect()
}

/// The report the window asks to see: the file this program wrote for the drive that is connected as `id` now.
fn report_to_reveal(st: &State, id: Option<&str>) -> Option<PathBuf> {
    let id = id?;
    let (declared, path) = st.reports.get(id)?;
    st.disks.iter().any(|d| d.id == id && d.size == *declared).then(|| path.clone())
}

/// What a finished scan sends to the window. Every Reload request (`asked` > 0) gets an answer, also when nothing
/// changed; a failed scan answers it with the previous list marked `stale`. None: nothing to send.
fn scan_reply(asked: u64, scan: Result<bool, &str>) -> Option<Value> {
    match scan {
        Ok(changed) => (changed || asked > 0).then(|| json!({"rescanned": asked})),
        Err(e) => (asked > 0).then(|| json!({"rescanned": asked, "stale": true, "error": e})),
    }
}

enum Applied {
    /// The scan overlapped a task start or end.
    Stale,
    /// The scan is applied; the `disks` message to send (if any) and the save of the store to run outside the lock.
    Done(Option<Value>, Option<capacity::SaveJob>),
}

/// Applies a finished drive scan (`generation`: the task counter when it started).
fn apply_scan(st: &mut State, generation: u64, list: Vec<disks::Disk>, asked: u64, now: u64) -> Applied {
    if st.task_gen != generation {
        return Applied::Stale;
    }
    let changed = st.disks != list || !st.scanned;
    st.scanned = true;
    // Knowledge of a drive that is gone or changed size is no longer shown (the saved records stay). During a task
    // every drive except the task's own is still kept in step, because the task's drive layout belongs to the task
    // (a repair re-keys its own record).
    let task_disk = st.task.as_ref().map(|t| t.disk.clone());
    let synced = capacity::sync(&mut st.capacity, &mut st.store, &list, now, task_disk.as_deref());
    st.disks = list;
    // a drive that waited for a re-plug and has left the list: it has been unplugged
    let State { notices, disks, .. } = st;
    let waiting = notices.len();
    notices.retain(|id, n| n.replug.is_none() || disks.iter().any(|d| &d.id == id && d.size == n.declared));
    let unplugged = notices.len() != waiting;
    let job = if synced { st.store.save_job() } else { None };
    Applied::Done(scan_reply(asked, Ok(changed || synced || unplugged)), job)
}

fn disk_watcher(rx: Receiver<u64>, state: Shared, ui: Ui) {
    let every = Duration::from_secs(if cfg!(windows) { 4 } else { 2 });
    // the newest Reload request that has not been answered yet
    let mut asked = 0u64;
    loop {
        // requests that came in before this pass started are answered by it
        while let Ok(n) = rx.try_recv() {
            asked = asked.max(n);
        }
        let generation = lock(&state).task_gen;
        match disks::list() {
            Ok(list) => {
                let mut guard = lock(&state);
                match apply_scan(&mut guard, generation, list, asked, settings::now()) {
                    // a task started or ended while the scan ran: it saw the drive before or half-way through the
                    // task and is not applied; the next scan answers the Reload request (at once)
                    Applied::Stale => {
                        drop(guard);
                        if asked > 0 {
                            continue;
                        }
                    }
                    Applied::Done(extra, job) => {
                        if let Some(extra) = extra {
                            send_disks(&guard, &ui, extra);
                        }
                        drop(guard);
                        // the file is written outside the lock (it is flushed to disk)
                        if let Some(job) = job {
                            job.run();
                        }
                        asked = 0;
                    }
                }
            }
            Err(e) => {
                eprintln!("drive list: {e}");
                if let Some(extra) = scan_reply(asked, Err(&e)) {
                    let mut st = lock(&state);
                    st.scanned = true;
                    send_disks(&st, &ui, extra);
                }
                asked = 0;
            }
        }
        match rx.recv_timeout(every) {
            Ok(n) => asked = asked.max(n),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

// ---------------------------------------------------------------- tests

/// What a running test gets.
struct TaskCtx {
    state: Shared,
    ui: Ui,
    kind: &'static str,
    disk: disks::Disk,
    cancel: Arc<AtomicBool>,
    last_progress: Instant,
}

impl TaskCtx {
    /// Progress for the window, at most ~8 times a second.
    fn progress(&mut self, fraction: f64, stage: &str, extra: Value) {
        if self.last_progress.elapsed() < Duration::from_millis(120) && fraction < 1.0 {
            return;
        }
        self.last_progress = Instant::now();
        let mut msg =
            json!({"type": "task", "kind": self.kind, "disk": self.disk.id, "fraction": fraction, "stage": stage});
        if let (Some(m), Some(e)) = (msg.as_object_mut(), extra.as_object()) {
            m.extend(e.clone());
        }
        self.ui.send(msg);
    }

    fn point(&self, op: &str, x: f64, mbps: f64) {
        self.ui.send(json!({"type": "point", "kind": self.kind, "disk": self.disk.id, "op": op, "x": x, "mbps": mbps}));
    }

    fn lang(&self) -> &'static str {
        lock(&self.state).lang
    }
}

/// Task outcome: Ok(result) or Err((error key, parameters)).
type TaskResult = Result<Value, (&'static str, Value)>;

/// No task may run on a drive that waits for a re-plug.
fn may_start(st: &State, disk: &disks::Disk) -> Result<(), &'static str> {
    match st.notices.get(&disk.id) {
        Some(n) if n.declared == disk.size && n.replug.is_some() => Err("errReplugFirst"),
        _ => Ok(()),
    }
}

/// A task of `disk` is about to start: refused while it waits for a re-plug; else the Full Disk Access message of
/// the drive goes (Ok(true) when there was one).
fn admit(st: &mut State, disk: &disks::Disk) -> Result<bool, &'static str> {
    may_start(st, disk)?;
    Ok(st.notices.remove(&disk.id).is_some())
}

fn start_task<F>(state: &Shared, ui: &Ui, kind: &'static str, disk: Option<String>, work: F)
where
    F: FnOnce(&mut TaskCtx) -> TaskResult + Send + 'static,
{
    let mut st = lock(state);
    if st.task.is_some() {
        ui.error("errBusy", json!({}));
        return;
    }
    if st.updates.downloading {
        ui.error("errBusy", json!({}));
        return;
    }
    let Some(disk) = disk.and_then(|id| st.disks.iter().find(|d| d.id == id).cloned()) else {
        ui.error("errNoDisk", json!({}));
        return;
    };
    let cleared = match admit(&mut st, &disk) {
        Ok(cleared) => cleared,
        Err(key) => {
            ui.error(key, json!({}));
            return;
        }
    };
    st.task = Some(Task { kind, disk: disk.id.clone() });
    st.task_gen += 1;
    st.cancel = Arc::new(AtomicBool::new(false));
    let cancel = Arc::clone(&st.cancel);
    if cleared {
        send_disks(&st, ui, json!({}));
    }
    drop(st);
    ui.send(json!({"type": "task", "kind": kind, "disk": disk.id, "fraction": 0.0, "stage": "prStart"}));
    let (state, ui) = (Arc::clone(state), ui.clone());
    std::thread::spawn(move || {
        let id = disk.id.clone();
        let mut ctx = TaskCtx {
            state: Arc::clone(&state),
            ui: ui.clone(),
            kind,
            disk,
            cancel,
            last_progress: Instant::now() - Duration::from_secs(1),
        };
        // a panic in the work (or in a helper thread of it) must not leave the task slot taken for good
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&mut ctx)))
            .unwrap_or_else(|_| Err(("errIo", json!({"err": "internal error"}))));
        let cancelled = ctx.cancel.load(Ordering::Relaxed);
        let mut st = lock(&state);
        st.task = None;
        st.task_gen += 1;
        let msg = match result {
            Ok(v) => json!({"type": "taskDone", "kind": kind, "disk": id, "ok": true, "result": v}),
            Err(_) if cancelled => {
                json!({"type": "taskDone", "kind": kind, "disk": id, "ok": false, "cancelled": true})
            }
            Err((key, args)) => {
                json!({"type": "taskDone", "kind": kind, "disk": id, "ok": false, "key": key, "args": args})
            }
        };
        ui.send(msg);
        // the state the window shows comes from the knowledge, so it gets the new one right away
        send_disks(&st, &ui, json!({}));
        let _ = st.rescan.send(0);
    });
}

fn volume_path(ctx: &TaskCtx) -> Result<PathBuf, (&'static str, Value)> {
    ctx.disk.main_volume().map(|v| PathBuf::from(&v.mount)).ok_or(("errNoVolume", json!({})))
}

fn run_speed(ctx: &mut TaskCtx, mb: u64) -> TaskResult {
    let mount = volume_path(ctx)?;
    let cancel = Arc::clone(&ctx.cancel);
    let r = bench::run(&mount, mb << 20, &cancel, &mut |p| {
        let half = if p.op == "write" { 0.0 } else { 0.5 };
        let x = p.done as f64 / p.total as f64;
        ctx.point(p.op, x, p.mbps);
        ctx.progress(half + x / 2.0, if p.op == "write" { "prWrite" } else { "prRead" }, json!({"mbps": p.mbps}));
    });
    match r {
        Ok(r) => Ok(json!({"mb": mb, "speed": r})),
        Err(bench::Error::Cancelled) => Err(("errCancelled", json!({}))),
        Err(bench::Error::NoSpace(need, free)) => Err(("errNoSpace", json!({"need": need, "free": free}))),
        Err(bench::Error::Io(e)) => Err(("errIo", json!({"err": e}))),
    }
}

fn run_full(ctx: &mut TaskCtx) -> TaskResult {
    let started = settings::now();
    let mount = match volume_path(ctx) {
        Ok(mount) => mount,
        Err(e) => {
            report_full(ctx, started, None, &Err(e.clone()));
            return Err(e);
        }
    };
    let cancel = Arc::clone(&ctx.cancel);
    let (mut last_write_x, mut rescaled) = (0.0f64, false);
    let r = fullcheck::run(&mount, None, &cancel, &mut |p| {
        let x = if p.total == 0 { 0.0 } else { p.done as f64 / p.total as f64 };
        if p.op == "write" {
            last_write_x = x;
        } else if !rescaled && last_write_x > 0.0 && last_write_x < 0.999 {
            // writing ended before the plan (early stop, full volume): stretch the write curve over the same width as the read-back
            rescaled = true;
            ctx.ui.send(json!({"type": "rescale", "disk": ctx.disk.id, "op": "write", "factor": 1.0 / last_write_x}));
        }
        let fraction = if p.op == "write" { x * 0.5 } else { 0.5 + x * 0.5 };
        if let Some(mbps) = p.mbps {
            ctx.point(p.op, x, mbps);
        }
        let stage = match (p.op == "write", p.early) {
            (true, _) => "prFill",
            (false, true) => "prVerifyEarly",
            (false, false) => "prVerify",
        };
        ctx.progress(fraction, stage, json!({"done": p.done, "total": p.total, "mbps": p.mbps}));
    });
    // the report takes the measurements before the record is worked out from them
    let part = report::Report::new(report::Kind::Full, &ctx.disk, started, settings::now()).full_data(
        Some(&r),
        &ctx.disk,
        &net::home_dir(),
    );
    let result = finish_full(&ctx.state, &ctx.disk, r);
    report_full(ctx, started, Some(part), &result);
    result
}

/// How a full check or a repair ended, for the report.
fn outcome_of(result: &TaskResult, cause: Option<String>) -> report::OutcomePart {
    match result {
        Ok(v) => report::OutcomePart {
            key: "ok".into(),
            verdict: v.pointer("/check/verdict").cloned(),
            real: v.pointer("/check/real").and_then(Value::as_u64),
            ..Default::default()
        },
        Err((key, args)) => report::OutcomePart {
            key: (*key).into(),
            cause,
            error: args["err"].as_str().map(str::to_string),
            ..Default::default()
        },
    }
}

/// `part`: the report with the measurements; None when the check did not get as far as running.
fn report_full(ctx: &TaskCtx, started: u64, part: Option<report::Report>, result: &TaskResult) {
    let home = net::home_dir();
    let part = part.unwrap_or_else(|| {
        report::Report::new(report::Kind::Full, &ctx.disk, started, settings::now()).full_data(None, &ctx.disk, &home)
    });
    let rep = part.ended(outcome_of(result, None), &ctx.disk, &home);
    save_report(&ctx.state, &ctx.disk, &report::dir(), rep);
}

/// Writes the report and remembers it for the "Show report" link; a failure changes nothing else.
fn save_report(state: &Shared, disk: &disks::Disk, dir: &Path, mut rep: report::Report) {
    match report::write(dir, &mut rep) {
        Ok(path) => {
            lock(state).reports.insert(disk.id.clone(), (disk.size, path));
        }
        Err(e) => eprintln!("could not write the report: {e}"),
    }
}

/// The outcome of the full check: the record is worked out from everything known about the drive (also what only
/// the store holds) and saved; a failed, cancelled or inconclusive check changes nothing.
fn finish_full(state: &Shared, disk: &disks::Disk, r: Result<fullcheck::Outcome, fullcheck::Error>) -> TaskResult {
    match r {
        Ok(o) => {
            let declared = disk.size;
            let result = capacity::FullResult {
                total: o.total,
                used_before: o.used_before,
                tested: o.tested,
                bad: o.bad,
                real_estimate: o.real_estimate,
                stopped_early: o.stopped_early,
                write_error: o.write_error.clone(),
                seconds: o.write.seconds + o.read.seconds,
            };
            if capacity::wrote_too_little(&result) {
                let err = o.write_error.clone().unwrap_or_else(|| "the drive accepted almost no data".into());
                return Err(("errIo", json!({"err": err})));
            }
            let disk = latest(state, disk);
            let mut made = None;
            record(state, &disk, None, |k| {
                let rec = capacity::full_record(Some(k), &disk, &result, settings::now());
                k.apply_full(rec.clone());
                made = Some(rec);
            });
            Ok(json!({"full": o, "reported": declared, "check": made}))
        }
        Err(fullcheck::Error::Cancelled) => Err(("errCancelled", json!({}))),
        Err(fullcheck::Error::NoSpace(free)) => Err(("errNoSpaceFull", json!({"free": free}))),
        Err(fullcheck::Error::Io(e)) => Err(("errIo", json!({"err": e}))),
        Err(fullcheck::Error::Inconclusive) => Err(("errFullInconclusive", json!({}))),
        Err(fullcheck::Error::Gone) => Err(("errDriveGone", json!({}))),
    }
}

/// The drive as the last scan saw it (a check may have remounted its volume), else the one the task started with.
/// A layout that cannot be read at that moment (a listing that lost the partition table once) does not replace a
/// readable one the task started with: the result belongs to the drive that was checked.
fn latest(state: &Shared, disk: &disks::Disk) -> disks::Disk {
    let st = lock(state);
    let now = st.disks.iter().find(|d| d.id == disk.id && d.size == disk.size);
    match now {
        Some(d) if capacity::Fingerprint::of(d).usable() || !capacity::Fingerprint::of(disk).usable() => d.clone(),
        _ => disk.clone(),
    }
}

/// Changes the knowledge of a drive and saves it (see `capacity::record_change`). `repair`: our own repair, with
/// the layout the drive had before it.
fn record(
    state: &Shared,
    disk: &disks::Disk,
    repair: Option<&capacity::Fingerprint>,
    change: impl FnOnce(&mut capacity::Knowledge),
) {
    let mut guard = lock(state);
    let st = &mut *guard;
    let job = if capacity::record_change(&mut st.capacity, &mut st.store, disk, settings::now(), repair, change) {
        st.store.save_job()
    } else {
        None
    };
    drop(guard);
    if let Some(job) = job {
        job.run();
    }
}

fn elevated(ctx: &mut TaskCtx, job: &helper::Job) -> helper::Ran {
    let prompt = i18n::admin_prompt(ctx.lang());
    let cancel = Arc::clone(&ctx.cancel);
    helper::run(job, prompt, &cancel, &mut |f, stage| ctx.progress(f, &stage, json!({})))
}

fn run_probe(ctx: &mut TaskCtx) -> TaskResult {
    let job = helper::Job {
        action: helper::Action::Probe,
        disk: ctx.disk.id.clone(),
        size: ctx.disk.size,
        real: 0,
        partition: 0,
        layout: Some(capacity::Fingerprint::of(&latest(&ctx.state, &ctx.disk))),
        whole_confirmed: false,
        fs: helper::Fs::Exfat,
        label: String::new(),
    };
    let started = settings::now();
    let ran = elevated(ctx, &job);
    let (result, judged) = finish_probe(&ctx.state, &ctx.disk, &ran.result, &ran.log);
    report_probe(&ctx.state, &ctx.disk, &report::dir(), started, &ran, &judged);
    result
}

fn report_probe(state: &Shared, disk: &disks::Disk, dir: &Path, started: u64, ran: &helper::Ran, judged: &Judged) {
    let home = net::home_dir();
    let rep = report::Report::new(report::Kind::Probe, disk, started, settings::now())
        .probe_data(&ran.log, &ran.result, disk, &home)
        .ended(judged.outcome_part(&ran.result), disk, &home);
    save_report(state, disk, dir, rep);
}

/// What the last quick check of a drive asks the user to do. It lives in memory only.
#[derive(Clone, Debug, PartialEq)]
struct Notice {
    declared: u64,
    /// macOS blocked the access: the Full Disk Access steps are shown until the next task of the drive.
    fda: bool,
    /// The result cannot be trusted: nothing runs on the drive until it has left the drive list.
    replug: Option<Replug>,
}

impl Notice {
    fn message(&self) -> Value {
        let (reason, counts) = match &self.replug {
            Some(r) => (r.reason, r.counts.clone()),
            None => ("", json!({})),
        };
        let kind = if self.replug.is_some() { "replug" } else { "fullDiskAccess" };
        json!({"kind": kind, "reason": reason, "counts": counts, "fda": self.fda})
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Replug {
    /// `unresponsive`, `restoreFailed`, `tableGone`, `mismatch`, `unreadable`, `layoutChanged`, `driveGone` or
    /// `layoutUnknown`.
    pub reason: &'static str,
    /// `io`, `mismatches`, `unsaved`, `restoreFailures` and `n`, the number the reason is about.
    pub counts: Value,
}

/// What the window makes of the end of a quick check.
pub(crate) struct Judged {
    /// The outcome when it is trusted (and goes into the knowledge of the drive), else the error to show.
    pub result: Result<probe::Outcome, (&'static str, Value)>,
    /// macOS privacy protection blocked the access.
    pub fda: bool,
    pub replug: Option<Replug>,
    /// The outcome the helper gave, trusted or not.
    pub seen: Option<probe::Outcome>,
}

impl Judged {
    pub fn outcome_part(&self, helper_result: &Result<Value, helper::RunError>) -> report::OutcomePart {
        let (cause, message) = match helper_result {
            Err(helper::RunError::Failed { cause, message, .. }) => (cause.clone(), Some(message.clone())),
            _ => (None, None),
        };
        report::OutcomePart {
            key: self.result.as_ref().err().map_or("ok", |e| e.0).into(),
            cause,
            verdict: self.seen.as_ref().map(|o| json!(o.verdict)),
            real: self.seen.as_ref().map(|o| o.real),
            error: message,
            trusted: Some(self.result.is_ok()),
            notice: self.replug.as_ref().map(|r| r.reason.to_string()),
        }
    }
}

/// The numbers of a quick check that a notice shows.
#[derive(Clone, Copy, Default)]
struct Counts {
    io: u64,
    mismatches: u64,
    unsaved: u64,
    restore_failures: u64,
}

/// Why the drive has to be unplugged before anything else, the most telling reason first. `layout`: None when the
/// helper did not look at the layout.
fn replug_reason(
    unresponsive: bool,
    counts: Counts,
    check: &probe::RestoreCheck,
    layout: Option<helper::LayoutAfter>,
) -> Option<Replug> {
    use helper::LayoutAfter as L;
    let (reason, n) = if unresponsive {
        ("unresponsive", 0)
    } else if counts.restore_failures > 0 {
        ("restoreFailed", counts.restore_failures)
    } else if layout == Some(L::TableGone) {
        ("tableGone", 0)
    } else if check.mismatched > 0 {
        ("mismatch", check.mismatched as u64)
    } else if check.unreadable > 0 {
        ("unreadable", check.unreadable as u64)
    } else {
        match layout? {
            L::Same | L::TableGone => return None,
            L::Changed => ("layoutChanged", 0),
            L::DriveGone => ("driveGone", 0),
            L::Unreadable => ("layoutUnknown", 0),
        }
    };
    let counts = json!({"io": counts.io, "mismatches": counts.mismatches, "unsaved": counts.unsaved,
        "restoreFailures": counts.restore_failures, "n": n});
    Some(Replug { reason, counts })
}

/// The helper's log shows that the probe may have written (the baseline is done) and has no read-back check.
fn may_have_written(log: &[String]) -> bool {
    let stages: Vec<(String, bool)> = log
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| Some((v["stage"].as_str()?.to_string(), v["ok"].as_bool()?)))
        .collect();
    stages.iter().any(|(s, ok)| s == "baseline" && *ok) && !stages.iter().any(|(s, _)| s == "verify")
}

/// Decides what the end of a quick check means: a trusted outcome or an error, and what the user has to do.
pub(crate) fn judge_probe(r: &Result<Value, helper::RunError>, log: &[String]) -> Judged {
    let failed = |key, args, fda, replug| Judged { result: Err((key, args)), fda, replug, seen: None };
    // no word about the restore at all, although the probe may have written: not confirmed
    let unfinished = || {
        may_have_written(log)
            .then(|| {
                replug_reason(false, Counts::default(), &Default::default(), Some(helper::LayoutAfter::Unreadable))
            })
            .flatten()
    };
    match r {
        Err(helper::RunError::Declined) => failed("errDeclined", json!({}), false, None),
        Err(helper::RunError::Failed { message, cause, details }) => {
            let cause = cause.as_deref().unwrap_or("other");
            let number = |k: &str| details["counts"][k].as_u64().unwrap_or(0);
            let counts = Counts {
                io: number("ioErrors"),
                mismatches: number("mismatches"),
                unsaved: number("unsaved"),
                restore_failures: number("restoreFailures"),
            };
            let check: probe::RestoreCheck =
                serde_json::from_value(details["restoreCheck"].clone()).unwrap_or_default();
            let layout: Option<helper::LayoutAfter> = serde_json::from_value(details["layoutAfter"].clone()).ok();
            let replug = if details.is_null() {
                unfinished()
            } else {
                replug_reason(cause == "unresponsive", counts, &check, layout)
            };
            let (key, args) = match cause {
                "privacy" => ("errFullDiskAccess", json!({})),
                "busy" => ("errAccessBusy", json!({})),
                "readOnly" => ("errAccessReadOnly", json!({})),
                "gone" => ("errDriveGone", json!({})),
                "denied" => ("errAccess", json!({"err": message})),
                "unresponsive" => ("errUnresponsive", json!({"io": counts.io, "mismatches": counts.mismatches})),
                "cancelled" => ("errCancelled", json!({})),
                _ => ("errHelper", json!({"err": message})),
            };
            failed(key, args, cause == "privacy", replug)
        }
        Ok(v) => {
            let outcome: probe::Outcome = match serde_json::from_value(v.clone()) {
                Ok(o) => o,
                Err(e) => {
                    let args = json!({"err": format!("unreadable probe result: {e}")});
                    return failed("errHelper", args, false, unfinished());
                }
            };
            // a result that does not say how the layout looks is not trusted
            let layout = serde_json::from_value(v["layoutAfter"].clone()).unwrap_or(helper::LayoutAfter::Unreadable);
            let counts = Counts {
                io: outcome.io_errors as u64,
                mismatches: outcome.mismatches as u64,
                unsaved: outcome.unsaved as u64,
                restore_failures: outcome.restore_failures as u64,
            };
            let replug = replug_reason(false, counts, &outcome.restore_check, Some(layout));
            let result = match &replug {
                None => Ok(outcome.clone()),
                Some(r) => Err(("errNotRestored", json!({"reason": r.reason, "n": r.counts["n"]}))),
            };
            Judged { result, fda: false, replug, seen: Some(outcome) }
        }
    }
}

/// The outcome of the quick check: only a trusted one changes the knowledge of the drive. A declined, failed or
/// untrusted one changes nothing, and may leave a notice for the user.
fn finish_probe(
    state: &Shared,
    disk: &disks::Disk,
    r: &Result<Value, helper::RunError>,
    log: &[String],
) -> (TaskResult, Judged) {
    let judged = judge_probe(r, log);
    if judged.fda || judged.replug.is_some() {
        let notice = Notice { declared: disk.size, fda: judged.fda, replug: judged.replug.clone() };
        lock(state).notices.insert(disk.id.clone(), notice);
    }
    let outcome = match &judged.result {
        Ok(outcome) => outcome.clone(),
        Err(e) => return (Err(e.clone()), judged),
    };
    let declared = disk.size;
    let verdict = match outcome.verdict {
        probe::Verdict::Genuine => capacity::Verdict::Genuine,
        probe::Verdict::Fake => capacity::Verdict::Fake,
        probe::Verdict::Damaged => capacity::Verdict::Damaged,
        probe::Verdict::Broken => capacity::Verdict::Broken,
    };
    let rec = capacity::CheckRecord {
        mode: capacity::Mode::Quick,
        scope: capacity::Scope::Device,
        verdict,
        real: if verdict == capacity::Verdict::Genuine { declared } else { outcome.real },
        declared,
        points: outcome.points,
        seconds: outcome.seconds,
        restore_failures: outcome.restore_failures,
        io_errors: outcome.io_errors,
        mismatches: outcome.mismatches,
        unsaved: outcome.unsaved,
        restore_checked: true,
        at: settings::now(),
        ..Default::default()
    };
    record(state, &latest(state, disk), None, |k| k.apply_quick(rec.clone()));
    (Ok(json!({"probe": outcome, "check": rec})), judged)
}

/// What the repair of `disk` needs before the helper starts: the exact partition size for the job (0 = the whole
/// drive) and the entry to restore if the drive drops out of the list meanwhile. The plan is worked out again from
/// everything known (the entry, else the store) and the current layout; a request that does not match is refused
/// here, so no password window appears and the drive stays as it was.
fn plan_repair(
    state: &Shared,
    disk: &disks::Disk,
    plan: &str,
    size: u64,
    explicit: Option<capacity::Shown>,
    layout: Option<&str>,
) -> Result<(u64, Option<capacity::Entry>, disks::Disk), (&'static str, Value)> {
    let mut guard = lock(state);
    let st = &mut *guard;
    // before anything else: a drive that waits for a re-plug is not touched, whatever the request says
    may_start(st, disk).map_err(|key| (key, json!({})))?;
    let disk = st.disks.iter().find(|d| d.id == disk.id && d.size == disk.size).unwrap_or(disk).clone();
    // the layout the window showed must be the one the drive has now (the helper checks the job's layout, which is
    // Rust's own view, so the window's view is compared here)
    if layout != Some(capacity::Fingerprint::of(&disk).key().as_str()) {
        return Err(("errRepairChanged", json!({})));
    }
    let before = capacity::entry_for(&st.capacity, &mut st.store, &disk, settings::now());
    // knowledge that the drive's layout does not confirm gives no plan
    let view = capacity::view_entry(before.as_ref(), &disk, st.started, false);
    let partition = capacity::check_fix(&view, plan, size, explicit).map_err(|key| (key, json!({})))?;
    Ok((partition, before, disk))
}

/// What the report of a repair needs beyond its result.
struct FixRun {
    /// The planned partition size: what the window asked for until the plan is accepted.
    planned: u64,
    log: Vec<String>,
    cause: Option<String>,
}

/// The repair: the window says which plan it showed (`limit` or `whole`), which size, and whether the full size
/// was chosen explicitly.
fn run_fix(
    ctx: &mut TaskCtx,
    fs: helper::Fs,
    label: String,
    plan: &str,
    size: u64,
    explicit: Option<capacity::Shown>,
    layout: Option<&str>,
) -> TaskResult {
    let started = settings::now();
    let mut run = FixRun { planned: size, log: Vec::new(), cause: None };
    // the new volume's name is one more label that stays out of the report
    let named = report::with_new_label(&ctx.disk, &helper::clean_label(&label));
    let result = repair_drive(ctx, fs, label, plan, size, explicit, layout, &mut run);
    let home = net::home_dir();
    let fs_name = if fs == helper::Fs::Fat32 { "fat32" } else { "exfat" };
    let rep = report::Report::new(report::Kind::Fix, &ctx.disk, started, settings::now())
        .fix_data(run.planned, fs_name, &run.log, &named, &home)
        .ended(outcome_of(&result, run.cause), &named, &home);
    save_report(&ctx.state, &ctx.disk, &report::dir(), rep);
    result
}

#[allow(clippy::too_many_arguments)]
fn repair_drive(
    ctx: &mut TaskCtx,
    fs: helper::Fs,
    label: String,
    plan: &str,
    size: u64,
    explicit: Option<capacity::Shown>,
    layout: Option<&str>,
    run: &mut FixRun,
) -> TaskResult {
    let (partition, before, disk) = plan_repair(&ctx.state, &ctx.disk, plan, size, explicit, layout)?;
    let declared = ctx.disk.size;
    run.planned = if partition > 0 { partition } else { declared };
    let job = helper::Job {
        action: helper::Action::Fix,
        disk: ctx.disk.id.clone(),
        size: declared,
        real: 0,
        partition,
        layout: Some(capacity::Fingerprint::of(&disk)),
        // `check_fix` let the whole-drive plan through: it is the default of a genuine drive or the explicit choice
        whole_confirmed: partition == 0,
        fs,
        label,
    };
    let ran = elevated(ctx, &job);
    run.log = ran.log;
    match ran.result {
        Ok(_) => {}
        Err(helper::RunError::Declined) => return Err(("errDeclined", json!({}))),
        Err(helper::RunError::Failed { message, cause, .. }) => {
            run.cause = cause;
            return Err(("errHelper", json!({"err": message})));
        }
    }
    let repair = if partition > 0 { capacity::Repair::Partition(partition) } else { capacity::Repair::Whole };
    // the new layout: the list sent at the end of the task shows it, and the knowledge moves to it right away
    // when the listing fails, the drive is taken as one without a usable layout: nothing is stored under any key
    // (never the pre-repair layout, which a factory image shares with the drives of its batch) and the knowledge
    // stays in memory, unconfirmed
    let layout = disks::find(&ctx.disk.id, declared)
        .unwrap_or_else(|_| disks::Disk { partitions: Vec::new(), ..latest(&ctx.state, &ctx.disk) });
    let pre = capacity::Fingerprint::of(&disk);
    let mut guard = lock(&ctx.state);
    let st = &mut *guard;
    let now = settings::now();
    let job =
        if capacity::finish_repair(&mut st.capacity, &mut st.store, &mut st.disks, before, layout, &pre, repair, now) {
            st.store.save_job()
        } else {
            None
        };
    drop(guard);
    if let Some(job) = job {
        job.run();
    }
    Ok(json!({"size": if partition > 0 { partition } else { declared }}))
}

// ---------------------------------------------------------------- donations

fn donations_msg(st: &State, loading: bool, offline: bool) -> Value {
    let info = donations::info(&st.donations, st.lang);
    json!({"type": "donations", "info": info, "loading": loading, "offline": offline})
}

/// Downloaded again every time the window opens; the last copy shows until then.
fn open_donations(state: &Shared, ui: &Ui) {
    let mut st = lock(state);
    ui.send(donations_msg(&st, true, false));
    if st.donations_loading {
        return;
    }
    st.donations_loading = true;
    drop(st);
    let (state, ui) = (Arc::clone(state), ui.clone());
    std::thread::spawn(move || {
        let fresh = donations::download();
        let mut st = lock(&state);
        st.donations_loading = false;
        let offline = fresh.is_none();
        if let Some(data) = fresh {
            st.donations = data;
        }
        ui.send(donations_msg(&st, false, offline));
    });
}

// ---------------------------------------------------------------- updates

/// `status` — (kind: ok / err / new, dictionary key, parameters).
fn update_msg(st: &State, status: Option<(&str, &str, Value)>) -> Value {
    let target = update::install_target();
    let writable = target.as_deref().is_some_and(update::can_write);
    let available = st
        .updates
        .available
        .as_ref()
        .map(|r| json!({"version": format_version(r.version), "notes": r.notes, "page": r.page_url}));
    json!({
        "type": "update",
        "checking": st.updates.checking,
        "downloading": st.updates.downloading,
        "available": available,
        "writable": writable,
        "status": status.map(|(kind, key, args)| json!({"kind": kind, "key": key, "args": args})),
    })
}

fn check_updates(state: &Shared, ui: &Ui, manual: bool) {
    let mut st = lock(state);
    if st.updates.checking || st.updates.downloading {
        return;
    }
    st.updates.checking = true;
    st.updates.manual = manual;
    ui.send(update_msg(&st, None));
    drop(st);
    let (state, ui) = (Arc::clone(state), ui.clone());
    std::thread::spawn(move || {
        let result = update::latest();
        let mut st = lock(&state);
        st.updates.checking = false;
        let current = update::current();
        let status = match result {
            Ok(found) => {
                st.settings.last_update_check = settings::now();
                st.settings.save();
                match found.filter(|r| r.version > current) {
                    Some(r) => {
                        let v = format_version(r.version);
                        let offer = st.updates.manual || st.settings.skipped_version != v;
                        st.updates.available = Some(r);
                        let msg = update_msg(&st, Some(("new", "updNew", json!({"v": v}))));
                        ui.send(msg);
                        if offer {
                            ui.send(json!({"type": "update_offer"}));
                        }
                        return;
                    }
                    None => {
                        st.updates.available = None;
                        ("ok", "updLatest", json!({"v": format_version(current)}))
                    }
                }
            }
            Err(e) => ("err", "updCheckFailed", json!({"err": e})),
        };
        ui.send(update_msg(&st, Some(status)));
    });
}

fn install_update(state: &Shared, ui: &Ui, proxy: &EventLoopProxy<UserEvent>) {
    let mut st = lock(state);
    if st.updates.downloading {
        return;
    }
    if st.task.is_some() {
        ui.error("errWaitTask", json!({}));
        return;
    }
    let Some(release) = st.updates.available.clone() else { return };
    st.updates.downloading = true;
    ui.send(update_msg(&st, Some(("new", "updDownloading", json!({"v": format_version(release.version)})))));
    drop(st);
    let (state, ui, proxy) = (Arc::clone(state), ui.clone(), proxy.clone());
    std::thread::spawn(move || {
        let result = update::download(&release).and_then(|source| {
            let target = update::install_target().ok_or("could not find where the app is installed")?;
            update::start_install(&source, &target).map_err(|e| format!("could not start the installer: {e}"))
        });
        let mut st = lock(&state);
        st.updates.downloading = false;
        match result {
            Ok(()) => {
                ui.send(update_msg(&st, Some(("ok", "updRestarting", json!({})))));
                let _ = proxy.send_event(UserEvent::Quit);
            }
            Err(e) => ui.send(update_msg(&st, Some(("err", "updFailed", json!({"err": e}))))),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_is_always_answered() {
        // nothing changed: no message without a request, an answer with one
        assert_eq!(scan_reply(0, Ok(false)), None);
        assert_eq!(scan_reply(3, Ok(false)), Some(json!({"rescanned": 3})));
        // the list changed by itself
        assert_eq!(scan_reply(0, Ok(true)), Some(json!({"rescanned": 0})));
        // a failed scan: silent without a request, a stale answer with one
        assert_eq!(scan_reply(0, Err("boom")), None);
        assert_eq!(scan_reply(2, Err("boom")), Some(json!({"rescanned": 2, "stale": true, "error": "boom"})));
    }

    const DECLARED: u64 = 125_829_120_000;
    const MIB: u64 = 1 << 20;

    fn test_disk(partition: u64) -> disks::Disk {
        disks::Disk {
            id: "disk6".into(),
            device: "/dev/rdisk6".into(),
            name: "Storage Device".into(),
            kind: "usb".into(),
            bus: "USB".into(),
            size: DECLARED,
            block_size: 512,
            volumes: Vec::new(),
            partitions: vec![disks::Partition { offset: MIB, size: partition, id: "UUID".into() }],
            table_unreadable: false,
            table_id: String::new(),
            partition_fs: Vec::new(),
        }
    }

    fn key(disk: &disks::Disk) -> Option<&'static str> {
        Some(Box::leak(capacity::Fingerprint::of(disk).key().into_boxed_str()))
    }

    fn test_state(disk: &disks::Disk, knowledge: Option<capacity::Knowledge>) -> Shared {
        let (tx, _rx) = channel::<u64>();
        let mut capacity = HashMap::new();
        if let Some(knowledge) = knowledge {
            capacity
                .insert(disk.id.clone(), capacity::Entry::new(knowledge, Some(capacity::Fingerprint::of(disk)), false));
        }
        Arc::new(Mutex::new(State {
            notices: HashMap::new(),
            reports: HashMap::new(),
            settings: Settings::default(),
            lang: "en",
            donations: Vec::new(),
            donations_loading: false,
            updates: UpdateState::default(),
            disks: vec![disk.clone()],
            scanned: true,
            task: None,
            task_gen: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            capacity,
            store: capacity::Store::default(),
            started: 0,
            rescan: tx,
        }))
    }

    fn loss(real: u64) -> capacity::Knowledge {
        let mut k = capacity::Knowledge::new(DECLARED);
        k.apply_quick(capacity::CheckRecord {
            mode: capacity::Mode::Quick,
            verdict: capacity::Verdict::Fake,
            real,
            declared: DECLARED,
            at: 1,
            ..Default::default()
        });
        k
    }

    fn outcome(bad: u64, write_error: Option<&str>, tested: u64) -> fullcheck::Outcome {
        fullcheck::Outcome {
            tested,
            ok: tested - bad,
            bad,
            first_bad: None,
            total: DECLARED - 40 * MIB,
            used_before: 8 * MIB,
            real_estimate: tested,
            write: Default::default(),
            read: Default::default(),
            write_error: write_error.map(str::to_string),
            stopped_early: false,
        }
    }

    #[test]
    fn failed_cancelled_or_gone_checks_leave_the_record_unchanged() {
        let disk = test_disk(8_000_000_000);
        let state = test_state(&disk, Some(loss(8_303_673_344)));
        let before = lock(&state).capacity["disk6"].knowledge.clone();
        let cases: [(fullcheck::Error, &str); 5] = [
            (fullcheck::Error::Cancelled, "errCancelled"),
            (fullcheck::Error::NoSpace(1), "errNoSpaceFull"),
            (fullcheck::Error::Io("boom".into()), "errIo"),
            (fullcheck::Error::Inconclusive, "errFullInconclusive"),
            (fullcheck::Error::Gone, "errDriveGone"),
        ];
        for (error, key) in cases {
            let r = finish_full(&state, &disk, Err(error));
            assert_eq!(r.unwrap_err().0, key);
            assert_eq!(lock(&state).capacity["disk6"].knowledge, before);
        }
    }

    #[test]
    fn a_full_check_that_ended_by_a_write_error_keeps_the_known_loss() {
        let disk = disks::Disk {
            partitions: vec![disks::Partition { offset: MIB, size: DECLARED - 2 * MIB, id: "U".into() }],
            ..test_disk(0)
        };
        let state = test_state(&disk, Some(loss(8_303_673_344)));
        let r = finish_full(&state, &disk, Ok(outcome(0, Some("Input/output error"), 8_000_000_000))).unwrap();
        assert_eq!(r["check"]["scope"], "partition");
        let st = lock(&state);
        let k = &st.capacity["disk6"].knowledge;
        assert_eq!(k.loss.as_ref().map(|l| l.real), Some(8_303_673_344));
        assert!(k.last.as_ref().unwrap().write_error.is_some());
    }

    #[test]
    fn a_repair_request_that_does_not_match_never_reaches_the_helper() {
        let size = capacity::loss_repair_size(120_000_000_000, DECLARED);
        let disk = test_disk(size);
        let mut known = loss(120_000_000_000);
        known.apply_repair(capacity::Repair::Partition(size), 5);
        let state = test_state(&disk, Some(known));
        // the stale window: full size without the explicit choice, or a size that is not the plan
        assert_eq!(plan_repair(&state, &disk, "whole", DECLARED, None, key(&disk)).unwrap_err().0, "errRepairChanged");
        assert_eq!(
            plan_repair(&state, &disk, "limit", size + MIB, None, key(&disk)).unwrap_err().0,
            "errRepairChanged"
        );
        assert_eq!(plan_repair(&state, &disk, "nonsense", size, None, key(&disk)).unwrap_err().0, "errRepairChanged");
        // the plan itself, and the explicit full size
        assert_eq!(plan_repair(&state, &disk, "limit", size, None, key(&disk)).unwrap().0, size);
        let shown = capacity::Shown { state: "repaired", limit: Some(size) };
        assert_eq!(plan_repair(&state, &disk, "whole", DECLARED, Some(shown), key(&disk)).unwrap().0, 0);
        // the tick was made in another state than the one the drive has now
        let other = capacity::Shown { state: "unknown", limit: None };
        assert_eq!(
            plan_repair(&state, &disk, "whole", DECLARED, Some(other), key(&disk)).unwrap_err().0,
            "errRepairChanged"
        );
    }

    #[test]
    fn a_drive_that_keeps_nothing_is_refused_for_every_plan() {
        let disk = test_disk(8_000_000_000);
        let state = test_state(&disk, Some(loss(MIB)));
        let shown = capacity::Shown { state: "broken", limit: None };
        for (plan, size) in [("limit", 8_000_000_000), ("whole", DECLARED), ("limit", 0)] {
            assert_eq!(plan_repair(&state, &disk, plan, size, Some(shown), key(&disk)).unwrap_err().0, "errBroken");
        }
    }

    #[test]
    fn a_scan_that_overlapped_a_task_is_not_applied() {
        let disk = test_disk(8_000_000_000);
        let state = test_state(&disk, None);
        let mut st = lock(&state);
        let other = disks::Disk { id: "disk9".into(), ..test_disk(1_000_000_000) };
        // the scan started at generation 0, a task started and ended meanwhile
        st.task_gen = 2;
        assert!(matches!(apply_scan(&mut st, 0, vec![other.clone()], 7, 100), Applied::Stale));
        assert_eq!(st.disks, vec![disk.clone()], "the stale list is not taken");
        // the next scan is applied and answers the Reload request even when nothing changed
        match apply_scan(&mut st, 2, vec![disk.clone()], 7, 101) {
            Applied::Done(Some(extra), _) => assert_eq!(extra, json!({"rescanned": 7})),
            _ => panic!("the Reload request must be answered"),
        }
        assert!(matches!(apply_scan(&mut st, 2, vec![disk], 0, 102), Applied::Done(None, _)));
    }

    #[test]
    fn a_scan_that_changed_only_the_knowledge_is_sent() {
        // a repaired drive plugged in during a task: listed without knowledge; after the task the store is read
        let size = capacity::loss_repair_size(120_000_000_000, DECLARED);
        let disk = test_disk(size);
        let state = test_state(&disk, None);
        let mut st = lock(&state);
        let mut known = loss(120_000_000_000);
        known.apply_repair(capacity::Repair::Partition(size), 5);
        st.store.put(None, capacity::Fingerprint::of(&disk), known, 1);
        // the list is the same, but sync loads the record: a message goes out although the list did not change
        match apply_scan(&mut st, 0, vec![disk.clone()], 0, 100) {
            Applied::Done(Some(_), _) => {}
            _ => panic!("a changed state must be sent"),
        }
        let v = capacity::view_entry(st.capacity.get("disk6"), &disk, 0, false);
        assert_eq!(v.state.as_str(), "repaired");
        assert!(matches!(apply_scan(&mut st, 0, vec![disk], 0, 101), Applied::Done(None, _)));
    }

    /// A result of the helper: a verdict with a confirmed restore and the same layout.
    fn probe_json(verdict: &str, real: u64) -> Value {
        json!({"verdict": verdict, "reported": DECLARED, "real": real, "points": 10, "badPoints": 2, "seconds": 1.0,
            "restoreFailures": 0, "restoreCheck": {"checked": 6000}, "flush": "ok", "layoutAfter": "same"})
    }

    fn finish(state: &Shared, disk: &disks::Disk, r: Result<Value, helper::RunError>) -> TaskResult {
        finish_probe(state, disk, &r, &[]).0
    }

    /// An error of the helper with its cause and details.
    fn helper_error(cause: &str, details: Value) -> Result<Value, helper::RunError> {
        Err(helper::RunError::Failed { message: format!("{cause}!"), cause: Some(cause.into()), details })
    }

    /// The knowledge of a drive our repair made smaller.
    fn repaired() -> (disks::Disk, capacity::Knowledge) {
        let size = capacity::loss_repair_size(8_305_000_000, DECLARED);
        let mut known = loss(8_305_000_000);
        known.apply_repair(capacity::Repair::Partition(size), 5);
        (test_disk(size), known)
    }

    fn knowledge(state: &Shared) -> capacity::Knowledge {
        lock(state).capacity["disk6"].knowledge.clone()
    }

    fn notice(state: &Shared) -> Option<Notice> {
        lock(state).notices.get("disk6").cloned()
    }

    fn replug(reason: &'static str) -> Notice {
        Notice { declared: DECLARED, fda: false, replug: Some(Replug { reason, counts: json!({}) }) }
    }

    #[test]
    fn a_declined_or_failed_quick_check_leaves_the_record_unchanged() {
        let disk = test_disk(8_000_000_000);
        let state = test_state(&disk, Some(loss(8_303_673_344)));
        let before = lock(&state).capacity["disk6"].knowledge.clone();
        let cases = [
            (Err(helper::RunError::Declined), "errDeclined"),
            (Err(helper::RunError::failed("boom")), "errHelper"),
            (helper_error("cancelled", json!({"layoutAfter": "same"})), "errCancelled"),
        ];
        for (error, key) in cases {
            let r = finish(&state, &disk, error);
            assert_eq!(r.unwrap_err().0, key);
            assert_eq!(lock(&state).capacity["disk6"].knowledge, before);
            assert_eq!(notice(&state), None, "{key}");
        }
        // an unreadable result changes nothing either
        assert_eq!(finish(&state, &disk, Ok(json!({"x": 1}))).unwrap_err().0, "errHelper");
        assert_eq!(lock(&state).capacity["disk6"].knowledge, before);
        // a good result does
        finish(&state, &disk, Ok(probe_json("fake", 5_000_000_000))).unwrap();
        assert_eq!(lock(&state).capacity["disk6"].knowledge.loss.as_ref().unwrap().real, 5_000_000_000);
    }

    #[test]
    fn a_transient_unreadable_layout_at_the_end_of_a_check_keeps_the_result() {
        // the check started on a drive with a readable layout; the last scan lost the partition table once
        let start = test_disk(8_000_000_000);
        let glitch = disks::Disk { partitions: Vec::new(), ..start.clone() };
        let state = test_state(&glitch, None);
        assert_eq!(latest(&state, &start), start, "the layout the task started with is kept");
        // a readable layout that changed (a remount) is taken as it is
        let remounted = disks::Disk {
            partitions: vec![disks::Partition { offset: MIB, size: 1, id: "N".into() }],
            ..start.clone()
        };
        let state = test_state(&remounted, None);
        assert_eq!(latest(&state, &start), remounted);
        // the result of a quick check is recorded under the readable layout, so the next scan confirms it
        let state = test_state(&glitch, None);
        finish(&state, &start, Ok(probe_json("fake", 5_000_000_000))).unwrap();
        let st = lock(&state);
        let v = capacity::view_entry(st.capacity.get("disk6"), &start, 0, false);
        assert_eq!(v.state.as_str(), "fake");
    }

    #[test]
    fn a_repair_request_for_another_layout_than_the_window_showed_is_refused() {
        let size = capacity::loss_repair_size(120_000_000_000, DECLARED);
        let disk = test_disk(size);
        let mut known = loss(120_000_000_000);
        known.apply_repair(capacity::Repair::Partition(size), 5);
        let state = test_state(&disk, Some(known));
        let other = disks::Disk {
            partitions: vec![disks::Partition { offset: MIB, size, id: "OTHER".into() }],
            ..disk.clone()
        };
        // the window showed the layout of another drive; none at all is refused too
        assert_eq!(plan_repair(&state, &disk, "limit", size, None, key(&other)).unwrap_err().0, "errRepairChanged");
        assert_eq!(plan_repair(&state, &disk, "limit", size, None, None).unwrap_err().0, "errRepairChanged");
        assert_eq!(plan_repair(&state, &disk, "limit", size, None, key(&disk)).unwrap().0, size);
    }

    #[test]
    fn incident_replay_changes_nothing() {
        let (disk, known) = repaired();
        let state = test_state(&disk, Some(known.clone()));
        // every operation of the coarse round failed, and afterwards the drive lists with no partitions
        let details = json!({"counts": {"ioErrors": 1882, "mismatches": 0, "unsaved": 1882, "restoreFailures": 0},
            "restoreCheck": {}, "layoutAfter": "tableGone"});
        let (key, args) = finish(&state, &disk, helper_error("unresponsive", details)).unwrap_err();
        assert_eq!(key, "errUnresponsive");
        assert_eq!(args, json!({"io": 1882, "mismatches": 0}));
        assert_eq!(knowledge(&state), known);
        let n = notice(&state).unwrap();
        let r = n.replug.as_ref().unwrap();
        assert_eq!((n.declared, n.fda, r.reason), (DECLARED, false, "unresponsive"));
        assert_eq!(r.counts["io"], 1882);
        assert_eq!(n.message()["kind"], "replug");
        assert_eq!(notices_of(&lock(&state))["disk6"]["counts"]["io"], 1882);
    }

    #[test]
    fn fake_with_unconfirmed_restore_is_not_recorded() {
        let (disk, known) = repaired();
        let state = test_state(&disk, Some(known.clone()));
        let mut v = probe_json("fake", 5_000_000_000);
        v["restoreCheck"] = json!({"checked": 6000, "mismatched": 1, "tableArea": true});
        let (key, args) = finish(&state, &disk, Ok(v)).unwrap_err();
        assert_eq!((key, args), ("errNotRestored", json!({"reason": "mismatch", "n": 1})));
        assert_eq!(knowledge(&state), known);
        assert_eq!(notice(&state).unwrap().replug.unwrap().reason, "mismatch");
    }

    #[test]
    fn every_untrusted_result_names_its_reason() {
        let cases: [(&str, Value, &str); 8] = [
            ("restoreFailures", json!(2), "restoreFailed"),
            ("layoutAfter", json!("tableGone"), "tableGone"),
            ("restoreCheck", json!({"mismatched": 2, "unreadable": 3}), "mismatch"),
            ("restoreCheck", json!({"unreadable": 3}), "unreadable"),
            ("layoutAfter", json!("changed"), "layoutChanged"),
            ("layoutAfter", json!("driveGone"), "driveGone"),
            ("layoutAfter", json!("unreadable"), "layoutUnknown"),
            ("layoutAfter", Value::Null, "layoutUnknown"),
        ];
        for (field, value, reason) in cases {
            let (disk, known) = repaired();
            let state = test_state(&disk, Some(known.clone()));
            let mut v = probe_json("genuine", DECLARED);
            v[field] = value;
            assert_eq!(finish(&state, &disk, Ok(v)).unwrap_err().0, "errNotRestored", "{reason}");
            assert_eq!(knowledge(&state), known, "{reason}");
            assert_eq!(notice(&state).unwrap().replug.unwrap().reason, reason);
        }
        // the most telling reason wins
        let all = replug_reason(
            true,
            Counts { restore_failures: 1, ..Default::default() },
            &probe::RestoreCheck { mismatched: 1, ..Default::default() },
            Some(helper::LayoutAfter::TableGone),
        );
        assert_eq!(all.unwrap().reason, "unresponsive");
    }

    #[test]
    fn a_helper_that_died_after_the_baseline_asks_for_a_replug() {
        let disk = test_disk(8_000_000_000);
        let line = |stage: &str| json!({"t_ms": 1, "stage": stage, "ok": true, "info": {}}).to_string();
        let died = || Err(helper::RunError::failed("the helper stopped without a result"));
        // it may have written and nobody checked the restore
        let state = test_state(&disk, None);
        assert_eq!(finish_probe(&state, &disk, &died(), &[line("baseline")]).0.unwrap_err().0, "errHelper");
        assert_eq!(notice(&state).unwrap().replug.unwrap().reason, "layoutUnknown");
        // before the baseline nothing was written
        let state = test_state(&disk, None);
        let _ = finish_probe(&state, &disk, &died(), &[line("found"), line("open")]);
        assert_eq!(notice(&state), None);
    }

    #[test]
    fn trusted_fake_records_counts() {
        let disk = test_disk(8_000_000_000);
        let state = test_state(&disk, None);
        let mut v = probe_json("fake", 5_000_000_000);
        v["ioErrors"] = json!(1700);
        v["mismatches"] = json!(50);
        v["unsaved"] = json!(1650);
        let ok = finish(&state, &disk, Ok(v)).unwrap();
        let last = knowledge(&state).last.unwrap();
        assert_eq!((last.io_errors, last.mismatches, last.unsaved, last.restore_checked), (1700, 50, 1650, true));
        assert_eq!(ok["check"]["ioErrors"], 1700);
        assert_eq!(ok["check"]["restoreChecked"], true);
        assert_eq!(notice(&state), None);
    }

    #[test]
    fn privacy_cause_sets_the_fda_notice() {
        let (disk, known) = repaired();
        let state = test_state(&disk, Some(known.clone()));
        let details = json!({"restoreCheck": {}, "layoutAfter": "same",
            "access": {"cause": "privacy", "op": "open", "errno": 1}});
        assert_eq!(finish(&state, &disk, helper_error("privacy", details)).unwrap_err().0, "errFullDiskAccess");
        let n = notice(&state).unwrap();
        assert_eq!((n.fda, n.replug.is_none()), (true, true));
        assert_eq!(n.message()["kind"], "fullDiskAccess");
        assert_eq!(knowledge(&state), known);
        // with a restore nobody could confirm, both messages are due
        let state = test_state(&disk, None);
        let details = json!({"restoreCheck": {"unreadable": 5}, "layoutAfter": "same"});
        let _ = finish(&state, &disk, helper_error("privacy", details));
        let n = notice(&state).unwrap();
        assert_eq!((n.fda, n.replug.as_ref().map(|r| r.reason)), (true, Some("unreadable")));
        assert_eq!((n.message()["kind"].clone(), n.message()["fda"].clone()), (json!("replug"), json!(true)));
    }

    #[test]
    fn busy_is_not_fda() {
        let disk = test_disk(8_000_000_000);
        for (cause, key) in [
            ("busy", "errAccessBusy"),
            ("readOnly", "errAccessReadOnly"),
            ("gone", "errDriveGone"),
            ("denied", "errAccess"),
        ] {
            let state = test_state(&disk, None);
            let details = json!({"restoreCheck": {}, "layoutAfter": "same"});
            let (got, args) = finish(&state, &disk, helper_error(cause, details)).unwrap_err();
            assert_eq!(got, key);
            if key == "errAccess" {
                assert_eq!(args["err"], "denied!");
            }
            assert_eq!(notice(&state), None, "{cause}");
        }
    }

    #[test]
    fn replug_notice_blocks_tasks() {
        let disk = test_disk(8_000_000_000);
        let state = test_state(&disk, None);
        let mut st = lock(&state);
        assert_eq!(may_start(&st, &disk), Ok(()));
        st.notices.insert("disk6".into(), replug("unresponsive"));
        assert_eq!(may_start(&st, &disk), Err("errReplugFirst"));
        assert_eq!(admit(&mut st, &disk), Err("errReplugFirst"));
        assert!(st.notices.contains_key("disk6"), "a refused task leaves the notice");
        // another drive is free, and so is a drive of another size under the same id
        let other = disks::Disk { id: "disk7".into(), ..disk.clone() };
        assert_eq!(may_start(&st, &other), Ok(()));
        let smaller = disks::Disk { size: 1_000, ..disk.clone() };
        assert_eq!(may_start(&st, &smaller), Ok(()));
    }

    #[test]
    fn replug_notice_survives_a_failed_scan_and_ends_on_a_good_one() {
        let disk = test_disk(8_000_000_000);
        let state = test_state(&disk, None);
        let mut st = lock(&state);
        st.notices.insert("disk6".into(), replug("tableGone"));
        // a failed scan changes no state; it only answers a Reload press
        assert!(scan_reply(1, Err("diskutil did not answer")).is_some());
        // a scan that overlapped a task is not applied
        st.task_gen = 1;
        assert!(matches!(apply_scan(&mut st, 0, Vec::new(), 0, 1), Applied::Stale));
        assert!(st.notices.contains_key("disk6"));
        // the drive still listed, now without partitions
        let blank = disks::Disk { partitions: Vec::new(), ..disk.clone() };
        apply_scan(&mut st, 1, vec![blank], 0, 2);
        assert!(st.notices.contains_key("disk6"));
        // unplugged: gone from a successful scan; it comes back under another id and is free
        apply_scan(&mut st, 1, Vec::new(), 0, 3);
        assert!(st.notices.is_empty());
        let back = disks::Disk { id: "disk8".into(), ..disk.clone() };
        apply_scan(&mut st, 1, vec![back.clone()], 0, 4);
        assert_eq!(may_start(&st, &back), Ok(()));
        // another drive under the same id ends it too
        st.notices.insert("disk8".into(), replug("tableGone"));
        apply_scan(&mut st, 1, vec![disks::Disk { size: 1_000, ..back }], 0, 5);
        assert!(st.notices.is_empty());
    }

    #[test]
    fn fda_notice_ends_when_the_next_task_starts() {
        let disk = test_disk(8_000_000_000);
        let state = test_state(&disk, None);
        let mut st = lock(&state);
        st.notices.insert("disk6".into(), Notice { declared: DECLARED, fda: true, replug: None });
        // it stays through scans
        apply_scan(&mut st, 0, vec![disk.clone()], 0, 1);
        assert!(st.notices.contains_key("disk6"));
        assert_eq!(admit(&mut st, &disk), Ok(true));
        assert!(st.notices.is_empty());
        assert_eq!(admit(&mut st, &disk), Ok(false));
    }

    #[test]
    fn repair_refused_while_replug_pending() {
        let (disk, known) = repaired();
        let size = known.repair.as_ref().unwrap().partition;
        let state = test_state(&disk, Some(known));
        assert!(plan_repair(&state, &disk, "limit", size, None, key(&disk)).is_ok());
        lock(&state).notices.insert("disk6".into(), replug("tableGone"));
        let shown = capacity::Shown { state: "repaired", limit: Some(size) };
        for (plan, size, explicit, layout) in [
            ("limit", size, None, key(&disk)),
            ("whole", DECLARED, Some(shown), key(&disk)),
            ("nonsense", 0, None, None),
        ] {
            let r = plan_repair(&state, &disk, plan, size, explicit, layout);
            assert_eq!(r.unwrap_err().0, "errReplugFirst", "{plan}");
        }
    }

    #[test]
    fn reveal_report_ignores_unknown_disks() {
        let disk = test_disk(8_000_000_000);
        let state = test_state(&disk, None);
        let mut st = lock(&state);
        let path = PathBuf::from("/tmp/report.json");
        assert_eq!(report_to_reveal(&st, Some("disk6")), None);
        st.reports.insert("disk6".into(), (DECLARED, path.clone()));
        st.reports.insert("disk9".into(), (DECLARED, path.clone()));
        assert_eq!(report_to_reveal(&st, Some("disk6")), Some(path));
        // not connected, a path instead of an id, nothing at all
        for id in [Some("disk9"), Some("/etc/passwd"), None] {
            assert_eq!(report_to_reveal(&st, id), None, "{id:?}");
        }
        assert_eq!(reports_of(&st).keys().collect::<Vec<_>>(), ["disk6"]);
        // another drive under this id now
        st.disks[0].size = 1_000;
        assert_eq!(report_to_reveal(&st, Some("disk6")), None);
        assert!(reports_of(&st).is_empty());
    }

    #[test]
    fn report_written_for_a_declined_probe() {
        let disk = test_disk(8_000_000_000);
        let state = test_state(&disk, None);
        let logs = std::env::temp_dir().join(format!("umc_main_declined_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&logs);
        let ran = helper::Ran { result: Err(helper::RunError::Declined), log: Vec::new() };
        let (result, judged) = finish_probe(&state, &disk, &ran.result, &ran.log);
        assert_eq!(result.unwrap_err().0, "errDeclined");
        report_probe(&state, &disk, &logs, settings::now(), &ran, &judged);
        let files: Vec<_> = std::fs::read_dir(&logs).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(files.len(), 1);
        let v: Value = serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
        assert_eq!(v["outcome"]["key"], "errDeclined");
        assert_eq!(v["kind"], "probe");
        assert_eq!(v["stages"], json!([]));
        assert_eq!(report_to_reveal(&lock(&state), Some("disk6")).as_ref(), Some(&files[0]));
        // a folder that cannot be made only prints an error: the link keeps pointing at the last written report
        std::fs::write(logs.join("file"), "").unwrap();
        report_probe(&state, &disk, &logs.join("file/checks"), settings::now(), &ran, &judged);
        assert_eq!(report_to_reveal(&lock(&state), Some("disk6")).as_ref(), Some(&files[0]));
        std::fs::remove_dir_all(&logs).unwrap();
    }

    #[test]
    fn no_test_store_has_a_path() {
        // a store made by a test never writes: nothing in the test suite touches the real config folder
        let disk = test_disk(8_000_000_000);
        let state = test_state(&disk, Some(loss(8_303_673_344)));
        assert!(lock(&state).store.save_job().is_none());
    }

    #[test]
    fn a_full_check_that_wrote_almost_nothing_is_an_error_and_changes_nothing() {
        let disk = disks::Disk {
            partitions: vec![disks::Partition { offset: MIB, size: 8_303_673_344, id: "U".into() }],
            ..test_disk(0)
        };
        let state = test_state(&disk, None);
        let r = finish_full(&state, &disk, Ok(outcome(0, Some("Input/output error"), 512 * 1024)));
        let (key, args) = r.unwrap_err();
        assert_eq!(key, "errIo");
        assert!(args["err"].as_str().unwrap().contains("Input/output"));
        assert!(lock(&state).capacity.is_empty(), "no record was made");
        // the plan of the drive is what the layout says: the size of its partition
        let st = lock(&state);
        let v = capacity::view_entry(st.capacity.get("disk6"), &disk, 0, false);
        assert_eq!((v.repair.default, v.repair.limit), (capacity::PlanDefault::Limit, Some(8_303_673_344)));
    }
}
