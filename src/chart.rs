//! Glucose chart geometry.
//!
//! The vertical scale matches the Libre home graph: labeled ticks from 50 to 350 mg/dL,
//! with a little padding above and below so a reading on a tick is not clipped.

use chrono::{DateTime, Duration, Local, Timelike, Utc};

use crate::model::Reading;

const Y_MIN_MGDL: f64 = 40.0;
const Y_MAX_MGDL: f64 = 360.0;
const MGDL_PER_MMOL: f64 = 18.0182;
const GAP_SECONDS: i64 = 7 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    Low,
    In,
    High,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    pub text: String,
    pub pos: f32,
}

#[derive(Clone, Debug)]
pub struct Plot {
    pub low_path: String,
    pub in_path: String,
    pub high_path: String,
    pub dash_path: String,
    pub band_top: f32,
    pub band_height: f32,
    pub dot_x: f32,
    pub dot_y: f32,
    pub dot_visible: bool,
    pub y_labels: Vec<Tick>,
    pub x_labels: Vec<Tick>,
    pub empty: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stats {
    pub count: usize,
    pub avg_mgdl: f64,
    pub min_mgdl: f64,
    pub max_mgdl: f64,
    pub low_pct: f64,
    pub in_pct: f64,
    pub high_pct: f64,
    pub span_hours: f64,
}

impl Stats {
    fn empty() -> Self {
        Self {
            count: 0,
            avg_mgdl: 0.0,
            min_mgdl: 0.0,
            max_mgdl: 0.0,
            low_pct: 0.0,
            in_pct: 0.0,
            high_pct: 0.0,
            span_hours: 0.0,
        }
    }
}

pub fn plot(
    readings: &[Reading],
    now: DateTime<Utc>,
    hours: u32,
    low: f64,
    high: f64,
    urgent_high: f64,
    width: f32,
    height: f32,
    mmol: bool,
) -> Plot {
    let mut plot = Plot {
        low_path: String::new(),
        in_path: String::new(),
        high_path: String::new(),
        dash_path: String::new(),
        band_top: 0.0,
        band_height: 0.0,
        dot_x: 0.0,
        dot_y: 0.0,
        dot_visible: false,
        y_labels: y_labels(height, mmol),
        x_labels: Vec::new(),
        empty: true,
    };
    if width < 2.0 || height < 2.0 {
        return plot;
    }

    let (low, high) = ordered_targets(low, high);
    let y_high = y_of(high, height, mmol);
    let y_low = y_of(low, height, mmol);
    plot.band_top = (y_high / height).clamp(0.0, 1.0);
    plot.band_height = ((y_low - y_high) / height).clamp(0.0, 1.0);

    let y_urgent = y_of(urgent_high, height, mmol);
    plot.dash_path = dashes(y_urgent, width);

    let hours = hours.clamp(1, 48);
    let end = now;
    let start = now - Duration::hours(i64::from(hours));
    plot.x_labels = x_labels(start, end, hours, width);

    let points = window_points(readings, start, end);
    if points.is_empty() {
        return plot;
    }
    plot.empty = false;

    let mut pens = [Pen::default(), Pen::default(), Pen::default()];
    for pair in points.windows(2) {
        let prev = &pair[0];
        let next = &pair[1];
        let gap = (next.at - prev.at).num_seconds();
        if gap > GAP_SECONDS {
            continue;
        }
        let x0 = x_of(prev.at, start, end, width);
        let x1 = x_of(next.at, start, end, width);
        let y0 = y_of(prev.mgdl, height, mmol);
        let y1 = y_of(next.mgdl, height, mmol);
        emit_segment(&mut pens, x0, y0, prev.mgdl, x1, y1, next.mgdl, low, high);
    }
    let [low_pen, in_pen, high_pen] = pens;
    plot.low_path = low_pen.buf;
    plot.in_path = in_pen.buf;
    plot.high_path = high_pen.buf;

    if let Some(last) = points.last() {
        plot.dot_visible = true;
        plot.dot_x = x_of(last.at, start, end, width) / width;
        plot.dot_y = y_of(last.mgdl, height, mmol) / height;
    }
    plot
}

pub fn summarize(
    readings: &[Reading],
    now: DateTime<Utc>,
    hours: u32,
    low: f64,
    high: f64,
) -> Stats {
    let (low, high) = ordered_targets(low, high);
    let start = now - Duration::hours(i64::from(hours.clamp(1, 48)));
    let points = window_points(readings, start, now);
    if points.is_empty() {
        return Stats::empty();
    }

    let mut sum = 0.0;
    let mut min_v = f64::MAX;
    let mut max_v = f64::MIN;
    let mut low_s = 0.0;
    let mut in_s = 0.0;
    let mut high_s = 0.0;
    for (i, point) in points.iter().enumerate() {
        sum += point.mgdl;
        min_v = min_v.min(point.mgdl);
        max_v = max_v.max(point.mgdl);
        let seconds = if let Some(next) = points.get(i + 1) {
            (next.at - point.at).num_seconds().clamp(0, 10 * 60) as f64
        } else {
            60.0
        };
        match zone(point.mgdl, low, high) {
            Zone::Low => low_s += seconds,
            Zone::In => in_s += seconds,
            Zone::High => high_s += seconds,
        }
    }
    let total = (low_s + in_s + high_s).max(1.0);
    let span = if points.len() >= 2 {
        (points.last().unwrap().at - points[0].at)
            .num_seconds()
            .max(0) as f64
            / 3600.0
    } else {
        0.0
    };
    Stats {
        count: points.len(),
        avg_mgdl: sum / points.len() as f64,
        min_mgdl: min_v,
        max_mgdl: max_v,
        low_pct: low_s / total * 100.0,
        in_pct: in_s / total * 100.0,
        high_pct: high_s / total * 100.0,
        span_hours: span,
    }
}

fn ordered_targets(low: f64, high: f64) -> (f64, f64) {
    let low = if low.is_finite() { low } else { 70.0 };
    let high = if high.is_finite() { high } else { 180.0 };
    if high > low {
        (low, high)
    } else {
        (70.0, 180.0)
    }
}

fn zone(v: f64, low: f64, high: f64) -> Zone {
    if v < low {
        Zone::Low
    } else if v > high {
        Zone::High
    } else {
        Zone::In
    }
}

fn y_of(mgdl: f64, height: f32, mmol: bool) -> f32 {
    let (ymin, ymax, value) = if mmol {
        (
            Y_MIN_MGDL / MGDL_PER_MMOL,
            Y_MAX_MGDL / MGDL_PER_MMOL,
            mgdl / MGDL_PER_MMOL,
        )
    } else {
        (Y_MIN_MGDL, Y_MAX_MGDL, mgdl)
    };
    let t = ((value - ymin) / (ymax - ymin)).clamp(0.0, 1.0);
    (1.0 - t as f32) * height
}

fn x_of(at: DateTime<Utc>, start: DateTime<Utc>, end: DateTime<Utc>, width: f32) -> f32 {
    let span = (end - start).num_milliseconds().max(1) as f64;
    let pos = (at - start).num_milliseconds() as f64 / span;
    (pos.clamp(0.0, 1.0) as f32) * width
}

fn y_labels(height: f32, mmol: bool) -> Vec<Tick> {
    const TICKS: [f64; 7] = [50.0, 100.0, 150.0, 200.0, 250.0, 300.0, 350.0];
    TICKS
        .into_iter()
        .map(|mgdl| {
            let text = if mmol {
                format!("{:.1}", mgdl / MGDL_PER_MMOL)
            } else {
                format!("{}", mgdl as i32)
            };
            let pos = if height < 2.0 {
                0.0
            } else {
                y_of(mgdl, height, mmol) / height
            };
            Tick { text, pos }
        })
        .collect()
}

fn x_labels(start: DateTime<Utc>, end: DateTime<Utc>, hours: u32, width: f32) -> Vec<Tick> {
    let step = match hours {
        0..=3 => 1,
        4..=6 => 2,
        7..=12 => 3,
        _ => 6,
    };
    let start_local = start.with_timezone(&Local);
    let end_local = end.with_timezone(&Local);
    // The window usually starts mid-hour. Walk clock hours, not offsets from `start`,
    // or a 6:32 window would never hit minute == 0 and the axis would be blank.
    let mut cursor = start_local
        .with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .unwrap_or(start_local);
    if cursor < start_local {
        cursor += Duration::hours(1);
    }
    let mut guard = 0;
    while i64::from(cursor.hour()) % i64::from(step) != 0 && guard < 24 {
        cursor += Duration::hours(1);
        guard += 1;
    }
    let mut labels = Vec::new();
    for _ in 0..16 {
        if cursor > end_local {
            break;
        }
        let at = cursor.with_timezone(&Utc);
        let pos = x_of(at, start, end, width) / width;
        if (0.04..0.97).contains(&pos) {
            labels.push(Tick {
                text: format_hour(cursor.hour()),
                pos,
            });
        }
        cursor += Duration::hours(i64::from(step));
    }
    labels
}

fn format_hour(hour: u32) -> String {
    let (h, suffix) = match hour {
        0 => (12, "am"),
        1..=11 => (hour, "am"),
        12 => (12, "pm"),
        h => (h - 12, "pm"),
    };
    format!("{h}{suffix}")
}

fn window_points(readings: &[Reading], start: DateTime<Utc>, end: DateTime<Utc>) -> Vec<Reading> {
    let mut points: Vec<Reading> = readings
        .iter()
        .filter(|r| r.mgdl.is_finite() && r.mgdl > 0.0 && r.at >= start && r.at <= end)
        .cloned()
        .collect();
    points.sort_by_key(|r| r.at);
    points.reverse();
    points.dedup_by_key(|r| r.at);
    points.reverse();
    points
}

#[derive(Default)]
struct Pen {
    buf: String,
    open: bool,
    last_x: f32,
    last_y: f32,
}

fn pen_line(pen: &mut Pen, x0: f32, y0: f32, x1: f32, y1: f32) {
    let continuous = pen.open && (pen.last_x - x0).abs() < 0.8 && (pen.last_y - y0).abs() < 0.8;
    if continuous {
        pen.buf.push_str(&format!("L {x1:.2} {y1:.2} "));
    } else {
        pen.buf
            .push_str(&format!("M {x0:.2} {y0:.2} L {x1:.2} {y1:.2} "));
        pen.open = true;
    }
    pen.last_x = x1;
    pen.last_y = y1;
}

fn emit_segment(
    pens: &mut [Pen; 3],
    x0: f32,
    y0: f32,
    v0: f64,
    x1: f32,
    y1: f32,
    v1: f64,
    low: f64,
    high: f64,
) {
    let mut cuts = Vec::new();
    if (v1 - v0).abs() > f64::EPSILON {
        for threshold in [low, high] {
            if (v0 - threshold) * (v1 - threshold) < 0.0 {
                cuts.push(threshold);
            }
        }
        cuts.sort_by(|a, b| {
            (*a - v0)
                .abs()
                .partial_cmp(&(*b - v0).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    let mut px = x0;
    let mut py = y0;
    let mut pv = v0;
    for threshold in cuts {
        let frac = ((threshold - v0) / (v1 - v0)) as f32;
        let x = x0 + (x1 - x0) * frac;
        let y = y0 + (y1 - y0) * frac;
        let z = zone((pv + threshold) / 2.0, low, high);
        pen_line(&mut pens[z as usize], px, py, x, y);
        px = x;
        py = y;
        pv = threshold;
    }
    let z = zone((pv + v1) / 2.0, low, high);
    pen_line(&mut pens[z as usize], px, py, x1, y1);
}

fn dashes(y: f32, width: f32) -> String {
    let mut path = String::new();
    let mut x = 0.0;
    while x < width {
        let x2 = (x + 7.0).min(width);
        path.push_str(&format!("M {x:.1} {y:.1} L {x2:.1} {y:.1} "));
        x += 12.0;
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at_offset(base: DateTime<Utc>, minutes: i64) -> DateTime<Utc> {
        base + Duration::minutes(minutes)
    }

    #[test]
    fn low_reading_is_split_from_in_range() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        // Connected samples stay inside the 7-minute gap that breaks the trace.
        let readings = vec![
            Reading {
                at: at_offset(now, -8),
                mgdl: 60.0,
            },
            Reading {
                at: at_offset(now, -4),
                mgdl: 100.0,
            },
            Reading {
                at: now,
                mgdl: 91.0,
            },
        ];
        let plot = plot(&readings, now, 1, 70.0, 180.0, 250.0, 200.0, 100.0, false);
        assert!(!plot.low_path.is_empty(), "below-range segment");
        assert!(!plot.in_path.is_empty(), "in-range segment");
        assert!(plot.dot_visible);
        assert!((plot.dot_x - 1.0).abs() < 0.02);
    }

    #[test]
    fn gaps_break_the_line() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let readings = vec![
            Reading {
                at: at_offset(now, -40),
                mgdl: 100.0,
            },
            Reading {
                at: at_offset(now, -36),
                mgdl: 102.0,
            },
            Reading {
                at: at_offset(now, -5),
                mgdl: 110.0,
            },
            Reading {
                at: now,
                mgdl: 108.0,
            },
        ];
        let plot = plot(&readings, now, 1, 70.0, 180.0, 250.0, 300.0, 120.0, false);
        let moves = plot.in_path.matches("M ").count();
        assert!(moves >= 2, "path was {moves}: {}", plot.in_path);
    }

    #[test]
    fn x_labels_use_clock_hours_when_the_window_starts_mid_hour() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let readings = vec![Reading {
            at: now,
            mgdl: 91.0,
        }];
        let plot = plot(&readings, now, 12, 70.0, 180.0, 250.0, 300.0, 200.0, false);
        assert!(
            plot.x_labels.len() >= 3,
            "expected clock labels, got {:?}",
            plot.x_labels
        );
        for tick in &plot.x_labels {
            assert!(
                tick.text.ends_with("am") || tick.text.ends_with("pm"),
                "{}",
                tick.text
            );
            assert!((0.04..0.97).contains(&tick.pos), "{}", tick.pos);
        }
    }

    #[test]
    fn y_position_puts_higher_glucose_nearer_the_top() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let readings = vec![Reading {
            at: now,
            mgdl: 91.0,
        }];
        let plot = plot(&readings, now, 12, 70.0, 180.0, 250.0, 100.0, 100.0, false);
        assert!(
            plot.dot_y > 0.7 && plot.dot_y < 0.95,
            "dot y {}",
            plot.dot_y
        );
        let high_tick = plot.y_labels.iter().find(|t| t.text == "350").unwrap();
        let low_tick = plot.y_labels.iter().find(|t| t.text == "50").unwrap();
        assert!(high_tick.pos < low_tick.pos);
    }

    #[test]
    fn summary_counts_time_below_range() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let readings = vec![
            Reading {
                at: at_offset(now, -30),
                mgdl: 60.0,
            },
            Reading {
                at: at_offset(now, -20),
                mgdl: 60.0,
            },
            Reading {
                at: at_offset(now, -10),
                mgdl: 120.0,
            },
            Reading {
                at: now,
                mgdl: 120.0,
            },
        ];
        let stats = summarize(&readings, now, 1, 70.0, 180.0);
        assert_eq!(stats.count, 4);
        assert!(
            stats.low_pct > 20.0 && stats.low_pct < 80.0,
            "{}",
            stats.low_pct
        );
        assert!((stats.min_mgdl - 60.0).abs() < 0.01);
        assert!((stats.max_mgdl - 120.0).abs() < 0.01);
    }
}
