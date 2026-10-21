use chrono::{DateTime, Local, Timelike, Utc};

use crate::model::{Reading, Snapshot};
use crate::session::{self, Session};

const MGDL_PER_MMOL: f64 = 18.0182;

#[derive(Clone, Debug)]
pub struct View {
    pub signed_in: bool,
    pub preview: bool,
    pub device_name: String,
    pub patient_name: String,
    pub patients: Vec<String>,
    pub patient_index: i32,
    pub glucose_text: String,
    pub unit_text: String,
    pub status_kind: i32,
    pub status_text: String,
    pub as_of: String,
    pub sensor_text: String,
    pub sensor_level: i32,
    pub trend: i32,
    pub link_state: i32,
    pub banner: String,
    pub mmol: bool,
    pub low: f64,
    pub high: f64,
    pub urgent_high: f64,
    pub readings: Vec<Reading>,
    pub notify: bool,
    pub pinned: bool,
    pub wear_days: i32,
    pub range_index: i32,
    pub host_label: String,
    pub config_path: String,
    pub sync_text: String,
    pub low_threshold: String,
    pub high_threshold: String,
    pub very_low_threshold: String,
    pub urgent_threshold: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorLife {
    Unknown,
    Ended,
    Today,
    Days(i64),
}

pub fn present(
    snapshot: Option<&Snapshot>,
    session: &Session,
    preview: bool,
    signed_in: bool,
    banner: &str,
    link_down: bool,
    sync_text: &str,
    now: DateTime<Utc>,
) -> View {
    let wear =
        session::effective_wear_days(session.wear_days, snapshot.map(|s| s.dtid).unwrap_or(0));
    let mmol = snapshot.map(|s| s.mmol).unwrap_or(false);
    let low = snapshot.map(|s| s.target_low).unwrap_or(70.0);
    let high = snapshot.map(|s| s.target_high).unwrap_or(180.0);
    let very_low = snapshot.map(|s| s.very_low).unwrap_or(55.0);
    let unit = if mmol { "mmol/L" } else { "mg/dL" };
    let (glucose_text, status_kind, status_text, as_of, trend, stale) =
        match snapshot.and_then(|s| s.current_mgdl.map(|mgdl| (mgdl, s.trend, s.measured_at))) {
            Some((mgdl, trend, measured_at)) => {
                let (kind, label) = status_of(mgdl, low, high, very_low);
                let (as_of, stale) = match measured_at {
                    Some(at) => as_of(at, now),
                    None => (String::new(), true),
                };
                (
                    format_glucose(mgdl, mmol),
                    kind,
                    label.to_string(),
                    as_of,
                    trend,
                    stale,
                )
            }
            None => (
                "--".into(),
                0,
                "No current reading".into(),
                String::new(),
                0,
                true,
            ),
        };
    let life = sensor_life(
        snapshot.and_then(|s| s.sensor_started),
        wear,
        now.timestamp(),
    );
    let (sensor_text, sensor_level) = sensor_banner(life);
    let patients: Vec<String> = snapshot
        .map(|s| s.patients.iter().map(|p| p.label()).collect())
        .unwrap_or_default();
    let patient_index = snapshot.map(|s| s.selected as i32).unwrap_or(0);
    let patient_name = patients
        .get(patient_index as usize)
        .cloned()
        .unwrap_or_default();
    let link_state = if link_down {
        2
    } else if snapshot.and_then(|s| s.current_mgdl).is_none() {
        2
    } else if stale {
        1
    } else {
        0
    };

    View {
        signed_in,
        preview,
        device_name: device_name(snapshot.map(|s| s.dtid).unwrap_or(0), wear),
        patient_name,
        patients,
        patient_index,
        glucose_text,
        unit_text: unit.into(),
        status_kind,
        status_text,
        as_of,
        sensor_text,
        sensor_level,
        trend,
        link_state,
        banner: banner.to_string(),
        mmol,
        low,
        high,
        urgent_high: 250.0,
        readings: snapshot.map(|s| s.readings.clone()).unwrap_or_default(),
        notify: session.notify,
        pinned: session.pinned,
        wear_days: wear as i32,
        range_index: session::range_index(session.range_hours),
        host_label: host_label(&session.base_url, &session.region),
        config_path: session::config_file()
            .map(|path| path.display().to_string())
            .unwrap_or_default(),
        sync_text: sync_text.to_string(),
        low_threshold: format!("{} {unit}", format_number(low, mmol)),
        high_threshold: format!("{} {unit}", format_number(high, mmol)),
        very_low_threshold: format!("{} {unit}", format_number(very_low, mmol)),
        urgent_threshold: format!("{} {unit}", format_number(250.0, mmol)),
    }
}

pub fn device_name(dtid: i32, wear_days: u32) -> String {
    let plus = wear_days >= 15;
    match dtid {
        40066 | 40067 => {
            if plus {
                "Libre 2 Plus"
            } else {
                "Libre 2"
            }
        }
        40068 | 40069 | 40070 => {
            if plus {
                "Libre 3 Plus"
            } else {
                "Libre 3"
            }
        }
        0 => "Glucose",
        _ => "Libre",
    }
    .to_string()
}

pub fn status_of(mgdl: f64, low: f64, high: f64, very_low: f64) -> (i32, &'static str) {
    if very_low < low && mgdl < very_low {
        (5, "Glucose Very Low")
    } else if mgdl < low {
        (4, "Glucose Low")
    } else if mgdl <= high {
        (1, "Glucose In Range")
    } else if mgdl >= 250.0 {
        (3, "Glucose Very High")
    } else {
        (2, "Glucose High")
    }
}

pub fn format_glucose(mgdl: f64, mmol: bool) -> String {
    if !mgdl.is_finite() || mgdl <= 0.0 {
        return "--".into();
    }
    if mgdl >= 400.0 {
        return "HI".into();
    }
    if mgdl < 40.0 {
        return "LO".into();
    }
    format_number(mgdl, mmol)
}

pub fn format_number(mgdl: f64, mmol: bool) -> String {
    if mmol {
        format!("{:.1}", mgdl / MGDL_PER_MMOL)
    } else {
        format!("{}", mgdl.round() as i32)
    }
}

pub fn sensor_life(activation: Option<i64>, wear_days: u32, now_unix: i64) -> SensorLife {
    let Some(activation) = activation else {
        return SensorLife::Unknown;
    };
    if activation <= 0 {
        return SensorLife::Unknown;
    }
    if activation < 1_000_000_000 {
        let left = i64::from(wear_days) - activation;
        return if left <= 0 {
            SensorLife::Ended
        } else {
            SensorLife::Days(left)
        };
    }
    if activation > now_unix + 3_600 {
        return SensorLife::Unknown;
    }
    let end = activation + i64::from(wear_days) * 86_400;
    let left = end - now_unix;
    if left <= 0 {
        SensorLife::Ended
    } else if left < 86_400 {
        SensorLife::Today
    } else {
        SensorLife::Days(left / 86_400)
    }
}

pub fn sensor_banner(life: SensorLife) -> (String, i32) {
    match life {
        SensorLife::Unknown => ("Sensor time unavailable".into(), 0),
        SensorLife::Ended => ("Sensor has ended".into(), 2),
        SensorLife::Today => ("Sensor ends today".into(), 1),
        SensorLife::Days(1) => ("1 Day until Sensor ends".into(), 1),
        SensorLife::Days(n) if n <= 2 => (format!("{n} Days until Sensor ends"), 1),
        SensorLife::Days(n) => (format!("{n} Days until Sensor ends"), 0),
    }
}

pub fn format_clock(at: DateTime<Utc>) -> String {
    clock_parts(at.with_timezone(&Local))
}

fn as_of(at: DateTime<Utc>, now: DateTime<Utc>) -> (String, bool) {
    let clock = format_clock(at);
    let age = (now - at).num_seconds();
    if age < 90 {
        (format!("as of {clock}"), false)
    } else if age < 3_600 {
        (
            format!("as of {clock} · {} min ago", age / 60),
            age > 8 * 60,
        )
    } else if age < 48 * 3_600 {
        (format!("as of {clock} · {}h ago", age / 3_600), true)
    } else {
        (format!("as of {clock}"), true)
    }
}

fn clock_parts(local: DateTime<Local>) -> String {
    let hour = local.hour();
    let (h, suffix) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        h => (h - 12, "PM"),
    };
    format!("{h}:{:02} {suffix}", local.minute())
}

fn host_label(base_url: &str, region: &str) -> String {
    if !base_url.is_empty() {
        return base_url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .to_string();
    }
    if region.is_empty() || region == "auto" {
        "Automatic region".into()
    } else {
        region.to_ascii_uppercase()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eight_days_matches_a_fifteen_day_sensor() {
        let now = 1_700_000_000;
        let started = now - 7 * 86_400;
        assert_eq!(sensor_life(Some(started), 15, now), SensorLife::Days(8));
        assert_eq!(
            sensor_banner(SensorLife::Days(8)).0,
            "8 Days until Sensor ends"
        );
    }

    #[test]
    fn sensor_ends_today_and_when_finished() {
        let now = 1_700_000_000;
        assert_eq!(
            sensor_life(Some(now - 15 * 86_400 + 3_600), 15, now),
            SensorLife::Today
        );
        assert_eq!(
            sensor_life(Some(now - 16 * 86_400), 15, now),
            SensorLife::Ended
        );
        assert_eq!(sensor_life(None, 15, now), SensorLife::Unknown);
    }

    #[test]
    fn status_words_follow_the_range() {
        assert_eq!(status_of(91.0, 70.0, 180.0, 55.0).1, "Glucose In Range");
        assert_eq!(status_of(65.0, 70.0, 180.0, 55.0).1, "Glucose Low");
        assert_eq!(status_of(50.0, 70.0, 180.0, 55.0).1, "Glucose Very Low");
        assert_eq!(status_of(210.0, 70.0, 180.0, 55.0).1, "Glucose High");
        assert_eq!(status_of(260.0, 70.0, 180.0, 55.0).1, "Glucose Very High");
        assert_eq!(device_name(40068, 15), "Libre 3 Plus");
        assert_eq!(device_name(40068, 14), "Libre 3");
        assert_eq!(format_glucose(91.0, false), "91");
        assert_eq!(format_glucose(91.0, true), "5.1");
        assert_eq!(format_glucose(420.0, false), "HI");
    }
}
