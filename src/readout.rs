//! Helpers for the frequency card: digits and meter range.
use crate::protocol::Limits;

/// The big number as single characters ("5","0",".","0","1","2"), always
/// the same count so the layout never jumps. `None` shows dashes.
pub fn digits(freq: Option<f32>) -> Vec<String> {
    let text = match freq {
        Some(f) if (0.0..100.0).contains(&f) => format!("{f:06.3}"),
        _ => "--.---".into(),
    };
    text.chars().map(|c| c.to_string()).collect()
}

/// Range of the deviation meter: covers the emergency limits plus a
/// margin, symmetric around 50 Hz, rounded up to 0.5 Hz (48.5..51.5 by default).
pub fn meter_range(l: &Limits) -> (f32, f32) {
    let need = (50.0 - l.emergency_low).max(l.emergency_high - 50.0) + 0.2;
    let half = ((need / 0.5).ceil() * 0.5).clamp(0.5, 3.0);
    (50.0 - half, 50.0 + half)
}
