//! Cloud upload popup (ui/upload_dialog.slint).
//! The UI only picks a date and shows the answer; the backend does the upload.
//!   UI  -> backend : {"cmd":"upload","id":N,"date":"YYYY-MM-DD"}
//!   backend -> UI  : {"type":"upload_result","status":"success|info|error","text":"..."}
//! Everything here runs on the UI thread (callbacks, timer, show_result), so the
//! small state lives in a thread_local instead of a Mutex.
use std::cell::RefCell;
use std::time::Duration;

use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{AppWindow, UploadState};

use crate::cal::{self, Ymd};

const REPLY_TIMEOUT: Duration = Duration::from_secs(45); // the backend's own request timeout is 20 s

struct Pick {
    today: Ymd,
    shown: (i64, u32), // month shown in the calendar
    sel: Ymd,          // selected date
    next_id: i64,
    timeout: slint::Timer,
}

thread_local! {
    static PICK: RefCell<Pick> = RefCell::new(Pick {
        today: (1970, 1, 1), shown: (1970, 1), sel: (1970, 1, 1),
        next_id: 0, timeout: slint::Timer::default(),
    });
}

// ---- UI state -> Slint ----

fn refresh(ui: &AppWindow) {
    PICK.with(|p| {
        let p = p.borrow();
        let up = ui.global::<UploadState>();
        up.set_date_text(cal::fmt(p.sel).into());
        up.set_cal_title(cal::title(p.shown).into());
        up.set_can_next(p.shown < (p.today.0, p.today.1));
        up.set_days(ModelRc::new(VecModel::from(cal::cells(p.shown, p.sel, p.today))));
    });
}

/// Show the popup with today selected.
pub fn open(ui: &AppWindow, today: Ymd) {
    PICK.with(|p| {
        let mut p = p.borrow_mut();
        p.today = today;
        p.sel = today;
        p.shown = (today.0, today.1);
        p.timeout.stop();
    });
    let up = ui.global::<UploadState>();
    up.set_calendar_open(false);
    up.set_phase(1);
    refresh(ui);
}

/// The backend's answer. Ignored unless an upload is waiting.
pub fn show_result(ui: &AppWindow, status: &str, text: &str) {
    let up = ui.global::<UploadState>();
    if up.get_phase() != 2 {
        return;
    }
    PICK.with(|p| p.borrow().timeout.stop());
    up.set_result_status(if status.is_empty() { "error" } else { status }.into());
    up.set_result_text(text.into());
    up.set_phase(3);
}

/// Connect the popup's buttons. `send` writes one JSON command to the backend.
pub fn wire(ui: &AppWindow, send: impl Fn(serde_json::Value) + 'static) {
    let up = ui.global::<UploadState>();

    up.on_month_step({
        let weak = ui.as_weak();
        move |step| {
            let Some(ui) = weak.upgrade() else { return };
            PICK.with(|p| {
                let mut p = p.borrow_mut();
                p.shown = cal::step(p.shown, step, p.today);
            });
            refresh(&ui);
        }
    });

    up.on_day_picked({
        let weak = ui.as_weak();
        move |day| {
            let Some(ui) = weak.upgrade() else { return };
            PICK.with(|p| {
                let mut p = p.borrow_mut();
                let ymd = (p.shown.0, p.shown.1, day.max(1) as u32);
                if ymd <= p.today {
                    p.sel = ymd;
                }
            });
            ui.global::<UploadState>().set_calendar_open(false);
            refresh(&ui);
        }
    });

    up.on_start({
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            let (date, id) = PICK.with(|p| {
                let mut p = p.borrow_mut();
                p.next_id += 1;
                let weak2 = weak.clone();
                p.timeout.start(slint::TimerMode::SingleShot, REPLY_TIMEOUT, move || {
                    if let Some(ui) = weak2.upgrade() {
                        show_result(&ui, "error", "No answer from the backend. The upload may still be running.");
                    }
                });
                (cal::fmt(p.sel), p.next_id)
            });
            let up = ui.global::<UploadState>();
            up.set_busy_text(format!("Uploading {date} …").into());
            up.set_phase(2);
            send(serde_json::json!({"cmd": "upload", "id": id, "date": date}));
        }
    });
}
