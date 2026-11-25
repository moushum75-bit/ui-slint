//! Turns the backend "stats" message into the 8 tiles of the statistics panel.
//! Arrows compare today with yesterday: ↑ higher, ↓ lower.
use crate::protocol::{DayStats, Limits};

pub struct TileText {
    pub label: &'static str,
    pub value: String,
    pub unit: &'static str,
    pub sub: String,
    pub level: i32, // -1 plain, 0..3 alarm colour
}

/// Same bands as the backend: 0 normal, 1 alert, 2 warning, 3 emergency.
pub fn classify(f: f32, l: &Limits) -> i32 {
    if f >= l.emergency_high || f <= l.emergency_low {
        3
    } else if f >= l.warning_high || f <= l.warning_low {
        2
    } else if f >= l.alert_high || f <= l.alert_low {
        1
    } else {
        0
    }
}

/// "45 s", "12 m 05 s", "3 h 07 m"
pub fn dur(secs: i64) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} m {:02} s", secs / 60, secs % 60)
    } else {
        format!("{} h {:02} m", secs / 3600, (secs % 3600) / 60)
    }
}

/// short form for sub-lines: "45s", "12m", "3h07"
fn short(secs: i64) -> String {
    if secs == 0 {
        "0".into()
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h{:02}", secs / 3600, (secs % 3600) / 60)
    }
}

/// "↑0.020" / "↓0.004" / "same": difference today - yesterday
fn delta(today: f32, yday: Option<f32>, digits: usize) -> String {
    match yday {
        None => String::new(),
        Some(y) => {
            let d = today - y;
            if d.abs() < 0.5 * 10f32.powi(-(digits as i32)) {
                "same".into()
            } else {
                format!("{}{:.*}", if d > 0.0 { "↑" } else { "↓" }, digits, d.abs())
            }
        }
    }
}

fn hm(t: &Option<String>) -> String {
    t.as_deref().map(|t| t.get(..5).unwrap_or(t).to_string()).unwrap_or_default()
}

fn join(a: String, b: String) -> String {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b,
        (_, true) => a,
        _ => format!("{a} · {b}"),
    }
}

pub fn tiles(today: &DayStats, yday: Option<&DayStats>, lim: &Limits) -> Vec<TileText> {
    let alarm_secs = |d: &DayStats| d.secs.iter().skip(1).sum::<i64>();
    let mut v = Vec::with_capacity(8);

    let max = today.max.unwrap_or(50.0);
    v.push(TileText {
        label: "Maximum", value: format!("{max:.3}"), unit: "Hz",
        sub: join(hm(&today.max_time), delta(max, yday.and_then(|y| y.max), 3)),
        level: classify(max, lim),
    });
    let min = today.min.unwrap_or(50.0);
    v.push(TileText {
        label: "Minimum", value: format!("{min:.3}"), unit: "Hz",
        sub: join(hm(&today.min_time), delta(min, yday.and_then(|y| y.min), 3)),
        level: classify(min, lim),
    });
    let avg = today.avg.unwrap_or(50.0);
    v.push(TileText {
        label: "Average", value: format!("{avg:.3}"), unit: "Hz",
        sub: join(format!("σ {:.3}", today.std), delta(avg, yday.and_then(|y| y.avg), 3)),
        level: -1,
    });
    v.push(TileText {
        label: "Within normal band", value: format!("{:.1}", today.normal_pct), unit: "%",
        sub: yday.map(|y| format!("yday {:.1}%", y.normal_pct)).unwrap_or_else(|| "no data yesterday".into()),
        level: if today.normal_pct >= 99.0 { 0 } else if today.normal_pct >= 95.0 { 1 } else { 2 },
    });

    let worst = (1..4).rev().find(|&i| today.secs.get(i).copied().unwrap_or(0) > 0).unwrap_or(0) as i32;
    let s = |i: usize| today.secs.get(i).copied().unwrap_or(0);
    v.push(TileText {
        label: "Time in alarm", value: dur(alarm_secs(today)), unit: "",
        sub: format!("A {} · W {} · E {}", short(s(1)), short(s(2)), short(s(3))),
        level: if worst == 0 { -1 } else { worst },
    });
    v.push(TileText {
        label: "Excursions", value: today.events.to_string(), unit: "",
        sub: join(
            if today.longest > 0 { format!("longest {}", short(today.longest)) } else { String::new() },
            yday.map(|y| format!("yday {}", y.events)).unwrap_or_default(),
        ),
        level: -1,
    });
    let rocof = today.max_rocof.unwrap_or(0.0);
    v.push(TileText {
        label: "Fastest change", value: format!("{rocof:+.3}"), unit: "Hz/s",
        sub: join(hm(&today.max_rocof_time), format!("{} df/dt", today.fast)),
        level: if today.fast > 0 { 2 } else { -1 },
    });
    let hours = today.samples as f32 / 3600.0;
    v.push(TileText {
        label: "Recorded", value: format!("{hours:.1}"), unit: "h",
        sub: yday
            .map(|y| format!("yday {:.1} h · alarm {}", y.samples as f32 / 3600.0, short(alarm_secs(y))))
            .unwrap_or_else(|| "no data yesterday".into()),
        level: -1,
    });
    v
}
