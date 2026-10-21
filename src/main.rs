mod api;
mod chart;
mod model;
mod present;
mod preview;
mod session;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use slint::{ComponentHandle, ModelRc, VecModel, Weak};

slint::include_modules!();

struct AppState {
    session: session::Session,
    snapshot: Option<model::Snapshot>,
    preview: bool,
    signed_in: bool,
    refreshing: bool,
    banner: String,
    link_down: bool,
    chart_w: f32,
    chart_h: f32,
    last_status: Option<i32>,
    last_notify: Option<Instant>,
    sync_at: Option<DateTime<Utc>>,
}

fn main() -> Result<(), slint::PlatformError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("start async runtime");
    let handle = runtime.handle().clone();
    let http = api::http_client().ok();

    let ui = MainWindow::new()?;
    let session = session::load();
    let regions: Vec<slint::SharedString> = session::REGIONS
        .iter()
        .map(|(_, label)| (*label).into())
        .collect();
    ui.set_region_choices(ModelRc::new(VecModel::from(regions)));
    ui.set_email_text(session.email.clone().into());
    if session.remember {
        ui.set_password_text(session.password.clone().into());
    }
    ui.set_remember_checked(session.remember);
    ui.set_region_index(session::region_index(&session.region));
    ui.set_pinned(session.pinned);
    ui.set_notify_on(session.notify);
    ui.set_range_index(session::range_index(session.range_hours));
    ui.set_config_path(
        session::config_file()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
            .into(),
    );

    let state = Arc::new(Mutex::new(AppState {
        session,
        snapshot: None,
        preview: false,
        signed_in: false,
        refreshing: false,
        banner: String::new(),
        link_down: false,
        chart_w: 0.0,
        chart_h: 0.0,
        last_status: None,
        last_notify: None,
        sync_at: None,
    }));

    wire(&ui, &state, handle.clone(), http.clone());

    let boot_state = state.clone();
    let boot_handle = handle.clone();
    let boot_http = http.clone();
    let boot_ui = ui.as_weak();
    let boot = slint::Timer::default();
    boot.start(
        slint::TimerMode::SingleShot,
        Duration::from_millis(40),
        move || {
            on_boot(
                boot_ui.clone(),
                boot_state.clone(),
                boot_handle.clone(),
                boot_http.clone(),
            );
        },
    );

    let poll_state = state.clone();
    let poll_handle = handle;
    let poll_http = http;
    let poll_ui = ui.as_weak();
    let poll = slint::Timer::default();
    poll.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(60),
        move || {
            let ready = {
                let guard = poll_state.lock().unwrap();
                guard.signed_in
                    && !guard.preview
                    && guard.session.can_refresh()
                    && !guard.refreshing
            };
            if ready {
                start_sync(
                    poll_ui.clone(),
                    poll_state.clone(),
                    poll_handle.clone(),
                    poll_http.clone(),
                    false,
                );
            }
        },
    );

    ui.run()?;
    drop(poll);
    drop(boot);
    Ok(())
}

fn wire(
    ui: &MainWindow,
    state: &Arc<Mutex<AppState>>,
    handle: tokio::runtime::Handle,
    http: Option<reqwest::Client>,
) {
    let weak = ui.as_weak();
    let state_sign = state.clone();
    let handle_sign = handle.clone();
    let http_sign = http.clone();
    ui.on_sign_in(move |email, password, region, remember| {
        let Some(ui) = weak.upgrade() else { return };
        let email = email.trim().to_string();
        let password = password.to_string();
        if email.is_empty() || password.is_empty() {
            ui.set_login_error("Enter the email and password from the LibreLinkUp app.".into());
            return;
        }
        {
            let mut guard = state_sign.lock().unwrap();
            if guard.refreshing {
                return;
            }
            guard.session.email = email;
            guard.session.password = password;
            guard.session.remember = remember;
            guard.session.region = session::region_key(region);
            guard.session.base_url.clear();
            guard.session.token.clear();
            guard.session.user_id.clear();
            guard.session.expires_unix = 0;
            guard.preview = false;
        }
        start_sync(
            weak.clone(),
            state_sign.clone(),
            handle_sign.clone(),
            http_sign.clone(),
            true,
        );
    });

    let weak = ui.as_weak();
    let state_out = state.clone();
    ui.on_sign_out(move || {
        let Some(ui) = weak.upgrade() else { return };
        sign_out(&ui, &state_out);
    });

    let weak = ui.as_weak();
    let state_refresh = state.clone();
    let handle_refresh = handle.clone();
    let http_refresh = http.clone();
    ui.on_refresh(move || {
        let Some(ui) = weak.upgrade() else { return };
        let preview = state_refresh.lock().unwrap().preview;
        if preview {
            show_preview(&ui, &state_refresh);
            return;
        }
        start_sync(
            weak.clone(),
            state_refresh.clone(),
            handle_refresh.clone(),
            http_refresh.clone(),
            false,
        );
    });

    let weak = ui.as_weak();
    let state_preview = state.clone();
    ui.on_preview_sample(move || {
        let Some(ui) = weak.upgrade() else { return };
        show_preview(&ui, &state_preview);
    });

    let weak = ui.as_weak();
    let state_range = state.clone();
    ui.on_range_changed(move |index| {
        let Some(ui) = weak.upgrade() else { return };
        let mut guard = state_range.lock().unwrap();
        guard.session.range_hours = session::range_hours(index);
        let _ = session::save(&guard.session);
        let view = build_view(&guard);
        let size = (guard.chart_w, guard.chart_h);
        drop(guard);
        apply(&ui, &view, size.0, size.1);
    });

    let weak = ui.as_weak();
    let state_patient = state.clone();
    let handle_patient = handle.clone();
    let http_patient = http.clone();
    ui.on_patient_changed(move |index| {
        let Some(_ui) = weak.upgrade() else { return };
        let mut guard = state_patient.lock().unwrap();
        if guard.preview || guard.refreshing {
            return;
        }
        let Some(patient) = guard
            .snapshot
            .as_ref()
            .and_then(|s| s.patients.get(index as usize))
        else {
            return;
        };
        if guard.session.patient_id == patient.id {
            return;
        }
        guard.session.patient_id = patient.id.clone();
        let _ = session::save(&guard.session);
        drop(guard);
        start_sync(
            weak.clone(),
            state_patient.clone(),
            handle_patient.clone(),
            http_patient.clone(),
            false,
        );
    });

    let weak = ui.as_weak();
    let state_wear = state.clone();
    ui.on_wear_changed(move |days| {
        let Some(ui) = weak.upgrade() else { return };
        let mut guard = state_wear.lock().unwrap();
        guard.session.wear_days = days as u32;
        let _ = session::save(&guard.session);
        let view = build_view(&guard);
        let size = (guard.chart_w, guard.chart_h);
        drop(guard);
        apply(&ui, &view, size.0, size.1);
    });

    let weak = ui.as_weak();
    let state_notify = state.clone();
    ui.on_notify_changed(move |enabled| {
        let Some(ui) = weak.upgrade() else { return };
        let mut guard = state_notify.lock().unwrap();
        guard.session.notify = enabled;
        let _ = session::save(&guard.session);
        drop(guard);
        ui.set_notify_on(enabled);
    });

    let weak = ui.as_weak();
    let state_pin = state.clone();
    ui.on_pin_changed(move |pinned| {
        let Some(ui) = weak.upgrade() else { return };
        let mut guard = state_pin.lock().unwrap();
        guard.session.pinned = pinned;
        let _ = session::save(&guard.session);
        drop(guard);
        ui.set_pinned(pinned);
    });

    let weak = ui.as_weak();
    let state_chart = state.clone();
    ui.on_chart_resized(move |width, height| {
        let Some(ui) = weak.upgrade() else { return };
        let width = logical_px(width);
        let height = logical_px(height);
        let mut guard = state_chart.lock().unwrap();
        guard.chart_w = width;
        guard.chart_h = height;
        let view = build_view(&guard);
        drop(guard);
        draw(&ui, &view, width, height);
    });
}

fn on_boot(
    weak: Weak<MainWindow>,
    state: Arc<Mutex<AppState>>,
    handle: tokio::runtime::Handle,
    http: Option<reqwest::Client>,
) {
    let Some(ui) = weak.upgrade() else { return };
    if std::env::var("GLUCO_PREVIEW").ok().as_deref() == Some("1") {
        show_preview(&ui, &state);
        return;
    }
    let can_refresh = state.lock().unwrap().session.can_refresh();
    if can_refresh {
        start_sync(weak, state, handle, http, true);
    }
}

fn show_preview(ui: &MainWindow, state: &Mutex<AppState>) {
    let mut guard = state.lock().unwrap();
    guard.preview = true;
    guard.signed_in = true;
    guard.refreshing = false;
    guard.banner.clear();
    guard.link_down = false;
    guard.snapshot = Some(preview::sample(Utc::now()));
    guard.sync_at = Some(Utc::now());
    guard.last_status = None;
    let view = build_view(&guard);
    let size = (guard.chart_w, guard.chart_h);
    drop(guard);
    apply(ui, &view, size.0, size.1);
    ui.set_tab(0);
    ui.set_busy(false);
}

fn sign_out(ui: &MainWindow, state: &Mutex<AppState>) {
    let mut guard = state.lock().unwrap();
    let preview = guard.preview;
    guard.preview = false;
    guard.signed_in = false;
    guard.snapshot = None;
    guard.banner.clear();
    guard.link_down = false;
    guard.last_status = None;
    guard.refreshing = false;
    if !preview {
        guard.session.password.clear();
        guard.session.token.clear();
        guard.session.user_id.clear();
        guard.session.expires_unix = 0;
        guard.session.remember = false;
        let _ = session::save(&guard.session);
    }
    let email = guard.session.email.clone();
    drop(guard);
    ui.set_signed_in(false);
    ui.set_preview(false);
    ui.set_busy(false);
    ui.set_banner("".into());
    ui.set_password_text("".into());
    ui.set_email_text(email.into());
    ui.set_remember_checked(false);
    ui.set_login_error("".into());
}

fn start_sync(
    weak: Weak<MainWindow>,
    state: Arc<Mutex<AppState>>,
    handle: tokio::runtime::Handle,
    http: Option<reqwest::Client>,
    interactive: bool,
) {
    let Some(http) = http else {
        if let Some(ui) = weak.upgrade() {
            ui.set_busy(false);
            ui.set_login_error("The network client could not start on this computer.".into());
        }
        return;
    };
    let session = {
        let mut guard = state.lock().unwrap();
        if guard.refreshing {
            return;
        }
        guard.refreshing = true;
        if interactive {
            guard.preview = false;
        }
        guard.session.clone()
    };
    if let Some(ui) = weak.upgrade() {
        ui.set_busy(true);
        if interactive {
            ui.set_login_error("".into());
        }
    }
    let weak_done = weak.clone();
    let state_done = state.clone();
    handle.spawn(async move {
        let result = api::sync(&http, &session).await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(ui) = weak_done.upgrade() else {
                return;
            };
            finish(&ui, &state_done, result, interactive);
        });
    });
}

fn finish(
    ui: &MainWindow,
    state: &Mutex<AppState>,
    result: Result<api::Sync, api::ApiError>,
    interactive: bool,
) {
    let mut guard = state.lock().unwrap();
    guard.refreshing = false;
    match result {
        Ok(sync) => {
            if let Some(patient) = sync.snapshot.selected_patient() {
                guard.session.patient_id = patient.id.clone();
            }
            guard.session.base_url = sync.base_url;
            guard.session.token = sync.token;
            guard.session.user_id = sync.user_id;
            guard.session.expires_unix = sync.expires_unix;
            guard.snapshot = Some(sync.snapshot);
            guard.preview = false;
            guard.signed_in = true;
            guard.banner.clear();
            guard.link_down = false;
            guard.sync_at = Some(Utc::now());
            let _ = session::save(&guard.session);
            let view = build_view(&guard);
            let size = (guard.chart_w, guard.chart_h);
            maybe_notify(&mut guard, &view);
            drop(guard);
            apply(ui, &view, size.0, size.1);
            if interactive {
                ui.set_tab(0);
            }
            ui.set_busy(false);
            ui.set_login_error("".into());
        }
        Err(error) => {
            let auth = matches!(
                error,
                api::ApiError::BadCredentials
                    | api::ApiError::Locked { .. }
                    | api::ApiError::ActionRequired(_)
                    | api::ApiError::VersionRejected
            );
            let message = api::explain(&error);
            if auth {
                guard.session.token.clear();
                guard.signed_in = false;
                guard.banner.clear();
                let _ = session::save(&guard.session);
                drop(guard);
                ui.set_signed_in(false);
                ui.set_busy(false);
                ui.set_banner("".into());
                ui.set_login_error(message.into());
            } else if guard.signed_in && !interactive {
                guard.banner = message;
                guard.link_down = true;
                let view = build_view(&guard);
                let size = (guard.chart_w, guard.chart_h);
                drop(guard);
                apply(ui, &view, size.0, size.1);
                ui.set_busy(false);
            } else {
                drop(guard);
                ui.set_signed_in(false);
                ui.set_busy(false);
                ui.set_login_error(message.into());
            }
        }
    }
}

fn build_view(state: &AppState) -> present::View {
    let sync_text = match state.sync_at {
        Some(at) => format!("Last sync {}", present::format_clock(at)),
        None => "Not synced yet".into(),
    };
    present::present(
        state.snapshot.as_ref(),
        &state.session,
        state.preview,
        state.signed_in,
        &state.banner,
        state.link_down,
        &sync_text,
        Utc::now(),
    )
}

fn apply(ui: &MainWindow, view: &present::View, width: f32, height: f32) {
    ui.set_signed_in(view.signed_in);
    ui.set_preview(view.preview);
    ui.set_pinned(view.pinned);
    ui.set_notify_on(view.notify);
    ui.set_device_name(view.device_name.clone().into());
    ui.set_patient_name(view.patient_name.clone().into());
    let names: Vec<slint::SharedString> = view.patients.iter().cloned().map(Into::into).collect();
    ui.set_patient_names(ModelRc::new(VecModel::from(names)));
    ui.set_patient_count(view.patients.len() as i32);
    ui.set_patient_index(view.patient_index);
    ui.set_glucose_text(view.glucose_text.clone().into());
    ui.set_unit_text(view.unit_text.clone().into());
    ui.set_status_kind(view.status_kind);
    ui.set_status_text(view.status_text.clone().into());
    ui.set_as_of(view.as_of.clone().into());
    ui.set_sensor_text(view.sensor_text.clone().into());
    ui.set_sensor_level(view.sensor_level);
    ui.set_trend(view.trend);
    ui.set_link_state(view.link_state);
    ui.set_banner(view.banner.clone().into());
    ui.set_range_index(view.range_index);
    ui.set_wear_days(view.wear_days);
    ui.set_host_label(view.host_label.clone().into());
    ui.set_config_path(view.config_path.clone().into());
    ui.set_sync_text(view.sync_text.clone().into());
    ui.set_low_threshold(view.low_threshold.clone().into());
    ui.set_high_threshold(view.high_threshold.clone().into());
    ui.set_very_low_threshold(view.very_low_threshold.clone().into());
    ui.set_urgent_threshold(view.urgent_threshold.clone().into());
    draw(ui, view, width, height);
}

fn draw(ui: &MainWindow, view: &present::View, width: f32, height: f32) {
    let now = Utc::now();
    let hours = session::range_hours(view.range_index);
    let stats = chart::summarize(&view.readings, now, hours, view.low, view.high);
    apply_stats(ui, view, &stats, hours);
    if width < 2.0 || height < 2.0 {
        ui.set_line_in("".into());
        ui.set_line_low("".into());
        ui.set_line_high("".into());
        ui.set_dash_path("".into());
        ui.set_dot_visible(false);
        ui.set_chart_empty(false);
        return;
    }
    let drawn = chart::plot(
        &view.readings,
        now,
        hours,
        view.low,
        view.high,
        view.urgent_high,
        width,
        height,
        view.mmol,
    );
    ui.set_line_in(drawn.in_path.into());
    ui.set_line_low(drawn.low_path.into());
    ui.set_line_high(drawn.high_path.into());
    ui.set_dash_path(drawn.dash_path.into());
    ui.set_band_top(drawn.band_top);
    ui.set_band_height(drawn.band_height);
    ui.set_dot_x(drawn.dot_x);
    ui.set_dot_y(drawn.dot_y);
    ui.set_dot_visible(drawn.dot_visible);
    ui.set_chart_empty(drawn.empty);
    ui.set_y_labels(tick_model(drawn.y_labels));
    ui.set_x_labels(tick_model(drawn.x_labels));
}

fn apply_stats(ui: &MainWindow, view: &present::View, stats: &chart::Stats, hours: u32) {
    if stats.count == 0 {
        ui.set_avg_text("--".into());
        ui.set_min_text("--".into());
        ui.set_max_text("--".into());
        ui.set_in_text("--".into());
        ui.set_low_pct(0.0);
        ui.set_in_pct(0.0);
        ui.set_high_pct(0.0);
        ui.set_low_caption("Low".into());
        ui.set_high_caption("High".into());
        ui.set_count_text("No readings in this window".into());
    } else {
        ui.set_avg_text(present::format_number(stats.avg_mgdl, view.mmol).into());
        ui.set_min_text(present::format_number(stats.min_mgdl, view.mmol).into());
        ui.set_max_text(present::format_number(stats.max_mgdl, view.mmol).into());
        ui.set_in_text(format!("{}%", stats.in_pct.round() as i32).into());
        ui.set_low_pct(stats.low_pct as f32);
        ui.set_in_pct(stats.in_pct as f32);
        ui.set_high_pct(stats.high_pct as f32);
        ui.set_low_caption(format!("Low {}%", stats.low_pct.round() as i32).into());
        ui.set_high_caption(format!("High {}%", stats.high_pct.round() as i32).into());
        ui.set_count_text(format!("{} readings", stats.count).into());
    }
    let coverage = if view.preview {
        "Sample history for the layout. Not a sensor.".to_string()
    } else if stats.count == 0 {
        "LibreLinkUp did not send readings for this window.".to_string()
    } else if stats.span_hours + 1.0 < f64::from(hours) {
        format!(
            "LibreLinkUp sent about {:.0} h of history. This window is longer than that. Times use this computer's clock.",
            stats.span_hours.max(0.0)
        )
    } else {
        "Times use this computer's clock. LibreLinkUp can lag the sensor by a minute or more."
            .to_string()
    };
    ui.set_coverage_text(coverage.into());
}

fn tick_model(ticks: Vec<chart::Tick>) -> ModelRc<TickLabel> {
    let labels: Vec<TickLabel> = ticks
        .into_iter()
        .map(|tick| TickLabel {
            text: tick.text.into(),
            pos: tick.pos,
        })
        .collect();
    ModelRc::new(VecModel::from(labels))
}

fn maybe_notify(state: &mut AppState, view: &present::View) {
    let kind = view.status_kind;
    let previous = state.last_status.replace(kind);
    if state.preview || !state.session.notify || !view.signed_in {
        return;
    }
    let Some(previous) = previous else { return };
    if previous == kind || kind == 0 || kind == 1 {
        return;
    }
    let urgent = kind == 5;
    if !urgent {
        if let Some(when) = state.last_notify {
            if when.elapsed() < Duration::from_secs(300) {
                return;
            }
        }
    }
    state.last_notify = Some(Instant::now());
    let summary = view.status_text.clone();
    let body = format!("{} {}", view.glucose_text, view.unit_text);
    std::thread::spawn(move || {
        let mut note = notify_rust::Notification::new();
        note.summary(&summary).body(&body).appname("Gluco");
        if urgent {
            note.urgency(notify_rust::Urgency::Critical);
        }
        let _ = note.show();
    });
}

fn logical_px<T: Into<f32>>(value: T) -> f32 {
    value.into()
}
