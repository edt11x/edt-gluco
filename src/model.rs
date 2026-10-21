use chrono::{DateTime, Utc};

/// One historical glucose point. Values are always mg/dL inside the app.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub at: DateTime<Utc>,
    pub mgdl: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patient {
    pub id: String,
    pub first_name: String,
    pub last_name: String,
}

impl Patient {
    pub fn label(&self) -> String {
        let name = format!("{} {}", self.first_name, self.last_name);
        let name = name.trim();
        if name.is_empty() {
            "Connection".to_string()
        } else {
            name.to_string()
        }
    }
}

/// What the follower API returned for the signed-in account.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub patients: Vec<Patient>,
    pub selected: usize,
    pub readings: Vec<Reading>,
    pub current_mgdl: Option<f64>,
    pub trend: i32,
    pub measured_at: Option<DateTime<Utc>>,
    pub target_low: f64,
    pub target_high: f64,
    pub very_low: f64,
    pub dtid: i32,
    pub sensor_started: Option<i64>,
    pub mmol: bool,
}

impl Snapshot {
    pub fn selected_patient(&self) -> Option<&Patient> {
        self.patients.get(self.selected)
    }
}
