//! UsbMemoryChecker — checks USB flash drives and memory cards: read/write speed, real capacity (finds fake
//! drives that report more than they hold) and repairs a fake drive so it shows only its real size.
//!
//! Window: tao + wry (system WebView), page in `ui/index.html`. Threads: the WebView lives only in the main
//! thread, so everything sent to the window goes as `UserEvent::Js` through the `EventLoopProxy`. One test runs
//! at a time (`State::task`) in its own thread; the quick probe and the repair run in an elevated copy of the
//! program (`helper`).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bench;
mod cli;
mod dio;
mod disks;
mod donations;
mod fullcheck;
mod helper;
mod i18n;
mod net;
mod pattern;
mod probe;
mod rawdev;
mod settings;
mod update;

use std::collections::HashMap;
use std::path::PathBuf;
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

/// Result of the last capacity check of a drive: what the repair needs.
#[derive(Clone)]
struct Capacity {
    reported: u64,
    real: u64,
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
    cancel: Arc<AtomicBool>,
    capacity: HashMap<String, Capacity>,
    /// Ask the disk watcher to look now.
    rescan: Sender<()>,
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
    let (rescan_tx, rescan_rx) = channel::<()>();
    let state: Shared = Arc::new(Mutex::new(State {
        lang: i18n::resolve(&settings.language),
        settings,
        donations: donations::cached(),
        donations_loading: false,
        updates: UpdateState::default(),
        disks: Vec::new(),
        scanned: false,
        task: None,
        cancel: Arc::new(AtomicBool::new(false)),
        capacity: HashMap::new(),
        rescan: rescan_tx,
    }));
    {
        let (state, ui) = (Arc::clone(&state), ui.clone());
        std::thread::spawn(move || disk_watcher(rescan_rx, state, ui));
    }

    #[allow(unused_mut)]
    let mut builder = WindowBuilder::new()
        .with_title("UsbMemoryChecker")
        .with_inner_size(tao::dpi::LogicalSize::new(1180.0, 800.0))
        .with_min_inner_size(tao::dpi::LogicalSize::new(880.0, 620.0));
    #[cfg(target_os = "macos")]
    {
        use tao::platform::macos::WindowBuilderExtMacOS;
        builder = builder.with_titlebar_transparent(true).with_fullsize_content_view(true).with_title_hidden(true);
    }
    let window = builder.build(&event_loop)?;

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
            ui.send(json!({"type": "disks", "disks": st.disks, "scanned": st.scanned}));
            if let Some(t) = &st.task {
                ui.send(json!({"type": "task", "kind": t.kind, "disk": t.disk, "fraction": 0.0, "stage": "prStart"}));
            }
            let auto = st.settings.auto_update
                && update::install_target().is_some()
                && settings::now().saturating_sub(st.settings.last_update_check) >= 86_400;
            let _ = st.rescan.send(());
            drop(st);
            if auto {
                check_updates(state, ui, false);
            }
        }
        "rescan" => {
            let _ = lock(state).rescan.send(());
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
        "fix" => {
            let (fs, label) = (req.fs.unwrap_or(helper::Fs::Exfat), req.label.unwrap_or_default());
            start_task(state, ui, "fix", req.disk, move |ctx| run_fix(ctx, fs, label));
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

fn disk_watcher(rx: Receiver<()>, state: Shared, ui: Ui) {
    let every = Duration::from_secs(if cfg!(windows) { 4 } else { 2 });
    loop {
        match disks::list() {
            Ok(list) => {
                let mut st = lock(&state);
                if st.disks != list || !st.scanned {
                    st.scanned = true;
                    // results of a drive that is gone or changed size are no longer valid
                    st.capacity.retain(|id, c| list.iter().any(|d| &d.id == id && d.size == c.reported));
                    st.disks = list;
                    ui.send(json!({"type": "disks", "disks": st.disks, "scanned": true}));
                }
            }
            Err(e) => eprintln!("drive list: {e}"),
        }
        match rx.recv_timeout(every) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        }
        while rx.try_recv().is_ok() {}
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
    st.task = Some(Task { kind, disk: disk.id.clone() });
    st.cancel = Arc::new(AtomicBool::new(false));
    let cancel = Arc::clone(&st.cancel);
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
        let result = work(&mut ctx);
        let cancelled = ctx.cancel.load(Ordering::Relaxed);
        let mut st = lock(&state);
        st.task = None;
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
        let _ = st.rescan.send(());
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
    let mount = volume_path(ctx)?;
    let cancel = Arc::clone(&ctx.cancel);
    let r = fullcheck::run(&mount, None, &cancel, &mut |p| {
        let x = if p.total == 0 { 0.0 } else { p.done as f64 / p.total as f64 };
        let fraction = if p.op == "write" { x * 0.5 } else { 0.5 + x * 0.5 };
        if let Some(mbps) = p.mbps {
            ctx.point(p.op, x, mbps);
        }
        let stage = if p.op == "write" { "prFill" } else { "prVerify" };
        ctx.progress(fraction, stage, json!({"done": p.done, "total": p.total, "mbps": p.mbps}));
    });
    match r {
        Ok(o) => {
            let disk = &ctx.disk;
            // a bad block means the drive loses data: the estimate becomes the size for the repair
            if o.bad > 0 {
                let real = o.real_estimate.min(disk.size) / 100 * 99;
                lock(&ctx.state).capacity.insert(disk.id.clone(), Capacity { reported: disk.size, real });
            }
            Ok(json!({"full": o, "reported": disk.size}))
        }
        Err(fullcheck::Error::Cancelled) => Err(("errCancelled", json!({}))),
        Err(fullcheck::Error::NoSpace(free)) => Err(("errNoSpaceFull", json!({"free": free}))),
        Err(fullcheck::Error::Io(e)) => Err(("errIo", json!({"err": e}))),
    }
}

fn elevated(ctx: &mut TaskCtx, job: &helper::Job) -> TaskResult {
    let prompt = i18n::admin_prompt(ctx.lang());
    let cancel = Arc::clone(&ctx.cancel);
    match helper::run(job, prompt, &cancel, &mut |f, stage| ctx.progress(f, &stage, json!({}))) {
        Ok(v) => Ok(v),
        Err(helper::RunError::Declined) => Err(("errDeclined", json!({}))),
        Err(helper::RunError::Failed(e)) => Err(("errHelper", json!({"err": e}))),
    }
}

fn run_probe(ctx: &mut TaskCtx) -> TaskResult {
    let job = helper::Job {
        action: helper::Action::Probe,
        disk: ctx.disk.id.clone(),
        size: ctx.disk.size,
        real: 0,
        fs: helper::Fs::Exfat,
        label: String::new(),
    };
    let v = elevated(ctx, &job)?;
    let outcome: probe::Outcome = serde_json::from_value(v)
        .map_err(|e| ("errHelper", json!({"err": format!("unreadable probe result: {e}")})))?;
    let mut st = lock(&ctx.state);
    if outcome.verdict == probe::Verdict::Genuine {
        st.capacity.remove(&ctx.disk.id);
    } else {
        st.capacity.insert(ctx.disk.id.clone(), Capacity { reported: ctx.disk.size, real: outcome.real });
    }
    Ok(json!({"probe": outcome}))
}

fn run_fix(ctx: &mut TaskCtx, fs: helper::Fs, label: String) -> TaskResult {
    let cap = lock(&ctx.state).capacity.get(&ctx.disk.id).cloned().filter(|c| c.reported == ctx.disk.size);
    // without a check result the drive is formatted at full size
    let real = cap.as_ref().map(|c| c.real).unwrap_or(0);
    let job =
        helper::Job { action: helper::Action::Fix, disk: ctx.disk.id.clone(), size: ctx.disk.size, real, fs, label };
    if real == 0 && cap.is_some() {
        return Err(("errBroken", json!({})));
    }
    elevated(ctx, &job)?;
    lock(&ctx.state).capacity.remove(&ctx.disk.id);
    Ok(json!({"size": helper::partition_size(real, ctx.disk.size)}))
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
