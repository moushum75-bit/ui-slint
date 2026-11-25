//! Password-protected settings (ui/settings_dialog.slint).
//!   Settings / Limits button -> password popup -> {"cmd":"check_password"} ->
//!   settings window (draft copy of the backend values) ->
//!   Submit: {"cmd":"set_limits",...} then {"cmd":"set_sound",...}.
//! The backend checks the password again, validates, writes settings.json and
//! applies the values at once. Nothing is stored here except the draft.
//! Everything runs on the UI thread, so the state is a thread_local.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use serde_json::{json, Value};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::protocol::{Limits, SoundLevel};
use crate::{AppWindow, PwdState, SetState};

const KEYS: [&str; 7] = ["HIGH_ALERT", "LOW_ALERT", "HIGH_WARNING", "LOW_WARNING", "HIGH_EMERGENCY", "LOW_EMERGENCY", "DFDT"];
const LIMIT_WIRE: [&str; 6] = ["alert_high", "alert_low", "warning_high", "warning_low", "emergency_high", "emergency_low"];
const REPLY_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy)]
struct Snd {
    freq: f32,
    length: f32,
    interval: f32,
    volume: f32,
    wobble: f32,
}

#[derive(Clone)]
struct Cfg {
    lim: [f32; 7], // 6 limits (Hz) + DF/DT (Hz/s)
    snd: [Snd; 7],
    on: bool,
}

impl Cfg {
    fn defaults() -> Cfg {
        let l = Limits::default();
        let s = |freq, length, interval, wobble| Snd { freq, length, interval, volume: 0.6, wobble };
        Cfg {
            lim: [l.alert_high, l.alert_low, l.warning_high, l.warning_low, l.emergency_high, l.emergency_low, l.dfdt_limit],
            snd: [
                s(1000.0, 0.30, 1.00, 0.0), s(500.0, 0.30, 1.00, 0.0),
                s(1200.0, 0.20, 0.50, 0.0), s(400.0, 0.20, 0.50, 0.0),
                s(1600.0, 0.45, 0.55, 200.0), s(400.0, 0.45, 0.55, 150.0),
                s(2000.0, 0.12, 0.24, 0.0),
            ],
            on: true,
        }
    }
}

#[derive(PartialEq)]
enum Stage {
    Idle,
    Checking,
    SavingLimits,
    SavingSound,
}

struct St {
    server: Cfg, // what the backend has now
    draft: Cfg,  // what the window edits
    tab: usize,
    pw_text: String, // typed
    pw: String,      // verified, sent with set_limits / set_sound
    stage: Stage,
    send: Option<Rc<dyn Fn(Value)>>,
    timer: slint::Timer,
}

thread_local! {
    static ST: RefCell<St> = RefCell::new(St {
        server: Cfg::defaults(), draft: Cfg::defaults(), tab: 0,
        pw_text: String::new(), pw: String::new(), stage: Stage::Idle,
        send: None, timer: slint::Timer::default(),
    });
}

fn send(v: Value) {
    let f = ST.with(|s| s.borrow().send.clone());
    if let Some(f) = f {
        f(v);
    }
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

// ------------------------------------------------ values from the backend ----

pub fn on_limits(l: &Limits) {
    ST.with(|s| {
        s.borrow_mut().server.lim = [l.alert_high, l.alert_low, l.warning_high, l.warning_low, l.emergency_high, l.emergency_low, l.dfdt_limit];
    });
}

pub fn on_sound(on: bool, levels: &HashMap<String, SoundLevel>) {
    ST.with(|s| {
        let mut st = s.borrow_mut();
        st.server.on = on;
        for (i, k) in KEYS.iter().enumerate() {
            if let Some(l) = levels.get(*k) {
                st.server.snd[i] = Snd { freq: l.freq, length: l.length, interval: l.interval, volume: l.volume, wobble: l.wobble };
            }
        }
    });
}

// ------------------------------------------------------- slider limits ----

/// (min, max, step) of one row of a tab. Rows: 0 limit, 1 frequency, 2 beep, 3 interval, 4 volume, 5 wobble.
fn bounds(c: &Cfg, tab: usize, row: usize) -> (f32, f32, f32) {
    let s = c.snd[tab];
    match row {
        0 if tab == 6 => (0.01, 0.50, 0.01),
        0 if tab % 2 == 0 => (50.0, 52.0, 0.01), // high limits
        0 => (48.0, 50.0, 0.01),                 // low limits
        1 => (200.0, 3000.0, 10.0),
        2 => (0.05, s.interval.min(1.0).max(0.05), 0.05),
        3 => (s.length.max(0.1), 2.0, 0.05),
        4 => (0.0, 1.0, 0.05),
        _ => (0.0, (s.freq / 2.0).min(400.0), 10.0),
    }
}

fn get(c: &Cfg, tab: usize, row: usize) -> f32 {
    let s = c.snd[tab];
    [c.lim[tab], s.freq, s.length, s.interval, s.volume, s.wobble][row]
}

fn set(c: &mut Cfg, tab: usize, row: usize, v: f32) {
    let (lo, hi, step) = bounds(c, tab, row);
    let v = ((v / step).round() * step).clamp(lo, hi.max(lo));
    let s = &mut c.snd[tab];
    match row {
        0 => c.lim[tab] = v,
        1 => {
            s.freq = v;
            s.wobble = s.wobble.min(v / 2.0);
        }
        2 => {
            s.length = v;
            s.interval = s.interval.max(v);
        }
        3 => s.interval = v,
        4 => s.volume = v,
        _ => s.wobble = v,
    }
}

fn text_of(tab: usize, row: usize, v: f32) -> String {
    match row {
        0 if tab == 6 => format!("{v:.2} Hz/s"),
        0 => format!("{v:.2} Hz"),
        1 => format!("{v:.0} Hz"),
        2 | 3 => format!("{v:.2} s"),
        4 => format!("{v:.2}"),
        _ => format!("{v:.0} Hz"),
    }
}

/// Copy the draft of the selected tab into Slint.
fn push(ui: &AppWindow) {
    ST.with(|s| {
        let st = s.borrow();
        let (c, t) = (&st.draft, st.tab);
        let ss = ui.global::<SetState>();
        ss.set_tab(t as i32);
        ss.set_sound_on(c.on);
        ss.set_limit_label(if t == 6 { "Rate (Hz/s)" } else { "Limit (Hz)" }.into());
        let rows = 0..6usize;
        ss.set_values(model(rows.clone().map(|r| get(c, t, r)).collect()));
        ss.set_mins(model(rows.clone().map(|r| bounds(c, t, r).0).collect()));
        ss.set_maxs(model(rows.clone().map(|r| bounds(c, t, r).1).collect()));
        ss.set_texts(model(rows.map(|r| slint::SharedString::from(text_of(t, r, get(c, t, r)))).collect()));
    });
}

// --------------------------------------------------------- popups ----

fn expect_answer(ui: &AppWindow) {
    let weak = ui.as_weak();
    ST.with(|s| {
        s.borrow().timer.start(slint::TimerMode::SingleShot, REPLY_TIMEOUT, move || {
            let Some(ui) = weak.upgrade() else { return };
            let was = ST.with(|s| std::mem::replace(&mut s.borrow_mut().stage, Stage::Idle));
            match was {
                Stage::Checking => denied(&ui, "No answer from the backend"),
                Stage::SavingLimits | Stage::SavingSound => message(&ui, "Settings error", "No answer from the backend. The values may not be saved."),
                Stage::Idle => {}
            }
        });
    });
}

fn dots(ui: &AppWindow) {
    let n = ST.with(|s| s.borrow().pw_text.chars().count());
    ui.global::<PwdState>().set_count(n as i32);
}

fn denied(ui: &AppWindow, text: &str) {
    ST.with(|s| s.borrow_mut().pw_text.clear());
    dots(ui);
    let p = ui.global::<PwdState>();
    p.set_denied_text(text.into());
    p.set_phase(3);
}

fn message(ui: &AppWindow, title: &str, text: &str) {
    let ss = ui.global::<SetState>();
    ss.set_msg_title(title.into());
    ss.set_msg_text(text.into());
    ss.set_phase(3);
}

/// Settings / Limits button.
pub fn open_password(ui: &AppWindow) {
    ST.with(|s| {
        let mut st = s.borrow_mut();
        st.pw_text.clear();
        st.stage = Stage::Idle;
    });
    dots(ui);
    let p = ui.global::<PwdState>();
    p.set_shift(false);
    p.set_phase(1);
}

fn close_settings(ui: &AppWindow) {
    send(json!({"cmd": "stop_sound"}));
    ST.with(|s| {
        let mut st = s.borrow_mut();
        st.pw.clear();
        st.stage = Stage::Idle;
        st.timer.stop();
    });
    ui.global::<SetState>().set_phase(0);
}

/// Answers of the backend to our commands.
pub fn on_reply(ui: &AppWindow, cmd: &str, ok: bool, text: &str) {
    let stage = ST.with(|s| std::mem::replace(&mut s.borrow_mut().stage, Stage::Idle));
    match (cmd, stage) {
        ("check_password", Stage::Checking) => {
            ST.with(|s| s.borrow().timer.stop());
            if ok {
                ST.with(|s| {
                    let mut st = s.borrow_mut();
                    st.pw = std::mem::take(&mut st.pw_text);
                    st.draft = st.server.clone();
                    st.tab = 0;
                });
                dots(ui);
                ui.global::<PwdState>().set_phase(0);
                ui.global::<SetState>().set_phase(1);
                push(ui);
            } else {
                denied(ui, if text.is_empty() { "Incorrect password" } else { text });
            }
        }
        ("set_limits", Stage::SavingLimits) => {
            if ok {
                let (pw, cfg) = ST.with(|s| {
                    let mut st = s.borrow_mut();
                    st.stage = Stage::SavingSound;
                    (st.pw.clone(), st.draft.clone())
                });
                let levels: serde_json::Map<String, Value> = KEYS
                    .iter()
                    .enumerate()
                    .map(|(i, k)| {
                        let s = cfg.snd[i];
                        (k.to_string(), json!({"freq": s.freq, "length": s.length, "interval": s.interval, "volume": s.volume, "wobble": s.wobble}))
                    })
                    .collect();
                send(json!({"cmd": "set_sound", "password": pw, "on": cfg.on, "levels": levels}));
                expect_answer(ui);
            } else {
                ST.with(|s| s.borrow().timer.stop());
                message(ui, "Settings error", text);
            }
        }
        ("set_sound", Stage::SavingSound) => {
            ST.with(|s| s.borrow().timer.stop());
            if ok {
                close_settings(ui);
                ui.set_toast_text("Settings saved".into());
            } else {
                message(ui, "Settings error", text);
            }
        }
        ("test_sound", other) => {
            ST.with(|s| s.borrow_mut().stage = other); // not part of the save sequence
            if !ok && !text.is_empty() {
                ui.set_toast_text(text.into());
            }
        }
        (_, other) => ST.with(|s| s.borrow_mut().stage = other),
    }
}

// ----------------------------------------------------------- wiring ----

/// Connect the popups. `send_cmd` writes one JSON command to the backend.
pub fn wire(ui: &AppWindow, send_cmd: impl Fn(Value) + 'static) {
    ST.with(|s| s.borrow_mut().send = Some(Rc::new(send_cmd)));

    // ---- password popup ----
    let p = ui.global::<PwdState>();
    p.on_key({
        let weak = ui.as_weak();
        move |k| {
            let Some(ui) = weak.upgrade() else { return };
            ST.with(|s| {
                let mut st = s.borrow_mut();
                for c in k.chars().filter(|c| c.is_ascii_graphic() || *c == ' ') {
                    if st.pw_text.chars().count() < 32 {
                        st.pw_text.push(c);
                    }
                }
            });
            dots(&ui);
        }
    });
    p.on_backspace({
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            ST.with(|s| {
                s.borrow_mut().pw_text.pop();
            });
            dots(&ui);
        }
    });
    p.on_cancel({
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            ST.with(|s| {
                let mut st = s.borrow_mut();
                st.pw_text.clear();
                st.stage = Stage::Idle;
                st.timer.stop();
            });
            dots(&ui);
            ui.global::<PwdState>().set_phase(0);
        }
    });
    p.on_submit({
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let (pw, busy) = ST.with(|s| {
                let mut st = s.borrow_mut();
                let busy = st.stage != Stage::Idle;
                if !busy && !st.pw_text.is_empty() {
                    st.stage = Stage::Checking;
                }
                (st.pw_text.clone(), busy)
            });
            if busy || pw.is_empty() {
                return;
            }
            ui.global::<PwdState>().set_phase(2);
            send(json!({"cmd": "check_password", "password": pw}));
            expect_answer(&ui);
        }
    });

    // ---- settings window ----
    let ss = ui.global::<SetState>();
    ss.on_select_tab({
        let weak = ui.as_weak();
        move |i| {
            let Some(ui) = weak.upgrade() else { return };
            ST.with(|s| s.borrow_mut().tab = (i.max(0) as usize).min(6));
            send(json!({"cmd": "stop_sound"})); // a running test belongs to the old tab
            push(&ui);
        }
    });
    ss.on_set_value({
        let weak = ui.as_weak();
        move |row, v| {
            let Some(ui) = weak.upgrade() else { return };
            ST.with(|s| {
                let mut st = s.borrow_mut();
                let t = st.tab;
                set(&mut st.draft, t, (row.max(0) as usize).min(5), v);
            });
            push(&ui);
        }
    });
    ss.on_step({
        let weak = ui.as_weak();
        move |row, dir| {
            let Some(ui) = weak.upgrade() else { return };
            ST.with(|s| {
                let mut st = s.borrow_mut();
                let (t, r) = (st.tab, (row.max(0) as usize).min(5));
                let (_, _, step) = bounds(&st.draft, t, r);
                let v = get(&st.draft, t, r) + dir as f32 * step;
                set(&mut st.draft, t, r, v);
            });
            push(&ui);
        }
    });
    ss.on_toggle_sound({
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            ST.with(|s| {
                let mut st = s.borrow_mut();
                st.draft.on = !st.draft.on;
            });
            push(&ui);
        }
    });
    ss.on_test(|| {
        let (key, s) = ST.with(|s| {
            let st = s.borrow();
            (KEYS[st.tab], st.draft.snd[st.tab])
        });
        send(json!({"cmd": "test_sound", "level": key, "seconds": 3.0,
                    "params": {"freq": s.freq, "length": s.length, "interval": s.interval, "volume": s.volume, "wobble": s.wobble}}));
    });
    ss.on_stop(|| send(json!({"cmd": "stop_sound"})));
    ss.on_cancel({
        let weak = ui.as_weak();
        move || {
            if let Some(ui) = weak.upgrade() {
                close_settings(&ui);
            }
        }
    });
    ss.on_submit({
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let (pw, cfg, busy) = ST.with(|s| {
                let mut st = s.borrow_mut();
                let busy = st.stage != Stage::Idle;
                if !busy {
                    st.stage = Stage::SavingLimits;
                }
                (st.pw.clone(), st.draft.clone(), busy)
            });
            if busy {
                return;
            }
            send(json!({"cmd": "stop_sound"}));
            let mut limits = serde_json::Map::new();
            for (i, k) in LIMIT_WIRE.iter().enumerate() {
                limits.insert(k.to_string(), json!(cfg.lim[i]));
            }
            limits.insert("dfdt_limit".into(), json!(cfg.lim[6]));
            ui.global::<SetState>().set_phase(2);
            send(json!({"cmd": "set_limits", "password": pw, "limits": limits}));
            expect_answer(&ui);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_stay_consistent() {
        let mut c = Cfg::defaults();
        // beep longer than the interval pushes the interval up
        c.snd[0].interval = 0.30;
        set(&mut c, 0, 2, 0.9);
        assert!(c.snd[0].interval >= c.snd[0].length);
        // the beep can't go past the interval
        let (_, hi, _) = bounds(&c, 0, 2);
        assert!(hi <= 1.0 && hi >= c.snd[0].interval.min(1.0) - 1e-6);
        // wobble follows the frequency (at most half of it)
        set(&mut c, 4, 1, 400.0);
        assert!(c.snd[4].wobble <= 200.0);
        set(&mut c, 4, 5, 400.0);
        assert!(c.snd[4].wobble <= 200.0);
    }

    #[test]
    fn limit_ranges_and_snapping() {
        let mut c = Cfg::defaults();
        set(&mut c, 0, 0, 50.4049);
        assert!((c.lim[0] - 50.40).abs() < 1e-4);
        set(&mut c, 1, 0, 55.0); // low limit can't go above 50
        assert!((c.lim[1] - 50.0).abs() < 1e-4);
        set(&mut c, 6, 0, 5.0);
        assert!((c.lim[6] - 0.5).abs() < 1e-4);
        assert_eq!(text_of(2, 0, 51.0), "51.00 Hz");
        assert_eq!(text_of(6, 0, 0.03), "0.03 Hz/s");
    }
}
