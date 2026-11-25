//! Alerts & notifications: turns the live stream into short, readable events
//! (alarm started / changed / cleared, fast changes, meter and link problems).
//! Newest first, at most MAX entries.
use std::collections::VecDeque;

const MAX: usize = 100;

#[derive(Clone)]
pub struct Entry {
    pub time: String,
    pub level: i32,
    pub text: String,
}

pub struct Notes {
    pub list: VecDeque<Entry>,
    pub rev: u64, // bumped on every change, so the UI knows when to redraw
    last_level: Option<i32>,
    alarm_since: Option<i64>,
    peak: f32,
    last_dfdt: bool,
    fault: String,
}

fn duration(secs: i64) -> String {
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{} h {} min", secs / 3600, (secs % 3600) / 60)
    }
}

impl Notes {
    pub fn new() -> Self {
        Notes { list: VecDeque::new(), rev: 0, last_level: None, alarm_since: None, peak: 50.0, last_dfdt: false, fault: String::new() }
    }

    pub fn push(&mut self, time: &str, level: i32, text: String) {
        self.list.push_front(Entry { time: time.to_string(), level, text });
        self.list.truncate(MAX);
        self.rev += 1;
    }

    pub fn connected(&mut self, time: &str) {
        self.push(time, 0, "Connected to the monitor backend".into());
    }

    pub fn disconnected(&mut self, time: &str) {
        self.push(time, 3, "Lost connection to the backend".into());
        self.last_level = None;
    }

    pub fn fault(&mut self, time: &str, text: &str) {
        if self.fault != text {
            self.fault = text.to_string();
            self.push(time, 3, format!("Meter: {text}"));
        }
    }

    /// Called for every live sample. `status` is e.g. "High warning".
    pub fn live(&mut self, epoch: i64, time: &str, freq: f32, level: i32, status: &str, dfdt: bool, rocof: f32) {
        if !self.fault.is_empty() {
            self.fault.clear();
            self.push(time, 0, "Meter back online".into());
        }
        if self.alarm_since.is_some() && (freq - 50.0).abs() > (self.peak - 50.0).abs() {
            self.peak = freq;
        }
        match self.last_level {
            None if level == 0 => self.push(time, 0, format!("Receiving data, {freq:.3} Hz, normal")),
            Some(prev) if prev == level => {}
            Some(prev) if level == 0 && prev > 0 => {
                let dur = self.alarm_since.map(|t| epoch - t).unwrap_or(0);
                self.push(time, 0, format!("Back to normal after {}, peak {:.3} Hz", duration(dur), self.peak));
                self.alarm_since = None;
            }
            _ => {
                // alarm started, or changed level
                if self.alarm_since.is_none() {
                    self.alarm_since = Some(epoch);
                    self.peak = freq;
                }
                self.push(time, level, format!("{status}: {freq:.3} Hz"));
            }
        }
        self.last_level = Some(level);

        if dfdt && !self.last_dfdt {
            self.push(time, 2, format!("Fast change: {rocof:+.3} Hz/s at {freq:.3} Hz"));
        }
        self.last_dfdt = dfdt;
    }
}
