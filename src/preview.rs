use chrono::{DateTime, Duration, Utc};

use crate::model::{Patient, Reading, Snapshot};

/// A stand-in trace shaped like the Libre home screen: flat and low overnight,
/// a rise late in the window, and a current reading of 91 mg/dL.
pub fn sample(now: DateTime<Utc>) -> Snapshot {
    let window = Duration::hours(12);
    let start = now - window;
    let mut readings = Vec::new();
    let mut cursor = start;
    while cursor < now {
        let fraction = (cursor - start).num_seconds() as f64 / window.num_seconds() as f64;
        readings.push(Reading {
            at: cursor,
            mgdl: curve(fraction),
        });
        cursor += Duration::minutes(5);
    }
    readings.push(Reading {
        at: now,
        mgdl: 91.0,
    });

    Snapshot {
        patients: vec![Patient {
            id: "sample".into(),
            first_name: "Sample".into(),
            last_name: String::new(),
        }],
        selected: 0,
        readings,
        current_mgdl: Some(91.0),
        trend: 3,
        measured_at: Some(now),
        target_low: 70.0,
        target_high: 180.0,
        very_low: 55.0,
        dtid: 40068,
        sensor_started: Some(now.timestamp() - 7 * 86_400),
        mmol: false,
    }
}

fn curve(fraction: f64) -> f64 {
    if fraction < 0.56 {
        66.0 + (fraction * 40.0).sin() * 1.3
    } else if fraction < 0.70 {
        let t = (fraction - 0.56) / 0.14;
        let eased = t * t * (3.0 - 2.0 * t);
        66.0 + eased * 34.0
    } else if fraction < 0.84 {
        let t = (fraction - 0.70) / 0.14;
        100.0 - t * 12.0 + (t * 8.0).sin() * 2.0
    } else {
        let t = (fraction - 0.84) / 0.16;
        88.0 + (t * std::f64::consts::TAU).sin() * 3.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::present::{self, SensorLife};

    #[test]
    fn sample_looks_like_the_phone_screen() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let snapshot = sample(now);
        let last = snapshot.readings.last().unwrap();
        assert_eq!(last.mgdl.round() as i32, 91);
        assert_eq!(snapshot.trend, 3);
        assert!(snapshot.readings.iter().any(|r| r.mgdl < 70.0));
        assert_eq!(
            present::sensor_life(snapshot.sensor_started, 15, now.timestamp()),
            SensorLife::Days(8)
        );
        assert_eq!(present::device_name(snapshot.dtid, 15), "Libre 3 Plus");
    }
}
