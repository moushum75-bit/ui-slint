//! Messages from the backend: one JSON object per line, tagged by "type".
//! serde turns each line into one of these enum variants.
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Debug, Clone, Copy)]
pub struct Limits {
    pub alert_high: f32,
    pub alert_low: f32,
    pub warning_high: f32,
    pub warning_low: f32,
    pub emergency_high: f32,
    pub emergency_low: f32,
    /// rate-of-change limit, Hz per second
    #[serde(default = "default_dfdt")]
    pub dfdt_limit: f32,
}

fn default_dfdt() -> f32 {
    0.03
}

/// One alert sound (backend config.py sound_params_from()).
#[derive(Deserialize, Debug, Clone, Copy, Default)]
#[serde(default)]
pub struct SoundLevel {
    pub freq: f32,
    pub length: f32,
    pub interval: f32,
    pub volume: f32,
    pub wobble: f32,
}

impl Default for Limits {
    fn default() -> Self {
        // same defaults as the backend / main6.py
        Limits {
            alert_high: 50.4,
            alert_low: 49.85,
            warning_high: 51.0,
            warning_low: 49.5,
            emergency_high: 51.25,
            emergency_low: 49.25,
            dfdt_limit: 0.03,
        }
    }
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct Live {
    pub epoch: i64,
    pub date: String,
    pub time: String,
    pub freq: f32,
    pub status: String,
    pub level: i32,
    pub high: bool,
    pub dfdt: bool,
    pub rocof: f32,
    pub window: i64,
    pub min: Option<f32>,
    pub min_time: Option<String>,
    pub max: Option<f32>,
    pub max_time: Option<String>,
    pub avg: Option<f32>,
    pub temp: Option<f32>,
    pub fan: bool,
    pub net: bool,
    pub sim: bool,
}

/// One day of statistics (backend/freqmon/daystats.py summary()).
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct DayStats {
    pub samples: i64,
    pub avg: Option<f32>,
    pub std: f32,
    pub min: Option<f32>,
    pub min_time: Option<String>,
    pub max: Option<f32>,
    pub max_time: Option<String>,
    pub secs: Vec<i64>, // seconds at level 0..3
    pub normal_pct: f32,
    pub events: i64,
    pub longest: i64,
    pub max_rocof: Option<f32>,
    pub max_rocof_time: Option<String>,
    pub fast: i64,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    Hello {
        #[serde(default)]
        tz_offset: i64,
        /// path of the backend's SQLite file (the history plots read it directly)
        #[serde(default)]
        db: Option<String>,
    },
    Limits {
        limits: Limits,
    },
    Live(Live),
    Backfill {
        end: i64,
        values: Vec<Option<f32>>,
    },
    Stats {
        today: Option<DayStats>,
        yesterday: Option<DayStats>,
    },
    Fault {
        text: String,
    },
    Button {
        idx: u8,
    },
    Home,
    /// comparison curve for the split trend (backend freqmon/overlay.py)
    Overlay {
        kind: String,
        start: i64,
        end: i64,
        mid: Vec<Option<f32>>,
        #[serde(default)]
        lo: Option<Vec<Option<f32>>>,
        #[serde(default)]
        hi: Option<Vec<Option<f32>>>,
    },
    /// answer to {"cmd":"upload","date":...} (backend freqmon/upload.py)
    UploadResult {
        #[serde(default)]
        status: String, // success | info | error
        #[serde(default)]
        text: String,
    },
    /// alert sound settings (sent on connect and after every change)
    Sound {
        #[serde(default)]
        on: bool,
        #[serde(default)]
        levels: HashMap<String, SoundLevel>,
    },
    /// answer to a command (check_password, set_limits, set_sound, test_sound ...)
    Reply {
        #[serde(default)]
        cmd: Option<String>,
        #[serde(default)]
        ok: bool,
        #[serde(default)]
        text: Option<String>,
    },
    // history, ... are used by the next screens
    #[serde(other)]
    Other,
}

/// Parse one line. A bare number (the old dummy_backend.py) is accepted too.
pub fn parse_line(line: &str) -> Option<Msg> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    if let Ok(f) = line.parse::<f32>() {
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        return Some(Msg::Live(Live {
            epoch,
            freq: f,
            status: "RAW".into(),
            window: 600,
            ..Default::default()
        }));
    }
    match serde_json::from_str::<Msg>(line) {
        Ok(m) => Some(m),
        Err(e) => {
            eprintln!("bad line from backend: {e}");
            None
        }
    }
}
