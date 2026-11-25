//! 24 h ring buffer (one value per second) and the chart geometry.
//! Paths are in pixels of the plot area (w x h): x = time, y = frequency (top = high).
use crate::protocol::Limits;

pub const RING: i64 = 86_400;
const MAX_COLS: i64 = 700; // drawn columns; more is wasted on a 1280 px screen

pub struct Trend {
    vals: Vec<f32>, // NaN = no sample
    end: i64,       // epoch of the newest slot
}

/// A comparison curve from the backend ("overlay" message): one value per
/// bucket between `start` and `end` (epoch s). Forecast also has a band.
pub struct Overlay {
    pub start: i64,
    pub end: i64,
    pub mid: Vec<Option<f32>>,
    pub lo: Option<Vec<Option<f32>>>,
    pub hi: Option<Vec<Option<f32>>>,
    pub future_only: bool, // forecast: draw only right of NOW
}

pub struct View {
    pub ov_lines: [String; 3], // comparison lines: yesterday, 3-day, forecast
    pub ov_band: String,       // forecast band (closed shape)
    pub now_pos: f32,    // NOW line, 0..1 of the width; -1 = no split
    pub line: String,
    pub area: String, // closed shape under the line, for the gradient fill
    pub grid: String,
    pub alert: String,
    pub warning: String,
    pub emergency: String,
    pub xticks: Vec<(f32, String)>,
    pub yticks: Vec<(f32, String)>,
}

const YDIV: i32 = 4; // y labels: 5 values, 50 Hz in the middle

impl Trend {
    pub fn new() -> Self {
        Trend { vals: vec![f32::NAN; RING as usize], end: 0 }
    }

    fn slot(t: i64) -> usize {
        t.rem_euclid(RING) as usize
    }

    pub fn get(&self, t: i64) -> f32 {
        if self.end == 0 || t > self.end || t <= self.end - RING {
            f32::NAN
        } else {
            self.vals[Self::slot(t)]
        }
    }

    pub fn push(&mut self, t: i64, v: f32) {
        if self.end == 0 {
            self.end = t;
        } else if t > self.end {
            // clear the slots we skipped (no data for those seconds)
            let gap = (t - self.end).min(RING);
            for k in 1..gap {
                self.vals[Self::slot(self.end + k)] = f32::NAN;
            }
            self.end = t;
        } else if t <= self.end - RING {
            return; // too old
        }
        self.vals[Self::slot(t)] = v;
    }

    pub fn backfill(&mut self, end: i64, values: &[Option<f32>]) {
        self.vals.iter_mut().for_each(|v| *v = f32::NAN);
        self.end = end;
        let n = values.len() as i64;
        for (i, v) in values.iter().enumerate() {
            let t = end - n + 1 + i as i64;
            if t > end - RING {
                self.vals[Self::slot(t)] = v.unwrap_or(f32::NAN);
            }
        }
    }

    pub fn end(&self) -> i64 {
        self.end
    }

    /// Build the scope picture for the last `window` seconds ending at `now`,
    /// in a plot area of w x h pixels. `tz` = local offset from UTC (s).
    /// `ahead` > 0 splits the plot: the span becomes `window` past + `ahead`
    /// future seconds, NOW sits at the split, and the overlays in `ovs`
    /// (yesterday, 3-day, forecast; any mix of them) are drawn on top.
    #[allow(clippy::too_many_arguments)]
    pub fn view(&self, now: i64, window: i64, ahead: i64, ovs: [Option<&Overlay>; 3],
                lim: &Limits, tz: i64, w: f32, h: f32) -> View {
        let (w, h) = (w.max(10.0), h.max(10.0));
        let window = window.clamp(60, RING);
        let ahead = ahead.max(0);
        let cols_n = (w as i64).clamp(10, MAX_COLS);
        let start = now - window + 1;
        let span = window + ahead; // seconds across the full width

        // 1) min/max per column (several seconds per column on long windows)
        let mut cols: Vec<Option<(f32, f32)>> = Vec::with_capacity(cols_n as usize);
        let mut maxdev: f32 = 0.0;
        for c in 0..cols_n {
            let t0 = start + c * span / cols_n;
            let t1 = (start + (c + 1) * span / cols_n).max(t0 + 1);
            let mut mm: Option<(f32, f32)> = None;
            for t in t0..t1 {
                let v = self.get(t);
                if v.is_nan() {
                    continue;
                }
                maxdev = maxdev.max((v - 50.0).abs());
                mm = Some(match mm {
                    None => (v, v),
                    Some((a, b)) => (a.min(v), b.max(v)),
                });
            }
            cols.push(mm);
        }

        // 1b) overlay points (time, mid, lo, hi) inside the span, per overlay
        type Pt = (i64, Option<f32>, Option<f32>, Option<f32>);
        let mut ov_pts: [Vec<Pt>; 3] = Default::default();
        for (k, o) in ovs.iter().enumerate() {
            let Some(o) = o else { continue };
            let n = o.mid.len().max(1) as i64;
            for i in 0..n {
                let t = o.start + (2 * i + 1) * (o.end - o.start) / (2 * n);
                if t < start || t > start + span || (o.future_only && t < now) {
                    continue;
                }
                let get = |v: &Option<Vec<Option<f32>>>| v.as_ref().and_then(|v| v.get(i as usize).copied().flatten());
                let p = (t, o.mid.get(i as usize).copied().flatten(), get(&o.lo), get(&o.hi));
                for f in [p.1, p.2, p.3].into_iter().flatten() {
                    maxdev = maxdev.max((f - 50.0).abs());
                }
                ov_pts[k].push(p);
            }
        }

        // 2) y scale: symmetric around 50 Hz, zooms to the data
        let half = [0.05f32, 0.1, 0.2, 0.5, 1.0, 1.5, 2.0]
            .into_iter()
            .find(|hh| maxdev * 1.15 <= *hh)
            .unwrap_or(2.0);
        let y = |f: f32| -> f32 { ((50.0 + half - f) / (2.0 * half) * h).clamp(0.0, h) };

        // 3) the line, plus a closed area under each unbroken run of it
        let mut line = String::with_capacity(cols_n as usize * 24);
        let mut area = String::with_capacity(cols_n as usize * 14);
        let mut run_start: Option<f32> = None;
        let mut last_x = 0.0;
        let close = |area: &mut String, x0: f32, x1: f32| {
            *area += &format!("L {x1:.1} {h:.1} L {x0:.1} {h:.1} Z ");
        };
        for (c, mm) in cols.iter().enumerate() {
            let x = c as f32 * w / (cols_n - 1) as f32;
            if ahead > 0 && start + c as i64 * span / cols_n > now {
                continue; // right half: no live data yet
            }
            match mm {
                Some((lo, hi)) => {
                    let first = run_start.is_none();
                    let cmd = if first { 'M' } else { 'L' };
                    line += &format!("{cmd} {x:.1} {:.1} ", y(*hi));
                    if hi - lo > 0.0005 {
                        line += &format!("L {x:.1} {:.1} ", y(*lo));
                    }
                    area += &format!("{cmd} {x:.1} {:.1} ", y((lo + hi) / 2.0));
                    if first {
                        run_start = Some(x);
                    }
                    last_x = x;
                }
                None => {
                    if let Some(x0) = run_start.take() {
                        close(&mut area, x0, last_x);
                    }
                }
            }
        }
        if let Some(x0) = run_start {
            close(&mut area, x0, last_x);
        }

        // 3b) overlay lines (break where there is no data) and the forecast band
        let xt = |t: i64| -> f32 { ((t - start) as f32 / span as f32 * w).clamp(0.0, w) };
        let mut ov_lines: [String; 3] = Default::default();
        for (k, pts) in ov_pts.iter().enumerate() {
            let mut pen_down = false;
            for (t, mid, _, _) in pts {
                match mid {
                    Some(f) => {
                        ov_lines[k] += &format!("{} {:.1} {:.1} ", if pen_down { 'L' } else { 'M' }, xt(*t), y(*f));
                        pen_down = true;
                    }
                    None => pen_down = false,
                }
            }
        }
        let band: Vec<(f32, f32, f32)> = ov_pts[2] // only the forecast has lo/hi
            .iter()
            .filter_map(|(t, _, lo, hi)| Some((xt(*t), y((*hi)?), y((*lo)?))))
            .collect();
        let mut ov_band = String::new();
        if band.len() >= 2 {
            for (i, (x, top, _)) in band.iter().enumerate() {
                ov_band += &format!("{} {x:.1} {top:.1} ", if i == 0 { 'M' } else { 'L' });
            }
            for (x, _, bot) in band.iter().rev() {
                ov_band += &format!("L {x:.1} {bot:.1} ");
            }
            ov_band += "Z";
        }
        let now_pos = if ahead > 0 { (now - start) as f32 / span as f32 } else { -1.0 };

        // limit lines: dashed (10 px dash, 6 px gap), one short segment per dash
        let hline = |f: f32| -> String {
            let mut s = String::new();
            if (f - 50.0).abs() < half {
                let p = y(f);
                let mut x = 0.0;
                while x < w {
                    s += &format!("M {x:.1} {p:.1} L {:.1} {p:.1} ", (x + 10.0).min(w));
                    x += 16.0;
                }
            }
            s
        };

        // 4) dotted grid (Slint paths have no dash style, so each dot is a
        //    short segment) and the labels
        const DOT: f32 = 2.0;
        const STEP: f32 = 6.0;
        let mut grid = String::new();
        let mut yticks = Vec::new();
        // oscilloscope style: 2 grid lines per label, so 8 rows; labels on every 2nd
        for j in 0..=2 * YDIV {
            let f = 50.0 + half - j as f32 * half / YDIV as f32;
            let p = y(f);
            let mut x = 0.0;
            while x < w {
                grid += &format!("M {x:.1} {p:.1} L {:.1} {p:.1} ", (x + DOT).min(w));
                x += STEP;
            }
            if j % 2 == 0 {
                yticks.push((p / h, format!("{f:.2}")));
            }
        }
        // x labels at round steps: 2 min on 10 min, 10 min on 1 h, 1 h on 6 h, 4 h on 24 h
        let xdiv: i32 = if window <= 900 { 5 } else { 6 } * if ahead > 0 { 2 } else { 1 };
        // 2 grid columns per time label: 10 columns on 10 min, 12 on the others
        for i in 0..=2 * xdiv {
            let x = i as f32 * w / (2 * xdiv) as f32;
            let mut yy = 0.0;
            while yy < h {
                grid += &format!("M {x:.1} {yy:.1} L {x:.1} {:.1} ", (yy + DOT).min(h));
                yy += STEP;
            }
        }
        let xticks = (0..=xdiv)
            .map(|i| {
                let t = start + span * i as i64 / xdiv as i64;
                let local = (t + tz).rem_euclid(86_400);
                (
                    i as f32 / xdiv as f32,
                    format!("{:02}:{:02}", local / 3600, (local % 3600) / 60),
                )
            })
            .collect();

        View {
            ov_lines,
            ov_band,
            now_pos,
            line,
            area,
            grid,
            alert: hline(lim.alert_low) + &hline(lim.alert_high),
            warning: hline(lim.warning_low) + &hline(lim.warning_high),
            emergency: hline(lim.emergency_low) + &hline(lim.emergency_high),
            xticks,
            yticks,
        }
    }
}

pub fn window_label(secs: i64) -> String {
    if secs >= 3600 {
        let h = secs as f32 / 3600.0;
        if (h - h.round()).abs() < 0.05 {
            format!("{:.0} h", h)
        } else {
            format!("{:.1} h", h)
        }
    } else {
        format!("{} min", (secs + 30) / 60)
    }
}
