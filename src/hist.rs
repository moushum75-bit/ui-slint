//! History data for the plot window: per-second values of a time range, the
//! seven frequency bands, and the chart geometry. Pure functions (no UI, no
//! database), so they can be tested.
pub const BAND_LABELS: [&str; 7] =
    ["<48.86", "48.86-49.00", "49.01-49.50", "49.51-50.00", "50.01-50.50", "50.51-51.00", ">51.00"];
const EDGES: [f32; 6] = [48.86, 49.00, 49.50, 50.00, 50.50, 51.00]; // right-inclusive
const INSIDE: [usize; 2] = [3, 4]; // 49.50 .. 50.50 Hz

pub fn band_of(f: f32) -> usize {
    EDGES.iter().position(|&e| f <= e).unwrap_or(6)
}

/// One second = one slot. `NaN` = no sample.
pub struct Hist {
    pub start: i64, // naive seconds of the first slot
    pub vals: Vec<f32>,
    pub counts: [u64; 7],
    pub present: u64,
}

impl Hist {
    pub fn new(start: i64, end: i64) -> Hist {
        let n = (end - start + 1).max(1) as usize;
        Hist { start, vals: vec![f32::NAN; n], counts: [0; 7], present: 0 }
    }

    pub fn add(&mut self, t: i64, f: f32) {
        let i = t - self.start;
        if i < 0 || i as usize >= self.vals.len() || f.is_nan() {
            return;
        }
        if self.vals[i as usize].is_nan() {
            self.present += 1;
        }
        self.vals[i as usize] = f;
        self.counts[band_of(f)] += 1;
    }

    pub fn total_s(&self) -> usize {
        self.vals.len()
    }

    pub fn hours(&self) -> [f64; 7] {
        self.counts.map(|c| c as f64 / 3600.0)
    }

    pub fn no_data_hours(&self) -> f64 {
        (self.total_s() as u64 - self.present) as f64 / 3600.0
    }

    /// (inside, outside) hours for the pie: 49.50 .. 50.50 Hz.
    pub fn inside_outside(&self) -> (f64, f64) {
        let h = self.hours();
        let inside: f64 = INSIDE.iter().map(|&i| h[i]).sum();
        (inside, h.iter().sum::<f64>() - inside)
    }
}

pub struct Geo {
    pub line: String,
    pub area: String,
    pub grid: String,
    pub refl: String, // dashed 50 Hz line
    pub xticks: Vec<(f32, String)>,
    pub yticks: Vec<(f32, String)>,
}

/// Trend paths in pixels of a w x h plot. Several seconds share a column; each
/// column is drawn as a min..max stroke so spikes stay visible.
pub fn geometry(hs: &Hist, w: f32, h: f32) -> Geo {
    let (w, h) = (w.max(10.0), h.max(10.0));
    let n = hs.vals.len();
    let cols = (w as usize).clamp(2, 1400).min(n.max(2));

    let mut mm: Vec<Option<(f32, f32)>> = Vec::with_capacity(cols);
    let (mut dmin, mut dmax) = (f32::MAX, f32::MIN);
    for c in 0..cols {
        let a = (c * n / cols).min(n - 1);
        let b = ((c + 1) * n / cols).max(a + 1).min(n);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for &v in &hs.vals[a..b] {
            if !v.is_nan() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        if hi >= lo {
            dmin = dmin.min(lo);
            dmax = dmax.max(hi);
            mm.push(Some((lo, hi)));
        } else {
            mm.push(None);
        }
    }

    // y range: 49.0 .. 51.5 as before, widened (in 0.5 Hz steps) if the data goes outside
    let (mut ylo, mut yhi) = (49.0f32, 51.5f32);
    if dmin < ylo {
        ylo = (dmin * 2.0).floor() / 2.0;
    }
    if dmax > yhi {
        yhi = (dmax * 2.0).ceil() / 2.0;
    }
    let y = |f: f32| ((yhi - f) / (yhi - ylo) * h).clamp(0.0, h);

    let mut line = String::new();
    let mut area = String::new();
    let mut run: Option<f32> = None;
    let mut last_x = 0.0;
    let close = |area: &mut String, x0: f32, x1: f32| *area += &format!("L {x1:.1} {h:.1} L {x0:.1} {h:.1} Z ");
    for (c, m) in mm.iter().enumerate() {
        let x = c as f32 * w / (cols - 1) as f32;
        match m {
            Some((lo, hi)) => {
                let cmd = if run.is_none() { 'M' } else { 'L' };
                line += &format!("{cmd} {x:.1} {:.1} ", y(*hi));
                if hi - lo > 0.0005 {
                    line += &format!("L {x:.1} {:.1} ", y(*lo));
                }
                area += &format!("{cmd} {x:.1} {:.1} ", y((lo + hi) / 2.0));
                if run.is_none() {
                    run = Some(x);
                }
                last_x = x;
            }
            None => {
                if let Some(x0) = run.take() {
                    close(&mut area, x0, last_x);
                }
            }
        }
    }
    if let Some(x0) = run {
        close(&mut area, x0, last_x);
    }

    // dotted grid: 4 intervals in y, one column per x label
    const DOT: f32 = 2.0;
    const STEP: f32 = 6.0;
    let mut grid = String::new();
    let mut yticks = Vec::new();
    for j in 0..=4 {
        let f = yhi - j as f32 * (yhi - ylo) / 4.0;
        let p = y(f);
        let mut x = 0.0;
        while x < w {
            grid += &format!("M {x:.1} {p:.1} L {:.1} {p:.1} ", (x + DOT).min(w));
            x += STEP;
        }
        yticks.push((p / h, format!("{f:.2}")));
    }
    let xticks = time_ticks(hs.start, n);
    for (pos, _) in &xticks {
        let x = pos * w;
        let mut yy = 0.0;
        while yy < h {
            grid += &format!("M {x:.1} {yy:.1} L {x:.1} {:.1} ", (yy + DOT).min(h));
            yy += STEP;
        }
    }

    // dashed 50 Hz reference line
    let mut refl = String::new();
    let p = y(50.0);
    let mut x = 0.0;
    while x < w {
        refl += &format!("M {x:.1} {p:.1} L {:.1} {p:.1} ", (x + 10.0).min(w));
        x += 16.0;
    }

    Geo { line, area, grid, refl, xticks, yticks }
}

/// Labels at round times (at most about 8): "HH:MM", "MM-DD HH:MM" over more than a day, "MM-DD" for day steps.
pub fn time_ticks(start: i64, total: usize) -> Vec<(f32, String)> {
    const STEPS: [i64; 14] = [60, 120, 300, 600, 900, 1800, 3600, 7200, 10_800, 21_600, 43_200, 86_400, 172_800, 604_800];
    let span = (total as i64 - 1).max(1);
    let step = *STEPS.iter().find(|&&s| span / s <= 8).unwrap_or(&604_800);
    let mut out = Vec::new();
    let mut t = (start + step - 1).div_euclid(step) * step; // wall-clock seconds are multiples of the step from midnight
    while t <= start + span {
        let (_, m, d) = crate::civil_from_days(t.div_euclid(86_400));
        let sod = t.rem_euclid(86_400);
        let label = if step >= 86_400 {
            format!("{m:02}-{d:02}")
        } else if span > 86_400 {
            format!("{m:02}-{d:02} {:02}:{:02}", sod / 3600, (sod % 3600) / 60)
        } else {
            format!("{:02}:{:02}", sod / 3600, (sod % 3600) / 60)
        };
        out.push(((t - start) as f32 / span as f32, label));
        t += step;
    }
    out
}

/// Pie slices as SVG paths in a 100 x 100 box: (inside, outside).
pub fn pie(inside: f64, outside: f64) -> (String, String) {
    let total = inside + outside;
    if total <= 0.0 {
        return (String::new(), String::new());
    }
    let (cx, cy, r) = (50.0f64, 50.0f64, 46.0f64);
    let circle = format!("M {:.1} {cy} A {r} {r} 0 1 1 {:.1} {cy} A {r} {r} 0 1 1 {:.1} {cy} Z", cx - r, cx + r, cx - r);
    let f = inside / total;
    if f >= 0.9999 {
        return (circle, String::new());
    }
    if f <= 0.0001 {
        return (String::new(), circle);
    }
    let a0 = -std::f64::consts::FRAC_PI_2;
    let a1 = a0 + std::f64::consts::TAU * f;
    let a2 = a0 + std::f64::consts::TAU;
    let pt = |a: f64| (cx + r * a.cos(), cy + r * a.sin());
    let slice = |from: f64, to: f64| {
        let (x0, y0) = pt(from);
        let (x1, y1) = pt(to);
        let large = if to - from > std::f64::consts::PI { 1 } else { 0 };
        format!("M {cx} {cy} L {x0:.2} {y0:.2} A {r} {r} 0 {large} 1 {x1:.2} {y1:.2} Z")
    };
    (slice(a0, a1), slice(a1, a2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cal;

    fn day() -> Hist {
        let start = cal::naive_secs((2026, 11, 21), 0, 0, 0);
        Hist::new(start, start + 86_399)
    }

    #[test]
    fn bands_are_right_inclusive() {
        assert_eq!(band_of(48.86), 0);
        assert_eq!(band_of(48.87), 1);
        assert_eq!(band_of(50.00), 3);
        assert_eq!(band_of(50.01), 4);
        assert_eq!(band_of(51.00), 5);
        assert_eq!(band_of(51.01), 6);
    }

    #[test]
    fn hours_and_inside_outside() {
        let mut h = day();
        for s in 0..3600 {
            h.add(h.start + s, 50.0); // band 3, inside
        }
        for s in 3600..5400 {
            h.add(h.start + s, 51.5); // band 6, outside
        }
        assert_eq!(h.present, 5400);
        let (i, o) = h.inside_outside();
        assert!((i - 1.0).abs() < 1e-9 && (o - 0.5).abs() < 1e-9);
        assert!((h.no_data_hours() - (24.0 - 1.5)).abs() < 1e-9);
    }

    #[test]
    fn geometry_breaks_at_gaps_and_has_ticks() {
        let mut h = day();
        for s in 0..3600 {
            h.add(h.start + s, 50.0 + (s as f32 / 600.0).sin() * 0.1);
        }
        for s in 7200..10_800 {
            h.add(h.start + s, 49.9);
        }
        let g = geometry(&h, 800.0, 300.0);
        assert!(g.line.matches('M').count() >= 2, "gap must start a new stroke");
        assert_eq!(g.yticks.len(), 5);
        assert!(g.xticks.len() >= 5 && g.xticks.len() <= 10);
        assert_eq!(g.xticks[0].1, "00:00");
    }

    #[test]
    fn data_outside_range_widens_axis() {
        let mut h = day();
        h.add(h.start, 47.2);
        h.add(h.start + 1, 50.0);
        let g = geometry(&h, 400.0, 200.0);
        assert_eq!(g.yticks.last().unwrap().1, "47.00");
    }

    #[test]
    fn pie_paths() {
        let (i, o) = pie(1.0, 0.0);
        assert!(!i.is_empty() && o.is_empty());
        let (i, o) = pie(3.0, 1.0);
        assert!(i.contains('A') && o.contains('A'));
    }

    #[test]
    fn multi_day_labels() {
        let start = cal::naive_secs((2026, 11, 20), 0, 0, 0);
        let t = time_ticks(start, 3 * 86_400);
        assert!(t.iter().all(|(_, l)| l.contains('-')));
    }
}
