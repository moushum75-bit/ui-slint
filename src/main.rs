//! Grid frequency monitor UI.
//!
//! Threads:
//!   * main thread  - runs the Slint event loop (all drawing happens here)
//!   * socket thread - connects to the backend, parses JSON lines, updates the
//!                     shared State and hands finished strings to the UI with
//!                     upgrade_in_event_loop().
//! Slint objects are not thread-safe, so the socket thread never touches the
//! window directly; it only sends closures to the event loop.

mod cal;
mod hist;
mod notes;
mod plot_ui;
mod settings_ui;
mod protocol;
mod readout;
mod stats;
mod trend;
mod upload_ui;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use slint::{ModelRc, SharedString, VecModel};

use protocol::{Limits, Msg};
use trend::Trend;

slint::include_modules!(); // AppWindow, Tick, Note, StatTile from ui/app.slint

const WINDOWS: [i64; 5] = [0, 600, 3600, 21_600, 86_400]; // chip index -> seconds (0 = knob)
const MIN_SPAN: i64 = 60; // the Knob sliders keep at least a minute between start and end
const DAY: i64 = 86_400;
const COMPARE: [&str; 3] = ["yesterday", "avg3", "forecast"]; // compare buttons 1..3 -> overlay kind

struct State {
    trend: Trend,
    limits: Limits,
    tz: Option<i64>,
    choice: usize,
    knob_start: i64, // Knob chip: seconds before now where the window starts ...
    knob_end: i64,   // ... and where it ends (0 = now, follows live)
    notes: notes::Notes,
    compare: [bool; 3],                       // on/off: yesterday, 3-day, forecast
    overlays: [Option<trend::Overlay>; 3],    // the last curve of each kind
    overlay_asked: i64,               // epoch of the last overlay request
}

impl State {
    fn window(&self) -> i64 {
        if self.choice == 0 {
            (self.knob_start - self.knob_end).max(MIN_SPAN)
        } else {
            WINDOWS[self.choice]
        }
    }

    /// How far the right edge of the plot lies in the past (only the Knob chip can move it).
    fn end_back(&self) -> i64 {
        if self.choice == 0 { self.knob_end } else { 0 }
    }
}

/// Shared between threads. Mutex for the big state, atomics for tiny flags.
struct Shared {
    state: Mutex<State>,
    last_live_ms: AtomicI64,
    connected: AtomicBool,
    writer: Mutex<Option<UnixStream>>, // write half of the socket, for commands
}

/// Send one JSON command line to the backend (ignored while disconnected).
fn send_cmd(sh: &Shared, v: serde_json::Value) {
    if let Some(w) = sh.writer.lock().unwrap().as_mut() {
        let _ = w.write_all(format!("{v}\n").as_bytes());
    }
}

/// Ask the backend for every comparison curve that is switched on:
/// window past + window ahead, one request per kind.
fn request_overlay(sh: &Shared) {
    let (on, start, end) = {
        let mut st = sh.state.lock().unwrap();
        if !st.compare.iter().any(|&c| c) {
            return;
        }
        let now = if st.trend.end() > 0 { st.trend.end() } else { now_ms() / 1000 };
        let win = st.window();
        st.overlay_asked = now;
        (st.compare, now - win, now + win)
    };
    for (k, kind) in COMPARE.iter().enumerate() {
        if on[k] {
            send_cmd(sh, serde_json::json!({"cmd": "overlay", "kind": kind, "start": start, "end": end, "buckets": 300}));
        }
    }
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn s(x: impl AsRef<str>) -> SharedString {
    SharedString::from(x.as_ref())
}

// ------------------------------------------------------------ UI updates --

fn apply_limits(ui: &AppWindow, l: &Limits) {
    ui.set_lim_el(l.emergency_low);
    ui.set_lim_wl(l.warning_low);
    ui.set_lim_al(l.alert_low);
    ui.set_lim_ah(l.alert_high);
    ui.set_lim_wh(l.warning_high);
    ui.set_lim_eh(l.emergency_high);
    let (lo, hi) = readout::meter_range(l);
    ui.set_meter_lo(lo);
    ui.set_meter_hi(hi);
}

/// Big number. `None` = no data (dashes).
fn apply_readout(ui: &AppWindow, freq: Option<f32>) {
    let d: Vec<SharedString> = readout::digits(freq).into_iter().map(SharedString::from).collect();
    ui.set_digits(ModelRc::new(VecModel::from(d)));
    if let Some(f) = freq {
        ui.set_freq(f);
    }
}

fn apply_trend(ui: &AppWindow, v: trend::View, label: String) {
    ui.set_t_line(s(&v.line));
    ui.set_t_area(s(&v.area));
    ui.set_t_grid(s(&v.grid));
    ui.set_t_alert(s(&v.alert));
    ui.set_t_warning(s(&v.warning));
    ui.set_t_emergency(s(&v.emergency));
    ui.set_t_ov_yday(s(&v.ov_lines[0]));
    ui.set_t_ov_avg3(s(&v.ov_lines[1]));
    ui.set_t_ov_fc(s(&v.ov_lines[2]));
    ui.set_t_ov_band(s(&v.ov_band));
    ui.set_t_now_pos(v.now_pos);
    let tick = |v: Vec<(f32, String)>| -> ModelRc<Tick> {
        ModelRc::new(VecModel::from(
            v.into_iter().map(|(pos, t)| Tick { pos, text: s(t) }).collect::<Vec<_>>(),
        ))
    };
    ui.set_t_xticks(tick(v.xticks));
    ui.set_t_yticks(tick(v.yticks));
    ui.set_window_text(s(label));
}

/// Slider position (0..1) <-> seconds before now. The curve is cubic: fine steps near
/// "now" (a minute is easy to hit), the whole 24 h still fits on the slider.
fn back_of(u: f32) -> i64 {
    (DAY as f32 * u.clamp(0.0, 1.0).powi(3)).round() as i64
}

fn u_of(back: i64) -> f32 {
    (back.clamp(0, DAY) as f32 / DAY as f32).cbrt()
}

/// "now", "-45 s", "-12 min", "-2 h 15 m"
fn back_text(back: i64) -> String {
    match back {
        0 => "now".into(),
        1..=89 => format!("-{back} s"),
        90..=3599 => format!("-{} min", (back + 30) / 60),
        _ => format!("-{} h {:02} m", back / 3600, (back % 3600) / 60),
    }
}

/// Put the two Knob slider positions and their labels into the UI.
fn apply_knob(ui: &AppWindow, sh: &Shared) {
    let (a, b) = {
        let st = sh.state.lock().unwrap();
        (st.knob_start, st.knob_end)
    };
    ui.set_knob_start(u_of(a));
    ui.set_knob_end(u_of(b));
    ui.set_knob_start_text(s(back_text(a)));
    ui.set_knob_end_text(s(back_text(b)));
}

/// Redraw the trend. Runs on the UI thread because it needs the plot size
/// in pixels; 86 400 samples -> 700 columns takes about a millisecond.
fn refresh_trend(ui: &AppWindow, sh: &Shared) {
    let (w, h) = (ui.get_trend_w(), ui.get_trend_h());
    let (view, label) = {
        let st = sh.state.lock().unwrap();
        let live_end = if st.trend.end() > 0 { st.trend.end() } else { now_ms() / 1000 };
        let back = st.end_back();
        let end = live_end - back;
        let win = st.window();
        // the comparison curves are about "now": only drawn while the window ends at now
        let any = st.compare.iter().any(|&c| c) && back == 0;
        let ahead = if any { win } else { 0 };
        let ovs = [0, 1, 2].map(|k| if st.compare[k] && back == 0 { st.overlays[k].as_ref() } else { None });
        let mut label = trend::window_label(win);
        if back > 0 {
            label = format!("{label}, ended {} ago", back_text(back).trim_start_matches('-'));
        }
        (st.trend.view(end, win, ahead, ovs, &st.limits, st.tz.unwrap_or(0), w, h), label)
    };
    apply_trend(ui, view, label);
}

/// Copy the alerts list into the UI (only called when it changed).
fn apply_notes(ui: &AppWindow, sh: &Shared) {
    let v: Vec<Note> = {
        let st = sh.state.lock().unwrap();
        st.notes.list.iter().map(|e| Note { time: s(&e.time), level: e.level, text: s(&e.text) }).collect()
    };
    ui.set_notes(ModelRc::new(VecModel::from(v)));
}

/// Local wall-clock "HH:MM:SS" for events that do not come with a time.
fn clock_now(sh: &Shared) -> String {
    let tz = sh.state.lock().unwrap().tz.unwrap_or(0);
    clock_texts(now_ms() / 1000, tz).0
}

/// Run `f` on the notes, and if it added anything, show the new list.
fn note(sh: &Arc<Shared>, ui: &slint::Weak<AppWindow>, f: impl FnOnce(&mut notes::Notes)) {
    let changed = {
        let mut st = sh.state.lock().unwrap();
        let before = st.notes.rev;
        f(&mut st.notes);
        st.notes.rev != before
    };
    if changed {
        let sh = sh.clone();
        let _ = ui.upgrade_in_event_loop(move |ui| apply_notes(&ui, &sh));
    }
}

/// "HIGH WARNING" -> "High warning" (calmer than all caps)
fn title_case(t: &str) -> String {
    let lower = t.to_lowercase();
    let mut c = lower.chars();
    match c.next() {
        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

// ------------------------------------------------------------ socket side --

fn handle(msg: Msg, sh: &Arc<Shared>, ui: &slint::Weak<AppWindow>) {
    match msg {
        Msg::Hello { tz_offset, db } => {
            sh.state.lock().unwrap().tz = Some(tz_offset);
            if let Some(db) = db {
                plot_ui::set_db_path(db); // where the history plots read from
            }
        }
        Msg::Limits { limits } => {
            sh.state.lock().unwrap().limits = limits;
            let sh = sh.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| {
                settings_ui::on_limits(&limits);
                apply_limits(&ui, &limits);
                refresh_trend(&ui, &sh);
            });
        }
        Msg::Backfill { end, values } => {
            sh.state.lock().unwrap().trend.backfill(end, &values);
            let sh = sh.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| refresh_trend(&ui, &sh));
        }
        Msg::Stats { today, yesterday } => {
            let Some(today) = today else { return };
            let lim = sh.state.lock().unwrap().limits;
            let tiles = stats::tiles(&today, yesterday.as_ref(), &lim);
            let _ = ui.upgrade_in_event_loop(move |ui| {
                let v: Vec<StatTile> = tiles
                    .into_iter()
                    .map(|t| StatTile { label: s(t.label), value: s(t.value), unit: s(t.unit), sub: s(t.sub), level: t.level })
                    .collect();
                ui.set_stats(ModelRc::new(VecModel::from(v)));
            });
        }
        Msg::Fault { text } => {
            let t = clock_now(sh);
            note(sh, ui, |n| n.fault(&t, &text));
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_fault_text(s(text));
                ui.set_serial_ok(false);
            });
        }
        Msg::Live(l) => {
            sh.last_live_ms.store(now_ms(), Ordering::Relaxed);
            {
                let mut st = sh.state.lock().unwrap();
                st.trend.push(l.epoch, l.freq);
            }
            let status = title_case(&l.status);
            note(sh, ui, |n| n.live(l.epoch, &l.time, l.freq, l.level, &status, l.dfdt, l.rocof));
            let sh = sh.clone();
            let fmt = |v: Option<f32>| v.map(|f| format!("{f:.3}")).unwrap_or_else(|| "—".into());
            let _ = ui.upgrade_in_event_loop(move |ui| {
                ui.set_fault_text(s(""));
                ui.set_serial_ok(true);
                ui.set_has_data(true);
                ui.set_temp_text(s(l.temp.map(|t| format!("{t:.0}°C")).unwrap_or_else(|| "--".into())));
                ui.set_fan_on(l.fan);
                ui.set_net_ok(l.net);
                ui.set_sim(l.sim);
                if ui.get_hold() {
                    return; // HOLD: keep recording, but freeze what is shown
                }
                apply_readout(&ui, Some(l.freq));
                ui.set_dev_text(s(format!("{:+.3} Hz", l.freq - 50.0)));
                ui.set_status_text(s(title_case(&l.status)));
                ui.set_level(l.level);
                ui.set_dfdt(l.dfdt);
                ui.set_rocof_text(s(format!("{:+.3}", l.rocof)));
                ui.set_min_text(s(fmt(l.min)));
                ui.set_min_time(s(l.min_time.as_deref().map(|t| format!("at {t}")).unwrap_or_default()));
                ui.set_max_text(s(fmt(l.max)));
                ui.set_max_time(s(l.max_time.as_deref().map(|t| format!("at {t}")).unwrap_or_default()));
                ui.set_avg_text(s(fmt(l.avg)));
                refresh_trend(&ui, &sh);
            });
        }
        Msg::Button { idx } => {
            let _ = ui.upgrade_in_event_loop(move |ui| match idx {
                0 => settings_ui::open_password(&ui), // was the password / settings button
                1 => ui.set_screen_off(!ui.get_screen_off()), // was the screensaver button
                _ => ui.set_toast_text(s(format!("Button {idx}: screen comes in the next step"))),
            });
        }
        Msg::Overlay { kind, start, end, mid, lo, hi } => {
            {
                let mut st = sh.state.lock().unwrap();
                let Some(k) = COMPARE.iter().position(|c| *c == kind) else { return };
                if !st.compare[k] {
                    return; // an old answer: that button was switched off meanwhile
                }
                st.overlays[k] = Some(trend::Overlay { start, end, mid, lo, hi, future_only: kind == "forecast" });
            }
            let sh = sh.clone();
            let _ = ui.upgrade_in_event_loop(move |ui| refresh_trend(&ui, &sh));
        }
        Msg::Home => {
            let _ = ui.upgrade_in_event_loop(|ui| ui.set_drawer_open(false));
        }
        Msg::Sound { on, levels } => {
            let _ = ui.upgrade_in_event_loop(move |_| settings_ui::on_sound(on, &levels));
        }
        Msg::Reply { cmd, ok, text } => {
            let _ = ui.upgrade_in_event_loop(move |ui| {
                settings_ui::on_reply(&ui, cmd.as_deref().unwrap_or(""), ok, text.as_deref().unwrap_or(""))
            });
        }
        Msg::UploadResult { status, text } => {
            let _ = ui.upgrade_in_event_loop(move |ui| upload_ui::show_result(&ui, &status, &text));
        }
        Msg::Other => {}
    }
}

fn socket_thread(sh: Arc<Shared>, ui: slint::Weak<AppWindow>) {
    let path = std::env::var("FREQMON_SOCK").unwrap_or_else(|_| "/tmp/freqmon.sock".into());
    loop {
        match UnixStream::connect(&path) {
            Ok(stream) => {
                println!("connected to {path}");
                sh.connected.store(true, Ordering::Relaxed);
                *sh.writer.lock().unwrap() = stream.try_clone().ok();
                request_overlay(&sh); // after a reconnect, if a compare view is on
                let t = clock_now(&sh);
                note(&sh, &ui, |n| n.connected(&t));
                let _ = ui.upgrade_in_event_loop(|ui| ui.set_connected(true));
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    if let Some(msg) = protocol::parse_line(&line) {
                        handle(msg, &sh, &ui);
                    }
                }
                println!("backend closed the connection");
                *sh.writer.lock().unwrap() = None;
                sh.connected.store(false, Ordering::Relaxed);
                let t = clock_now(&sh);
                note(&sh, &ui, |n| n.disconnected(&t));
                let _ = ui.upgrade_in_event_loop(|ui| {
                    ui.set_connected(false);
                    ui.set_has_data(false);
                });
            }
            Err(_) => {} // backend not running yet: try again
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

// ------------------------------------------------------------------ clock --

/// Days since 1970-01-01 -> (year, month, day). Howard Hinnant's algorithm,
/// so we don't need a date crate.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// Today's local date (year, month, day), from the backend's time zone.
fn today_ymd(sh: &Shared) -> (i64, u32, u32) {
    civil_from_days(local_secs(sh).div_euclid(86_400))
}

/// Wall-clock seconds now (seconds since 1970-01-01 00:00:00 local time).
fn local_secs(sh: &Shared) -> i64 {
    let tz = sh.state.lock().unwrap().tz.unwrap_or(0);
    now_ms() / 1000 + tz
}

fn clock_texts(epoch: i64, tz: i64) -> (String, String) {
    const DAYS: [&str; 7] = ["THU", "FRI", "SAT", "SUN", "MON", "TUE", "WED"];
    const MONTHS: [&str; 12] =
        ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
    let t = epoch + tz;
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (
        format!("{:02}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60),
        format!("{} {:02} {} {}", DAYS[days.rem_euclid(7) as usize], d, MONTHS[(m - 1) as usize], y),
    )
}

// ------------------------------------------------------------------- main --

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    if std::env::var("FREQMON_FULLSCREEN").is_ok_and(|v| v == "1") {
        ui.window().set_fullscreen(true);
    }

    let sh = Arc::new(Shared {
        state: Mutex::new(State {
            trend: Trend::new(),
            limits: Limits::default(),
            tz: None,
            choice: 1,
            knob_start: 600,
            knob_end: 0,
            notes: notes::Notes::new(),
            compare: [false; 3],
            overlays: [None, None, None],
            overlay_asked: 0,
        }),
        last_live_ms: AtomicI64::new(0),
        connected: AtomicBool::new(false),
        writer: Mutex::new(None),
    });

    apply_limits(&ui, &Limits::default());
    apply_knob(&ui, &sh);
    apply_readout(&ui, None);

    // drawer actions: Upload is built, the others are not yet
    ui.on_action({
        let weak = ui.as_weak();
        let sh = sh.clone();
        move |what| {
            if let Some(ui) = weak.upgrade() {
                ui.set_drawer_open(false);
                if what == "upload" {
                    upload_ui::open(&ui, today_ymd(&sh));
                    return;
                }
                if what == "settings" || what == "limits" {
                    settings_ui::open_password(&ui);
                    return;
                }
                if what == "distribution" {
                    plot_ui::open(&ui, today_ymd(&sh), local_secs(&sh));
                    return;
                }
                ui.set_toast_text(s(format!("{}: coming in the next step", title_case(&what))));
            }
        }
    });

    // password + settings window: the backend checks, saves and applies the values
    settings_ui::wire(&ui, {
        let sh = sh.clone();
        move |cmd| send_cmd(&sh, cmd)
    });

    // history plots: the UI reads the database itself
    plot_ui::wire(&ui);

    // cloud upload popup: the date goes to the backend, which does the upload
    upload_ui::wire(&ui, {
        let sh = sh.clone();
        move |cmd| send_cmd(&sh, cmd)
    });

    // touch: trend window chips
    ui.on_window_selected({
        let sh = sh.clone();
        let weak = ui.as_weak();
        move |i| {
            sh.state.lock().unwrap().choice = (i.max(0) as usize).min(WINDOWS.len() - 1);
            if let Some(ui) = weak.upgrade() {
                ui.set_window_choice(i);
                apply_knob(&ui, &sh);
                refresh_trend(&ui, &sh);
            }
            request_overlay(&sh); // new window size: new comparison range
        }
    });

    // Knob chip: two sliders move the start and the end of the window inside the last 24 h
    ui.on_knob_start_moved({
        let sh = sh.clone();
        let weak = ui.as_weak();
        move |u| {
            {
                let mut st = sh.state.lock().unwrap();
                let e = st.knob_end;
                st.knob_start = back_of(u).max(e + MIN_SPAN).min(DAY);
            }
            if let Some(ui) = weak.upgrade() {
                apply_knob(&ui, &sh);
                refresh_trend(&ui, &sh);
            }
        }
    });
    ui.on_knob_end_moved({
        let sh = sh.clone();
        let weak = ui.as_weak();
        move |u| {
            {
                let mut st = sh.state.lock().unwrap();
                let a = st.knob_start;
                st.knob_end = back_of(u).min(a - MIN_SPAN).max(0);
            }
            if let Some(ui) = weak.upgrade() {
                apply_knob(&ui, &sh);
                refresh_trend(&ui, &sh);
            }
        }
    });

    // touch: Yesterday / 3-day / Forecast - each one is its own on/off switch
    ui.on_compare_selected({
        let sh = sh.clone();
        let weak = ui.as_weak();
        move |i| {
            let on = {
                let mut st = sh.state.lock().unwrap();
                let k = ((i - 1).max(0) as usize).min(COMPARE.len() - 1);
                st.compare[k] = !st.compare[k];
                st.overlays[k] = None;
                st.compare
            };
            if let Some(ui) = weak.upgrade() {
                ui.set_show_yday(on[0]);
                ui.set_show_avg3(on[1]);
                ui.set_show_fc(on[2]);
                refresh_trend(&ui, &sh);
            }
            request_overlay(&sh);
        }
    });

    // 2 Hz housekeeping on the UI thread: clock, blink, "no data" detection,
    // and redraw the trend when the window size changed.
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(500), {
        let sh = sh.clone();
        let weak = ui.as_weak();
        let mut last_size = (0.0f32, 0.0f32);
        let mut toast_ticks = 0;
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let size = (ui.get_trend_w(), ui.get_trend_h());
            if size != last_size {
                last_size = size;
                refresh_trend(&ui, &sh);
            }
            let now = now_ms();
            ui.set_blink(!ui.get_blink());
            if !ui.get_toast_text().is_empty() {
                toast_ticks += 1;
                if toast_ticks >= 5 {
                    ui.set_toast_text(s(""));
                    toast_ticks = 0;
                }
            }
            ui.set_has_data(now - sh.last_live_ms.load(Ordering::Relaxed) < 3000);
            // keep the comparison curve moving with NOW: refresh it every
            // 1/60 of the window (10 s on 10 min, 24 min on 24 h)
            let due = {
                let st = sh.state.lock().unwrap();
                st.compare.iter().any(|&c| c) && now / 1000 - st.overlay_asked >= (st.window() / 60).max(10)
            };
            if due {
                request_overlay(&sh);
            }
            let tz = sh.state.lock().unwrap().tz;
            if let Some(tz) = tz {
                let (clock, date) = clock_texts(now / 1000, tz);
                ui.set_clock_text(s(clock));
                ui.set_date_text(s(date));
            }
        }
    });

    std::thread::spawn({
        let sh = sh.clone();
        let weak = ui.as_weak();
        move || socket_thread(sh, weak)
    });

    splash_handoff(); take_drm_master(); // kiosk boot: take the screen from the splash only now
    ui.run()
}

/// Kiosk boot hand-off (only when FREQMON_SPLASH_HANDOFF=1, set by freqmon-ui.service).
/// The splash keeps animating while the app loads the GPU driver and builds the
/// window. Just before the first frame, we ask Plymouth to quit and wait until it
/// has let go of the screen.
fn splash_handoff() {
    if std::env::var("FREQMON_SPLASH_HANDOFF").as_deref() != Ok("1") {
        return;
    }
    let pid_file = std::path::Path::new("/run/plymouth/pid");
    if !pid_file.exists() {
        return; // no splash running (e.g. the app was restarted)
    }
    let t0 = std::time::Instant::now();
    let _ = std::process::Command::new("/usr/bin/sudo")
        .args(["-n", "/usr/bin/plymouth", "quit", "--retain-splash"])
        .status();
    // plymouthd deletes its pid file after it has released the screen
    while pid_file.exists() && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    eprintln!("splash hand-off took {} ms", t0.elapsed().as_millis());
}

/// Kiosk boot: the app opened /dev/dri/card0 while Plymouth still owned it,
/// so after Plymouth has gone we claim it ("DRM master") ourselves.
/// Plymouth may hold it a little longer while it exits, so we retry for up to 5 s.
/// Allowed only with CAP_SYS_ADMIN, which freqmon-ui.service gives the app.
fn take_drm_master() {
    if std::env::var("FREQMON_SPLASH_HANDOFF").as_deref() != Ok("1") {
        return;
    }
    extern "C" {
        fn ioctl(fd: i32, request: std::ffi::c_ulong, ...) -> i32;
    }
    const DRM_IOCTL_SET_MASTER: std::ffi::c_ulong = 0x641e; // _IO('d', 0x1e)
    // every open file of ours that points to the screen device
    let mut fds = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/proc/self/fd") {
        for entry in dir.flatten() {
            let Ok(target) = std::fs::read_link(entry.path()) else { continue };
            if target.to_string_lossy().starts_with("/dev/dri/card") {
                if let Ok(fd) = entry.file_name().to_string_lossy().parse::<i32>() {
                    fds.push(fd);
                }
            }
        }
    }
    let t0 = std::time::Instant::now();
    loop {
        // 0 = we are master now; EBUSY = Plymouth has not let go yet
        let busy = fds.iter().filter(|&&fd| unsafe { ioctl(fd, DRM_IOCTL_SET_MASTER, 0) } != 0).count();
        if busy == 0 || t0.elapsed() > Duration::from_secs(5) {
            eprintln!("DRM master: {} of {} ok after {} ms", fds.len() - busy, fds.len(), t0.elapsed().as_millis());
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod knob_tests {
    use super::*;

    #[test]
    fn slider_curve_round_trips_and_is_fine_near_now() {
        assert_eq!(back_of(0.0), 0);
        assert_eq!(back_of(1.0), DAY);
        for back in [60, 600, 3600, 21_600, DAY] {
            let again = back_of(u_of(back));
            assert!((again - back).abs() <= back / 100 + 1, "{back} -> {again}");
        }
        // one slider pixel near "now" is a few seconds, not minutes (700 px track)
        assert!(back_of(2.0 / 700.0) < 5 && back_of(0.1) < 100);
    }

    #[test]
    fn labels() {
        assert_eq!(back_text(0), "now");
        assert_eq!(back_text(45), "-45 s");
        assert_eq!(back_text(600), "-10 min");
        assert_eq!(back_text(8100), "-2 h 15 m");
    }
}
