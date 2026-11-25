//! History plots: date popup, loading, chart window (ui/plot_dialog.slint, plot_window.slint).
//! The UI reads the SQLite file itself (read only); no backend request is needed.
//!   1. popup: one day, or a start/end with times           (callbacks, UI thread)
//!   2. a worker thread reads the rows and counts them     (Hist, src/hist.rs)
//!   3. the UI thread shows the bars and draws the trend in real pixels of its plot area
//! State lives in thread_locals because every callback and the timer run on the UI thread.
use std::cell::RefCell;
use std::sync::Mutex;
use std::time::Duration;

use slint::{Color, ComponentHandle, ModelRc, VecModel};

use crate::cal::{self, Ymd};
use crate::hist::{self, Hist};
use crate::{AppWindow, BarItem, PlotState, Tick};

static DB_PATH: Mutex<Option<String>> = Mutex::new(None);

/// The backend tells us where its database is (hello message). FREQMON_DB overrides it.
pub fn set_db_path(p: String) {
    *DB_PATH.lock().unwrap() = Some(p);
}

fn db_path() -> String {
    std::env::var("FREQMON_DB")
        .ok()
        .or_else(|| DB_PATH.lock().unwrap().clone())
        .unwrap_or_else(|| "frequency_data.db".into())
}

struct Side {
    shown: (i64, u32),
    sel: Ymd,
}

struct Pick {
    today: Ymd,
    now: i64, // wall-clock seconds now
    side: [Side; 2],
    times: [i32; 6],
    range: bool,
}

impl Pick {
    fn new(today: Ymd, now: i64) -> Pick {
        let side = || Side { shown: (today.0, today.1), sel: today };
        Pick { today, now, side: [side(), side()], times: [0, 0, 0, 23, 59, 59], range: false }
    }
}

struct Loaded {
    hist: Hist,
}

thread_local! {
    static PICK: RefCell<Pick> = RefCell::new(Pick::new((1970, 1, 1), 0));
    static LOADED: RefCell<Option<Loaded>> = const { RefCell::new(None) };
    static TIMER: slint::Timer = slint::Timer::default();
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

/// Copy the popup state into Slint.
fn push(ui: &AppWindow) {
    PICK.with(|p| {
        let p = p.borrow();
        let st = ui.global::<PlotState>();
        st.set_range_on(p.range);
        st.set_start_text(cal::fmt(p.side[0].sel).into());
        st.set_end_text(cal::fmt(p.side[1].sel).into());
        st.set_start_title(cal::title(p.side[0].shown).into());
        st.set_end_title(cal::title(p.side[1].shown).into());
        let this_month = (p.today.0, p.today.1);
        st.set_start_next(p.side[0].shown < this_month);
        st.set_end_next(p.side[1].shown < this_month);
        st.set_start_days(model(cal::cells(p.side[0].shown, p.side[0].sel, p.today)));
        st.set_end_days(model(cal::cells(p.side[1].shown, p.side[1].sel, p.today)));
        st.set_times(model(p.times.to_vec()));
    });
}

/// Open the date popup (today selected, one-day mode).
pub fn open(ui: &AppWindow, today: Ymd, now: i64) {
    PICK.with(|p| *p.borrow_mut() = Pick::new(today, now));
    let st = ui.global::<PlotState>();
    st.set_open_pick(0);
    st.set_phase(1);
    push(ui);
}

fn message(ui: &AppWindow, title: &str, text: String, warn: bool) {
    let st = ui.global::<PlotState>();
    st.set_msg_title(title.into());
    st.set_msg_text(text.into());
    st.set_msg_warn(warn);
    st.set_phase(4);
}

/// Read the rows of [start, end] (wall-clock seconds). Runs on a worker thread.
fn load(path: &str, start: i64, end: i64) -> Result<Hist, String> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| format!("Cannot open the database\n{path}\n{e}"))?;
    let _ = conn.busy_timeout(Duration::from_secs(5));
    let mut st = conn
        .prepare("SELECT timestamp, frequency FROM frequency_data WHERE timestamp BETWEEN ?1 AND ?2 ORDER BY timestamp")
        .map_err(|e| format!("Database error: {e}"))?;
    let mut rows = st
        .query([cal::fmt_naive(start), cal::fmt_naive(end)])
        .map_err(|e| format!("Database error: {e}"))?;
    let mut h = Hist::new(start, end);
    while let Some(r) = rows.next().map_err(|e| format!("Database error: {e}"))? {
        let ts: String = r.get(0).map_err(|e| e.to_string())?;
        let f: Option<f64> = r.get(1).map_err(|e| e.to_string())?;
        if let (Some(t), Some(f)) = (cal::parse_naive(&ts), f) {
            h.add(t, f as f32);
        }
    }
    Ok(h)
}

fn title_of(range: bool, start: i64, end: i64, day: Ymd) -> String {
    if range { format!("{} to {}", cal::fmt_naive(start), cal::fmt_naive(end)) } else { cal::fmt(day) }
}

const BAR_COLORS: [(u8, u8, u8); 7] = [
    (0xf8, 0x71, 0x71), (0xfb, 0x92, 0x3c), (0xfb, 0xbf, 0x24), (0x8b, 0xc3, 0x4a),
    (0x4c, 0xaf, 0x50), (0x3b, 0x82, 0xf6), (0xa7, 0x8b, 0xfa),
];

/// The worker is done: show a message, or the chart window.
pub fn finish(ui: &AppWindow, kind: i32, title: String, range_text: (String, String), res: Result<Hist, String>) {
    let h = match res {
        Err(e) => return message(ui, "Database Error", e, false),
        Ok(h) if h.present == 0 => {
            return message(
                ui,
                "No Data",
                format!("No frequency data found for the selected date range.\nFrom: {}\nTo: {}", range_text.0, range_text.1),
                true,
            )
        }
        Ok(h) => h,
    };
    let st = ui.global::<PlotState>();

    // bar chart + pie (independent of the window size)
    let hours = h.hours();
    let total_h = h.total_s() as f64 / 3600.0;
    let max = hours.iter().cloned().fold(0.0, f64::max);
    let bars: Vec<BarItem> = (0..7)
        .map(|i| BarItem {
            label: hist::BAND_LABELS[i].into(),
            value: format!("{:.2} h", hours[i]).into(),
            percent: format!("{:.1} %", 100.0 * hours[i] / total_h).into(),
            frac: if max > 0.0 { (hours[i] / max) as f32 } else { 0.0 },
            color: Color::from_rgb_u8(BAR_COLORS[i].0, BAR_COLORS[i].1, BAR_COLORS[i].2),
        })
        .collect();
    st.set_bars(model(bars));
    let nd = h.no_data_hours();
    st.set_nodata_text(if nd > 0.005 { format!("No data: {nd:.2} h") } else { String::new() }.into());
    let (inside, outside) = h.inside_outside();
    let (pin, pout) = hist::pie(inside, outside);
    st.set_pie_in(pin.into());
    st.set_pie_out(pout.into());
    let both = (inside + outside).max(1e-9);
    st.set_pie_in_text(format!("Inside 49.50-50.50 Hz: {:.2} h ({:.1} %)", inside, 100.0 * inside / both).into());
    st.set_pie_out_text(format!("Outside: {:.2} h ({:.1} %)", outside, 100.0 * outside / both).into());

    st.set_chart_title(title.into());
    st.set_kind(kind.clamp(0, 2));
    st.set_t_line("".into());
    st.set_t_area("".into());
    st.set_t_grid("".into());
    st.set_t_ref("".into());
    st.set_plot_w(0.0);
    st.set_plot_h(0.0);
    LOADED.with(|l| *l.borrow_mut() = Some(Loaded { hist: h }));
    st.set_phase(3);
    render(ui); // sizes are 0 until the window is laid out; the timer redraws then
}

/// Draw the trend for the current plot size (the plot is only there for kind 0 and 2).
pub fn render(ui: &AppWindow) {
    let st = ui.global::<PlotState>();
    if st.get_phase() != 3 || st.get_kind() == 1 {
        return;
    }
    let (w, h) = (st.get_plot_w(), st.get_plot_h());
    if w < 20.0 || h < 20.0 {
        return;
    }
    LOADED.with(|l| {
        let l = l.borrow();
        let Some(l) = l.as_ref() else { return };
        let g = hist::geometry(&l.hist, w, h);
        st.set_t_line(g.line.into());
        st.set_t_area(g.area.into());
        st.set_t_grid(g.grid.into());
        st.set_t_ref(g.refl.into());
        let ticks = |v: Vec<(f32, String)>| model(v.into_iter().map(|(pos, t)| Tick { pos, text: t.into() }).collect());
        st.set_t_xticks(ticks(g.xticks));
        st.set_t_yticks(ticks(g.yticks));
    });
}

/// Start/end of the request, or the text of the error box.
fn request(p: &Pick) -> Result<(i64, i64), String> {
    let t = &p.times;
    let (start, mut end) = if p.range {
        (cal::naive_secs(p.side[0].sel, t[0], t[1], t[2]), cal::naive_secs(p.side[1].sel, t[3], t[4], t[5]))
    } else {
        let d = p.side[0].sel;
        let end = if d == p.today { p.now } else { cal::naive_secs(d, 23, 59, 59) };
        (cal::naive_secs(d, 0, 0, 0), end)
    };
    if start > p.now {
        return Err("The start cannot be in the future.".into());
    }
    end = end.min(p.now); // a range may not run past "now"
    if start > end {
        return Err("Start date must be before end date".into());
    }
    Ok((start, end))
}

/// Connect the popup. Call once at start-up.
pub fn wire(ui: &AppWindow) {
    let st = ui.global::<PlotState>();

    st.on_toggle_range({
        let weak = ui.as_weak();
        move || {
            let Some(ui) = weak.upgrade() else { return };
            PICK.with(|p| {
                let mut p = p.borrow_mut();
                p.range = !p.range;
                if p.range && p.side[1].sel < p.side[0].sel {
                    p.side[1].sel = p.side[0].sel;
                    p.side[1].shown = p.side[0].shown;
                }
            });
            push(&ui);
        }
    });

    st.on_cal_step({
        let weak = ui.as_weak();
        move |which, by| {
            let Some(ui) = weak.upgrade() else { return };
            PICK.with(|p| {
                let mut p = p.borrow_mut();
                let today = p.today;
                let s = &mut p.side[(which as usize).min(1)];
                s.shown = cal::step(s.shown, by, today);
            });
            push(&ui);
        }
    });

    st.on_cal_pick({
        let weak = ui.as_weak();
        move |which, day| {
            let Some(ui) = weak.upgrade() else { return };
            PICK.with(|p| {
                let mut p = p.borrow_mut();
                let today = p.today;
                let s = &mut p.side[(which as usize).min(1)];
                let ymd = (s.shown.0, s.shown.1, day.max(1) as u32);
                if ymd <= today {
                    s.sel = ymd;
                }
            });
            push(&ui);
        }
    });

    st.on_time_pick({
        let weak = ui.as_weak();
        move |idx, v| {
            let Some(ui) = weak.upgrade() else { return };
            PICK.with(|p| {
                if let Some(t) = p.borrow_mut().times.get_mut(idx as usize) {
                    *t = v;
                }
            });
            push(&ui);
        }
    });

    st.on_run({
        let weak = ui.as_weak();
        move |kind| {
            let Some(ui) = weak.upgrade() else { return };
            let req = PICK.with(|p| {
                let p = p.borrow();
                request(&p).map(|(s, e)| (s, e, title_of(p.range, s, e, p.side[0].sel)))
            });
            let (start, end, title) = match req {
                Ok(r) => r,
                Err(text) => return message(&ui, "Date Error", text, true),
            };
            ui.global::<PlotState>().set_phase(2); // "Loading data"
            let path = db_path();
            let weak = ui.as_weak();
            std::thread::spawn(move || {
                let res = load(&path, start, end);
                let range_text = (cal::fmt_naive(start), cal::fmt_naive(end));
                let _ = weak.upgrade_in_event_loop(move |ui| finish(&ui, kind, title, range_text, res));
            });
        }
    });

    // The chart is drawn in pixels of its plot area. When the window is resized (or
    // laid out for the first time) the plot size changes: redraw.
    let mut last = (0.0f32, 0.0f32);
    TIMER.with(|t| {
        let weak = ui.as_weak();
        t.start(slint::TimerMode::Repeated, Duration::from_millis(250), move || {
            let Some(ui) = weak.upgrade() else { return };
            let st = ui.global::<PlotState>();
            if st.get_phase() != 3 {
                last = (0.0, 0.0);
                return;
            }
            let size = (st.get_plot_w(), st.get_plot_h());
            if size != last {
                last = size;
                render(&ui);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database like the backend's (WAL mode): the read-only reader must see the rows.
    #[test]
    fn reads_rows_of_a_range_from_a_wal_database() {
        let path = std::env::temp_dir().join(format!("freqmon_test_{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE frequency_data (timestamp TEXT PRIMARY KEY, frequency REAL, synced INTEGER DEFAULT 0);",
        )
        .unwrap();
        for s in 0..120 {
            conn.execute(
                "INSERT INTO frequency_data (timestamp, frequency) VALUES (?1, ?2)",
                rusqlite::params![format!("2026-11-21 10:{:02}:{:02}", s / 60, s % 60), 50.0 + s as f64 * 0.001],
            )
            .unwrap();
        }
        conn.execute("INSERT INTO frequency_data (timestamp, frequency) VALUES ('2026-11-22 00:00:01', 49.0)", []).unwrap();

        let day = (2026, 11, 21);
        let h = load(path.to_str().unwrap(), cal::naive_secs(day, 0, 0, 0), cal::naive_secs(day, 23, 59, 59)).unwrap();
        assert_eq!(h.present, 120); // the next day's row is not in the range
        assert_eq!(h.total_s(), 86_400);
        assert_eq!(h.counts[3] + h.counts[4], 120);
        assert!(load("/nonexistent/x.db", 0, 10).is_err());
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn request_rules() {
        let today = (2026, 11, 22);
        let now = cal::naive_secs(today, 11, 0, 0);
        let mut p = Pick::new(today, now);
        assert_eq!(request(&p).unwrap(), (cal::naive_secs(today, 0, 0, 0), now)); // today: until now
        p.side[0].sel = (2026, 11, 21);
        assert_eq!(request(&p).unwrap().1, cal::naive_secs((2026, 11, 21), 23, 59, 59));
        p.range = true;
        p.side[1].sel = (2026, 11, 20); // end before start
        assert!(request(&p).unwrap_err().contains("before end"));
    }
}
