use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

/// Region keys, in the same order as the sign-in menu.
pub const REGIONS: &[(&str, &str)] = &[
    ("auto", "Automatic"),
    ("us", "United States"),
    ("eu", "Europe"),
    ("eu2", "Europe 2"),
    ("de", "Germany"),
    ("fr", "France"),
    ("ca", "Canada"),
    ("au", "Australia"),
    ("jp", "Japan"),
    ("ap", "Asia-Pacific"),
    ("la", "Latin America"),
    ("ae", "United Arab Emirates"),
    ("ru", "Russia"),
    ("cn", "China"),
];

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub email: String,
    pub password: String,
    pub remember: bool,
    pub region: String,
    pub base_url: String,
    pub token: String,
    pub user_id: String,
    pub expires_unix: i64,
    pub patient_id: String,
    pub wear_days: u32,
    pub notify: bool,
    pub pinned: bool,
    pub range_hours: u32,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            email: String::new(),
            password: String::new(),
            remember: true,
            region: "auto".into(),
            base_url: String::new(),
            token: String::new(),
            user_id: String::new(),
            expires_unix: 0,
            patient_id: String::new(),
            wear_days: 0,
            notify: true,
            pinned: false,
            range_hours: 12,
        }
    }
}

impl Session {
    /// Copy safe to write. A forgotten sign-in keeps settings, not the secret.
    pub fn for_disk(&self) -> Self {
        let mut copy = self.clone();
        if !copy.remember {
            copy.password.clear();
            copy.token.clear();
            copy.user_id.clear();
            copy.expires_unix = 0;
        }
        copy
    }

    pub fn can_refresh(&self) -> bool {
        !self.token.is_empty() || !self.password.is_empty()
    }
}

pub fn region_key(index: i32) -> String {
    REGIONS
        .get(index as usize)
        .map(|(key, _)| (*key).to_string())
        .unwrap_or_else(|| "auto".to_string())
}

pub fn region_index(key: &str) -> i32 {
    REGIONS.iter().position(|(k, _)| *k == key).unwrap_or(0) as i32
}

pub fn range_hours(index: i32) -> u32 {
    match index {
        0 => 3,
        1 => 6,
        3 => 24,
        _ => 12,
    }
}

pub fn range_index(hours: u32) -> i32 {
    match hours {
        3 => 0,
        6 => 1,
        24 => 3,
        _ => 2,
    }
}

pub fn effective_wear_days(chosen: u32, dtid: i32) -> u32 {
    if chosen == 14 || chosen == 15 {
        return chosen;
    }
    match dtid {
        40068 | 40069 | 40070 => 15,
        _ => 14,
    }
}

pub fn config_file() -> Option<PathBuf> {
    directories::ProjectDirs::from("io.gluco", "Gluco", "gluco")
        .map(|dirs| dirs.config_dir().join("session.json"))
}

pub fn load() -> Session {
    let Some(path) = config_file() else {
        return Session::default();
    };
    let Ok(text) = fs::read_to_string(&path) else {
        return Session::default();
    };
    match serde_json::from_str::<Session>(&text) {
        Ok(mut session) => {
            if !matches!(session.range_hours, 3 | 6 | 12 | 24) {
                session.range_hours = 12;
            }
            if session.region.is_empty() {
                session.region = "auto".into();
            }
            session
        }
        Err(_) => Session::default(),
    }
}

pub fn save(session: &Session) -> Result<(), String> {
    let Some(path) = config_file() else {
        return Err("Could not find a config directory.".into());
    };
    if let Some(parent) = path.parent() {
        create_private_dir(parent).map_err(|e| e.to_string())?;
    }
    let json = serde_json::to_string_pretty(&session.for_disk()).map_err(|e| e.to_string())?;
    write_private(&path, json.as_bytes()).map_err(|e| e.to_string())
}

fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    if path.exists() {
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        return fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path);
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgotten_sign_in_drops_secrets() {
        let mut session = Session::default();
        session.password = "secret".into();
        session.token = "tok".into();
        session.user_id = "user".into();
        session.remember = false;
        session.wear_days = 15;
        let disk = session.for_disk();
        assert!(disk.password.is_empty());
        assert!(disk.token.is_empty());
        assert!(disk.user_id.is_empty());
        assert_eq!(disk.wear_days, 15);
        assert_eq!(session.password, "secret");
    }

    #[test]
    fn remembered_sign_in_keeps_secrets() {
        let mut session = Session::default();
        session.password = "secret".into();
        session.token = "tok".into();
        session.remember = true;
        let disk = session.for_disk();
        assert_eq!(disk.password, "secret");
        assert_eq!(disk.token, "tok");
    }

    #[test]
    fn region_and_range_indexes_round_trip() {
        assert_eq!(region_key(1), "us");
        assert_eq!(region_index("eu"), 2);
        assert_eq!(region_index("nope"), 0);
        assert_eq!(range_hours(0), 3);
        assert_eq!(range_hours(3), 24);
        assert_eq!(range_index(12), 2);
        assert_eq!(effective_wear_days(0, 40068), 15);
        assert_eq!(effective_wear_days(0, 40066), 14);
        assert_eq!(effective_wear_days(14, 40068), 14);
    }
}
