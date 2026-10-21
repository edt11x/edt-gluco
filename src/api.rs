//! LibreLinkUp follower client.
//!
//! This signs in with the account's own email and password and reads the glucose
//! the account is allowed to follow. It speaks the JSON follower API (product
//! `llu.android` / `llu.ios`, version 4.16). Newer app builds encrypt the glucose
//! payload. This client does not try to decrypt that. If the service returns a
//! protected payload, the window says so and the official app remains the place
//! to look.

use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::Client;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::model::{Patient, Reading, Snapshot};
use crate::session::Session;

const VERSION: &str = "4.16.0";
const ANDROID: &str = "llu.android";
const IOS: &str = "llu.ios";
const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU OS 17_4.1 like Mac OS X) AppleWebKit/536.26 (KHTML, like Gecko) Version/17.4.1 Mobile/10A5355d Safari/8536.25";
const MGDL_PER_MMOL: f64 = 18.0182;

#[derive(Debug)]
pub enum ApiError {
    BadCredentials,
    Locked { seconds: i64 },
    ActionRequired(String),
    VersionRejected,
    NoFollowers,
    Encrypted,
    Network(String),
    Server(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadCredentials => write!(f, "bad credentials"),
            Self::Locked { seconds } => write!(f, "locked for {seconds}s"),
            Self::ActionRequired(step) => write!(f, "action required: {step}"),
            Self::VersionRejected => write!(f, "version rejected"),
            Self::NoFollowers => write!(f, "no followers"),
            Self::Encrypted => write!(f, "encrypted"),
            Self::Network(msg) => write!(f, "network: {msg}"),
            Self::Server(msg) => write!(f, "server: {msg}"),
        }
    }
}

impl std::error::Error for ApiError {}

pub fn explain(error: &ApiError) -> String {
    match error {
        ApiError::BadCredentials => {
            "LibreLinkUp did not accept that email or password. Use the password from the LibreLinkUp app. Wait before trying again so the account is not locked.".into()
        }
        ApiError::Locked { seconds } => {
            let minutes = ((*seconds as f64) / 60.0).ceil().max(1.0) as i64;
            format!("LibreLinkUp locked sign-in for about {minutes} min after several attempts. Wait, then try once. More attempts make the lock longer.")
        }
        ApiError::ActionRequired(step) => {
            format!("LibreLinkUp wants another step ({step}). Open the LibreLinkUp app, finish that prompt, then sign in here.")
        }
        ApiError::VersionRejected => {
            "LibreLinkUp refused this sign-in. Abbott may have changed the follower service for this account. Gluco does not bypass that. Use the LibreLinkUp app on your phone.".into()
        }
        ApiError::NoFollowers => {
            "Signed in, but nobody is sharing a sensor with this LibreLinkUp account yet. In the FreeStyle Libre app, invite this email. Accept the invite in LibreLinkUp, then refresh.".into()
        }
        ApiError::Encrypted => {
            "LibreLinkUp returned a protected response this app cannot read. Open the LibreLinkUp app on your phone for this reading.".into()
        }
        ApiError::Network(msg) => {
            format!("Could not reach LibreLinkUp ({msg}). Check the network and try again.")
        }
        ApiError::Server(msg) => msg.clone(),
    }
}

#[derive(Clone, Debug)]
pub struct Sync {
    pub snapshot: Snapshot,
    pub base_url: String,
    pub token: String,
    pub user_id: String,
    pub expires_unix: i64,
}

pub fn http_client() -> Result<Client, ApiError> {
    Client::builder()
        .timeout(Duration::from_secs(25))
        .connect_timeout(Duration::from_secs(10))
        .gzip(true)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| ApiError::Network(e.to_string()))
}

pub async fn sync(http: &Client, session: &Session) -> Result<Sync, ApiError> {
    let mut base = if session.base_url.is_empty() {
        base_url(&session.region)
    } else {
        session.base_url.clone()
    };
    let mut token = session.token.clone();
    let mut user_id = session.user_id.clone();
    let mut expires = session.expires_unix;
    let now = chrono::Utc::now().timestamp();
    let token_stale = token.is_empty() || (expires > 0 && now > expires - 3_600);
    if token_stale {
        let login = login(http, &session.email, &session.password, &base).await?;
        base = login.base_url;
        token = login.token;
        user_id = login.user_id;
        expires = login.expires_unix;
    }

    let snapshot = match fetch_snapshot(http, &base, &token, &user_id, &session.patient_id).await {
        Ok(snapshot) => snapshot,
        Err(ApiError::Server(msg)) if msg.contains("HTTP 401") && !session.password.is_empty() => {
            let login = login(http, &session.email, &session.password, &base).await?;
            base = login.base_url;
            token = login.token;
            user_id = login.user_id;
            expires = login.expires_unix;
            fetch_snapshot(http, &base, &token, &user_id, &session.patient_id).await?
        }
        Err(other) => return Err(other),
    };

    if let Some(refreshed) = snapshot_token(&snapshot) {
        token = refreshed.0;
        if refreshed.1 > 0 {
            expires = refreshed.1;
        }
    }

    Ok(Sync {
        snapshot: snapshot.snapshot,
        base_url: base,
        token,
        user_id,
        expires_unix: expires,
    })
}

struct Login {
    base_url: String,
    token: String,
    user_id: String,
    expires_unix: i64,
}

async fn login(
    http: &Client,
    email: &str,
    password: &str,
    start_url: &str,
) -> Result<Login, ApiError> {
    if email.trim().is_empty() || password.is_empty() {
        return Err(ApiError::BadCredentials);
    }
    let mut product = ANDROID;
    let mut url = start_url.trim_end_matches('/').to_string();
    let mut switched_product = false;
    let mut seen_redirects = 0u8;
    for _ in 0..5 {
        let (status, body) = post_login(http, &url, product, email, password).await?;
        if is_version_gate(status, &body) && !switched_product {
            product = IOS;
            switched_product = true;
            continue;
        }
        match parse_login(status, &body) {
            LoginParse::Session {
                token,
                user_id,
                expires_unix,
            } => {
                return Ok(Login {
                    base_url: url,
                    token,
                    user_id,
                    expires_unix,
                });
            }
            LoginParse::Redirect { region } => {
                seen_redirects += 1;
                if seen_redirects > 2 {
                    return Err(ApiError::Server(
                        "LibreLinkUp kept redirecting regions. Pick the region in the sign-in list and try once.".into(),
                    ));
                }
                url = base_url(&region);
            }
            LoginParse::Failed(error) => return Err(error),
        }
    }
    Err(ApiError::Server(
        "LibreLinkUp did not finish sign-in. Pick the region and try once.".into(),
    ))
}

async fn post_login(
    http: &Client,
    base: &str,
    product: &str,
    email: &str,
    password: &str,
) -> Result<(u16, String), ApiError> {
    let response = http
        .post(format!("{base}/llu/auth/login"))
        .headers(common_headers(product))
        .header(CONTENT_TYPE, "application/json")
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send()
        .await
        .map_err(|e| ApiError::Network(brief_err(&e.to_string())))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    Ok((status, body))
}

async fn fetch_snapshot(
    http: &Client,
    base: &str,
    token: &str,
    user_id: &str,
    preferred_patient: &str,
) -> Result<Fetched, ApiError> {
    let connections_body =
        authed_get(http, &format!("{base}/llu/connections"), token, user_id).await?;
    let connections = parse_connections(&connections_body)?;
    if connections.is_empty() {
        return Err(ApiError::NoFollowers);
    }
    let selected = connections
        .iter()
        .position(|c| c.patient.id == preferred_patient)
        .unwrap_or(0);
    let chosen = &connections[selected];
    let graph_body = authed_get(
        http,
        &format!(
            "{base}/llu/connections/{}/graph",
            path_segment(&chosen.patient.id)
        ),
        token,
        user_id,
    )
    .await?;
    let graph = parse_graph(&graph_body)?;
    Ok(merge(connections, selected, graph))
}

struct Fetched {
    snapshot: Snapshot,
    token: Option<String>,
    token_expires: Option<i64>,
}

fn snapshot_token(fetched: &Fetched) -> Option<(String, i64)> {
    fetched
        .token
        .as_ref()
        .map(|token| (token.clone(), fetched.token_expires.unwrap_or(0)))
}

async fn authed_get(
    http: &Client,
    url: &str,
    token: &str,
    user_id: &str,
) -> Result<String, ApiError> {
    let response = http
        .get(url)
        .headers(common_headers(ANDROID))
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .header("Account-Id", account_id_header(user_id))
        .send()
        .await
        .map_err(|e| ApiError::Network(brief_err(&e.to_string())))?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;
    if status == 401 {
        return Err(ApiError::Server("HTTP 401".into()));
    }
    if is_version_gate(status, &body) {
        return Err(ApiError::VersionRejected);
    }
    if !(200..300).contains(&status) {
        return Err(ApiError::Server(public_message(&body, status)));
    }
    if looks_encrypted(&body) {
        return Err(ApiError::Encrypted);
    }
    Ok(body)
}

fn common_headers(product: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(ACCEPT, "application/json".parse().unwrap());
    headers.insert("product", product.parse().unwrap());
    headers.insert("version", VERSION.parse().unwrap());
    headers.insert("accept-language", "en-US".parse().unwrap());
    headers.insert("cache-control", "no-cache".parse().unwrap());
    headers.insert("pragma", "no-cache".parse().unwrap());
    headers
}

pub fn account_id_header(user_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(user_id.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn base_url(region: &str) -> String {
    let region = region.trim().trim_end_matches('/');
    if region.starts_with("https://") || region.starts_with("http://") {
        return region.to_string();
    }
    match region.to_ascii_lowercase().as_str() {
        "us" => "https://api-us.libreview.io",
        "eu" => "https://api-eu.libreview.io",
        "eu2" => "https://api-eu2.libreview.io",
        "de" => "https://api-de.libreview.io",
        "fr" => "https://api-fr.libreview.io",
        "jp" => "https://api-jp.libreview.io",
        "ap" => "https://api-ap.libreview.io",
        "au" => "https://api-au.libreview.io",
        "ae" => "https://api-ae.libreview.io",
        "ca" => "https://api-ca.libreview.io",
        "la" => "https://api-la.libreview.io",
        "ru" => "https://api.libreview.ru",
        "cn" => "https://api-cn.myfreestyle.cn",
        "auto" | "global" | "" => "https://api.libreview.io",
        other if other.len() <= 4 && other.chars().all(|c| c.is_ascii_alphanumeric()) => {
            return format!("https://api-{other}.libreview.io");
        }
        _ => "https://api.libreview.io",
    }
    .to_string()
}

#[derive(Debug)]
enum LoginParse {
    Session {
        token: String,
        user_id: String,
        expires_unix: i64,
    },
    Redirect {
        region: String,
    },
    Failed(ApiError),
}

fn parse_login(http_status: u16, body: &str) -> LoginParse {
    if is_version_gate(http_status, body) {
        return LoginParse::Failed(ApiError::VersionRejected);
    }
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        if http_status == 429 {
            return LoginParse::Failed(ApiError::Locked { seconds: 300 });
        }
        if looks_encrypted(body) && (200..300).contains(&http_status) {
            return LoginParse::Failed(ApiError::Encrypted);
        }
        return LoginParse::Failed(ApiError::Server(public_message(body, http_status)));
    };
    if let Some(seconds) = lockout_seconds(&value) {
        return LoginParse::Failed(ApiError::Locked { seconds });
    }
    if http_status == 429 {
        return LoginParse::Failed(ApiError::Locked { seconds: 300 });
    }
    let status = value.get("status").and_then(|s| s.as_i64()).unwrap_or(0);
    if status == 2 {
        return LoginParse::Failed(ApiError::BadCredentials);
    }
    if status == 4 {
        let step = value
            .pointer("/data/step/type")
            .or_else(|| value.pointer("/data/step/componentName"))
            .and_then(|s| s.as_str())
            .unwrap_or("extra step")
            .to_string();
        return LoginParse::Failed(ApiError::ActionRequired(step));
    }
    if let Some(token) = value
        .pointer("/data/authTicket/token")
        .and_then(|t| t.as_str())
    {
        if !token.is_empty() {
            let user_id = json_id(value.pointer("/data/user/id"));
            if user_id.is_empty() {
                return LoginParse::Failed(ApiError::Server(
                    "LibreLinkUp signed in without an account id.".into(),
                ));
            }
            let expires = value
                .pointer("/data/authTicket")
                .map(ticket_expiry)
                .unwrap_or(0);
            return LoginParse::Session {
                token: token.to_string(),
                user_id,
                expires_unix: expires,
            };
        }
    }
    if value.pointer("/data/redirect").and_then(|v| v.as_bool()) == Some(true) {
        if let Some(region) = value.pointer("/data/region").and_then(|r| r.as_str()) {
            return LoginParse::Redirect {
                region: region.to_string(),
            };
        }
    }
    if !(200..300).contains(&http_status) {
        return LoginParse::Failed(ApiError::Server(public_message(body, http_status)));
    }
    LoginParse::Failed(ApiError::Server(
        "LibreLinkUp returned a sign-in response this app does not understand.".into(),
    ))
}

struct Conn {
    patient: Patient,
    target_low: f64,
    target_high: f64,
    very_low: f64,
    dtid: i32,
    sensor_started: Option<i64>,
    current_mgdl: Option<f64>,
    trend: i32,
    measured_at: Option<DateTime<Utc>>,
    mmol: bool,
}

#[derive(Debug)]
struct Graph {
    readings: Vec<Reading>,
    target_low: Option<f64>,
    target_high: Option<f64>,
    very_low: Option<f64>,
    dtid: Option<i32>,
    sensor_started: Option<i64>,
    current_mgdl: Option<f64>,
    trend: Option<i32>,
    measured_at: Option<DateTime<Utc>>,
    mmol: Option<bool>,
    token: Option<String>,
    token_expires: Option<i64>,
}

fn parse_connections(body: &str) -> Result<Vec<Conn>, ApiError> {
    let value: Value = serde_json::from_str(body).map_err(|_| ApiError::Encrypted)?;
    if value
        .pointer("/data")
        .map(|d| d.is_string())
        .unwrap_or(false)
    {
        return Err(ApiError::Encrypted);
    }
    let Some(list) = value.get("data").and_then(|d| d.as_array()).or_else(|| {
        value
            .pointer("/data/connections")
            .and_then(|d| d.as_array())
    }) else {
        let status = value.get("status").and_then(|s| s.as_i64()).unwrap_or(0);
        if status != 0 {
            return Err(ApiError::Server(public_message(body, 200)));
        }
        return Ok(Vec::new());
    };
    Ok(list.iter().filter_map(parse_connection).collect())
}

fn parse_connection(value: &Value) -> Option<Conn> {
    let id = json_id(value.get("patientId"));
    if id.is_empty() {
        return None;
    }
    let measurement = value
        .get("glucoseMeasurement")
        .filter(|m| m.is_object())
        .or_else(|| value.get("glucoseItem").filter(|m| m.is_object()));
    let (current_mgdl, trend, measured_at, mmol) = match measurement {
        Some(item) => {
            let mgdl = item.get("ValueInMgPerDl").and_then(|v| v.as_f64());
            let display = item.get("Value").and_then(|v| v.as_f64());
            let mmol = match (display, mgdl) {
                (Some(display), Some(mgdl)) => detect_mmol(display, mgdl),
                _ => false,
            };
            (
                mgdl.filter(|v| v.is_finite() && *v > 0.0),
                item.get("TrendArrow").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                item.get("FactoryTimestamp")
                    .and_then(|v| v.as_str())
                    .and_then(parse_llu_time),
                mmol,
            )
        }
        None => (None, 0, None, false),
    };
    let target_low = sane_low(
        value
            .get("targetLow")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
    );
    let target_high = sane_high(
        value
            .get("targetHigh")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0),
    );
    let very_raw = value
        .pointer("/alarmRules/f/th")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    Some(Conn {
        patient: Patient {
            id,
            first_name: value
                .get("firstName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
            last_name: value
                .get("lastName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        },
        target_low,
        target_high,
        very_low: sane_very_low(very_raw, target_low),
        dtid: device_type(value),
        sensor_started: sensor_started(value.get("sensor")),
        current_mgdl,
        trend,
        measured_at,
        mmol,
    })
}

fn parse_graph(body: &str) -> Result<Graph, ApiError> {
    let value: Value = serde_json::from_str(body).map_err(|_| ApiError::Encrypted)?;
    if value
        .pointer("/data")
        .map(|d| d.is_string())
        .unwrap_or(false)
    {
        return Err(ApiError::Encrypted);
    }
    let points = value.pointer("/data/graphData").and_then(|d| d.as_array());
    let mut readings = Vec::new();
    let mut mmol = None;
    if let Some(points) = points {
        for point in points {
            let Some(mgdl) = point.get("ValueInMgPerDl").and_then(|v| v.as_f64()) else {
                continue;
            };
            if !mgdl.is_finite() || mgdl <= 0.0 {
                continue;
            }
            let Some(stamp) = point
                .get("FactoryTimestamp")
                .and_then(|v| v.as_str())
                .and_then(parse_llu_time)
            else {
                continue;
            };
            if mmol.is_none() {
                if let Some(display) = point.get("Value").and_then(|v| v.as_f64()) {
                    if detect_mmol(display, mgdl) {
                        mmol = Some(true);
                    }
                }
            }
            readings.push(Reading { at: stamp, mgdl });
        }
    }
    let connection = value.pointer("/data/connection");
    let measurement = connection
        .and_then(|c| c.get("glucoseMeasurement").filter(|m| m.is_object()))
        .or_else(|| connection.and_then(|c| c.get("glucoseItem").filter(|m| m.is_object())));
    let current = measurement.and_then(|item| {
        let mgdl = item.get("ValueInMgPerDl").and_then(|v| v.as_f64())?;
        Some((
            mgdl,
            item.get("TrendArrow").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
            item.get("FactoryTimestamp")
                .and_then(|v| v.as_str())
                .and_then(parse_llu_time),
            item.get("Value")
                .and_then(|v| v.as_f64())
                .map(|display| detect_mmol(display, mgdl)),
        ))
    });
    let token = value
        .pointer("/ticket/token")
        .and_then(|t| t.as_str())
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let token_expires = value.pointer("/ticket").map(ticket_expiry);
    Ok(Graph {
        readings,
        target_low: connection
            .and_then(|c| c.get("targetLow"))
            .and_then(|v| v.as_f64())
            .map(sane_low),
        target_high: connection
            .and_then(|c| c.get("targetHigh"))
            .and_then(|v| v.as_f64())
            .map(sane_high),
        very_low: connection
            .and_then(|c| c.pointer("/alarmRules/f/th"))
            .and_then(|v| v.as_f64()),
        dtid: connection
            .map(device_type)
            .filter(|id| *id != 0)
            .or_else(|| {
                value
                    .pointer("/data/activeSensors/0/device/dtid")
                    .and_then(|v| v.as_i64())
                    .map(|v| v as i32)
            }),
        sensor_started: connection
            .and_then(|c| sensor_started(c.get("sensor")))
            .or_else(|| sensor_started(value.pointer("/data/activeSensors/0/sensor"))),
        current_mgdl: current
            .as_ref()
            .map(|c| c.0)
            .filter(|v| v.is_finite() && *v > 0.0),
        trend: current.as_ref().map(|c| c.1),
        measured_at: current.as_ref().and_then(|c| c.2),
        mmol: mmol.or_else(|| current.and_then(|c| c.3)),
        token,
        token_expires,
    })
}

fn merge(connections: Vec<Conn>, selected: usize, graph: Graph) -> Fetched {
    let chosen = &connections[selected];
    let target_low = graph.target_low.unwrap_or(chosen.target_low);
    let target_high = graph.target_high.unwrap_or(chosen.target_high);
    let very_low = graph
        .very_low
        .map(|v| sane_very_low(v, target_low))
        .unwrap_or(chosen.very_low);
    let mut readings = graph.readings;
    let current_mgdl = graph.current_mgdl.or(chosen.current_mgdl);
    let measured_at = graph.measured_at.or(chosen.measured_at);
    if let (Some(mgdl), Some(at)) = (current_mgdl, measured_at) {
        if !readings.iter().any(|r| r.at == at) {
            readings.push(Reading { at, mgdl });
        }
    }
    let patients = connections.iter().map(|c| c.patient.clone()).collect();
    Fetched {
        snapshot: Snapshot {
            patients,
            selected,
            readings,
            current_mgdl,
            trend: graph.trend.unwrap_or(chosen.trend),
            measured_at,
            target_low,
            target_high,
            very_low,
            dtid: graph.dtid.unwrap_or(chosen.dtid),
            sensor_started: graph.sensor_started.or(chosen.sensor_started),
            mmol: graph.mmol.unwrap_or(chosen.mmol),
        },
        token: graph.token,
        token_expires: graph.token_expires,
    }
}

fn device_type(connection: &Value) -> i32 {
    connection
        .pointer("/patientDevice/dtid")
        .or_else(|| connection.pointer("/device/dtid"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as i32
}

fn sensor_started(sensor: Option<&Value>) -> Option<i64> {
    let sensor = sensor?;
    let raw = sensor.get("a")?;
    let value = raw.as_i64().or_else(|| raw.as_f64().map(|v| v as i64))?;
    if value <= 0 {
        None
    } else {
        Some(value)
    }
}

pub fn detect_mmol(display: f64, mgdl: f64) -> bool {
    if !display.is_finite() || !mgdl.is_finite() || mgdl < 1.0 {
        return false;
    }
    let mmol = mgdl / MGDL_PER_MMOL;
    let err_mg = (display - mgdl).abs();
    let err_mmol = (display - mmol).abs();
    err_mmol + 0.05 < err_mg && err_mmol < 0.3
}

fn to_mgdl_threshold(value: f64) -> f64 {
    if value > 0.0 && value < 35.0 {
        value * MGDL_PER_MMOL
    } else {
        value
    }
}

fn sane_low(value: f64) -> f64 {
    let value = to_mgdl_threshold(value);
    if (40.0..120.0).contains(&value) {
        value
    } else {
        70.0
    }
}

fn sane_high(value: f64) -> f64 {
    let value = to_mgdl_threshold(value);
    if (100.0..400.0).contains(&value) {
        value
    } else {
        180.0
    }
}

fn sane_very_low(value: f64, low: f64) -> f64 {
    let value = to_mgdl_threshold(value);
    if value >= 40.0 && value < low {
        value
    } else {
        55.0_f64.min((low - 1.0).max(40.0))
    }
}

pub fn parse_llu_time(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    let (date, rest) = text.split_once(' ')?;
    let mut date_parts = date.split('/');
    let month: u32 = date_parts.next()?.parse().ok()?;
    let day: u32 = date_parts.next()?.parse().ok()?;
    let year: i32 = date_parts.next()?.parse().ok()?;
    let mut time_parts = rest.split_whitespace();
    let hms = time_parts.next()?;
    let ampm = time_parts.next().map(|s| s.to_ascii_lowercase());
    let mut hms_parts = hms.split(':');
    let mut hour: u32 = hms_parts.next()?.parse().ok()?;
    let minute: u32 = hms_parts.next()?.parse().ok()?;
    let second: u32 = hms_parts.next().unwrap_or("0").parse().ok()?;
    if let Some(marker) = ampm {
        if marker.starts_with('p') && hour < 12 {
            hour += 12;
        }
        if marker.starts_with('a') && hour == 12 {
            hour = 0;
        }
    }
    if hour > 23 {
        return None;
    }
    let naive = NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hour, minute, second)?;
    Some(naive.and_utc())
}

fn ticket_expiry(ticket: &Value) -> i64 {
    let mut expires = ticket.get("expires").and_then(|v| v.as_i64()).unwrap_or(0);
    if expires > 10_000_000_000 {
        expires /= 1000;
    }
    if expires == 0 {
        let duration = ticket.get("duration").and_then(|v| v.as_i64()).unwrap_or(0);
        let duration = if duration > 10_000_000_000 {
            duration / 1000
        } else {
            duration
        };
        if duration > 0 {
            return Utc::now().timestamp() + duration;
        }
    }
    expires
}

fn json_id(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

fn lockout_seconds(value: &Value) -> Option<i64> {
    let message = value
        .pointer("/data/message")
        .or_else(|| value.get("message"))
        .and_then(|m| m.as_str())
        .unwrap_or("");
    let code = value
        .pointer("/data/code")
        .and_then(|c| c.as_i64())
        .unwrap_or(0);
    let status = value.get("status").and_then(|s| s.as_i64()).unwrap_or(0);
    if message.eq_ignore_ascii_case("locked") || code == 60 || status == 429 {
        return Some(
            value
                .pointer("/data/data/lockout")
                .or_else(|| value.pointer("/data/lockout"))
                .and_then(|n| n.as_i64())
                .unwrap_or(300),
        );
    }
    None
}

fn is_version_gate(status: u16, body: &str) -> bool {
    body.contains("minimumVersion")
        || body.contains(\"status\":920")
        || body.contains(\"status\": 920")
        || (status == 403 && body.contains("4.16"))
}

fn looks_encrypted(body: &str) -> bool {
    let trimmed = body.trim_start();
    !trimmed.is_empty() && !trimmed.starts_with('{') && !trimmed.starts_with('[')
}

fn public_message(body: &str, status: u16) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        for pointer in ["/error/message", "/message", "/data/message"] {
            if let Some(message) = value.pointer(pointer).and_then(|m| m.as_str()) {
                if message.len() < 140
                    && !message.to_ascii_lowercase().contains("token")
                    && !message.contains("eyJ")
                {
                    return format!("LibreLinkUp said: {message}");
                }
            }
        }
    }
    if status == 0 {
        "LibreLinkUp returned a response this app does not understand.".into()
    } else {
        format!("LibreLinkUp returned HTTP {status}.")
    }
}

fn brief_err(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() > 120 {
        format!("{}\u2026", &text[..120])
    } else {
        text
    }
}

fn path_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_id_is_sha256_of_the_user_id() {
        assert_eq!(
            account_id_header("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parses_factory_timestamps() {
        let pm = parse_llu_time("5/21/2022 1:38:50 PM").unwrap();
        assert_eq!(pm.to_rfc3339(), "2022-05-21T13:38:50+00:00");
        let am = parse_llu_time("5/21/2022 12:05:00 AM").unwrap();
        assert_eq!(am.to_rfc3339(), "2022-05-21T00:05:00+00:00");
        let noon = parse_llu_time("5/21/2022 12:00:00 PM").unwrap();
        assert_eq!(noon.to_rfc3339(), "2022-05-21T12:00:00+00:00");
    }

    #[test]
    fn login_outcomes() {
        let ok = r#"{"status":0,"data":{"user":{"id":"user-1"},"authTicket":{"token":"jwt","expires":1700000000,"duration":100},"messages":{},"notifications":{}}}"#;
        match parse_login(200, ok) {
            LoginParse::Session {
                token,
                user_id,
                expires_unix,
            } => {
                assert_eq!(token, "jwt");
                assert_eq!(user_id, "user-1");
                assert_eq!(expires_unix, 1_700_000_000);
            }
            _ => panic!("expected session"),
        }
        let redirect = r#"{"status":0,"data":{"redirect":true,"region":"eu"}}"#;
        match parse_login(200, redirect) {
            LoginParse::Redirect { region } => assert_eq!(region, "eu"),
            _ => panic!("expected redirect"),
        }
        match parse_login(
            200,
            r#"{"status":2,"error":{"message":"incorrect username/password"}}"#,
        ) {
            LoginParse::Failed(ApiError::BadCredentials) => {}
            other => panic!("unexpected {other:?}"),
        }
        let locked = r#"{"status":429,"data":{"code":60,"data":{"failures":3,"interval":60,"lockout":300},"message":"locked"}}"#;
        match parse_login(200, locked) {
            LoginParse::Failed(ApiError::Locked { seconds }) => assert_eq!(seconds, 300),
            other => panic!("unexpected {other:?}"),
        }
        match parse_login(
            200,
            r#"{"status":4,"data":{"step":{"type":"tou","componentName":"TermsOfUse"}}}"#,
        ) {
            LoginParse::Failed(ApiError::ActionRequired(step)) => assert_eq!(step, "tou"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn graph_and_connection_merge() {
        let connections = r#"{
            "status": 0,
            "data": [{
                "patientId": "patient-1",
                "firstName": "Alex",
                "lastName": "Rivera",
                "targetLow": 70,
                "targetHigh": 180,
                "sensor": {"a": 1652400270, "w": 60, "pt": 4},
                "alarmRules": {"f": {"th": 55}},
                "glucoseMeasurement": {
                    "FactoryTimestamp": "5/21/2022 1:38:50 PM",
                    "ValueInMgPerDl": 91,
                    "TrendArrow": 3,
                    "Value": 91,
                    "isHigh": false,
                    "isLow": false
                },
                "patientDevice": {"dtid": 40068, "ll": 70, "hl": 180}
            }]
        }"#;
        let graph = r#"{
            "status": 0,
            "data": {
                "connection": {"patientId": "patient-1", "targetLow": 70, "targetHigh": 180},
                "graphData": [
                    {"FactoryTimestamp": "5/21/2022 1:28:50 PM", "ValueInMgPerDl": 88, "Value": 88},
                    {"FactoryTimestamp": "5/21/2022 1:38:50 PM", "ValueInMgPerDl": 91, "Value": 91}
                ]
            },
            "ticket": {"token": "next", "expires": 1800000000}
        }"#;
        let parsed = parse_connections(connections).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].patient.label(), "Alex Rivera");
        let graph = parse_graph(graph).unwrap();
        assert_eq!(graph.readings.len(), 2);
        let merged = merge(parsed, 0, graph);
        assert_eq!(merged.snapshot.current_mgdl, Some(91.0));
        assert_eq!(merged.snapshot.trend, 3);
        assert_eq!(merged.snapshot.dtid, 40068);
        assert_eq!(merged.snapshot.sensor_started, Some(1_652_400_270));
        assert!(!merged.snapshot.mmol);
        assert_eq!(merged.token.as_deref(), Some("next"));
    }

    #[test]
    fn string_payload_is_protected() {
        let error = parse_graph(r#"{"status":0,"data":"ciphertext"}"#).unwrap_err();
        assert!(matches!(error, ApiError::Encrypted));
    }

    #[test]
    fn mmol_detection_uses_the_numbers() {
        assert!(!detect_mmol(91.0, 91.0));
        assert!(detect_mmol(5.1, 92.0));
        assert_eq!(base_url("us"), "https://api-us.libreview.io");
        assert_eq!(base_url("auto"), "https://api.libreview.io");
    }
}
