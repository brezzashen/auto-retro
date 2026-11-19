//! `.claude/skills/seo-competitor/scripts/charts.ts`: Mermaid chart builders for the report renderer
//! (bar, grouped bar, line, pie and quadrant), with the same label cut-offs and value rounding.

use crate::js_wire::js;

/// `label(text, max)`: quoted, without quotes and brackets, cut to `max` UTF-16 units.
fn label(text: &str, max: usize) -> String {
    let cleaned: String = text.chars().filter(|c| !matches!(c, '"' | '[' | ']')).collect();
    format!("\"{}\"", js::prefix_lossy(&cleaned, max))
}

/// `axisMax(values)`: the top of a 0-based axis, 10% above the largest value, rounded up to a round step.
fn axis_max(values: &[f64]) -> f64 {
    let largest = values.iter().fold(1.0_f64, |acc, v| js::max(acc, *v));
    let top = largest * 1.1;
    let step = js::pow(10.0, top.log10().floor());
    (top / step).ceil() * step
}

/// `Math.round(v)` of each value, joined for a Mermaid array.
fn rounded_list(values: &[f64]) -> String {
    values.iter().map(|v| js::number_string(js::round(*v))).collect::<Vec<_>>().join(", ")
}

fn labels(categories: &[String]) -> String {
    categories.iter().map(|c| label(c, 28)).collect::<Vec<_>>().join(", ")
}

/// Horizontal bars; two or more series draw grouped bars, so name them in the title.
pub fn bars(title: &str, categories: &[String], series: &[Vec<f64>], y_label: &str, max: Option<f64>) -> String {
    let all: Vec<f64> = series.iter().flatten().copied().collect();
    let top = max.unwrap_or_else(|| axis_max(&all));
    let mut lines = vec![
        "xychart-beta horizontal".to_string(),
        format!("    title {}", label(title, 90)),
        format!("    x-axis [{}]", labels(categories)),
        format!("    y-axis {} 0 --> {}", label(y_label, 28), js::number_string(top)),
    ];
    lines.extend(series.iter().map(|s| format!("    bar [{}]", rounded_list(s))));
    lines.join("\n")
}

/// A line chart over the categories.
pub fn line(title: &str, categories: &[String], values: &[f64], y_label: &str) -> String {
    [
        "xychart-beta".to_string(),
        format!("    title {}", label(title, 90)),
        format!("    x-axis [{}]", labels(categories)),
        format!("    y-axis {} 0 --> {}", label(y_label, 28), js::number_string(axis_max(values))),
        format!("    line [{}]", rounded_list(values)),
    ]
    .join("\n")
}

/// A pie of the positive slices.
pub fn pie(title: &str, slices: &[(String, f64)]) -> String {
    let cleaned: String = title.chars().filter(|c| *c != '"' && *c != '\n').collect();
    let mut lines = vec!["pie showData".to_string(), format!("    title {cleaned}")];
    for (name, value) in slices.iter().filter(|(_, v)| *v > 0.0) {
        lines.push(format!("    {} : {}", label(name, 40), js::number_string(js::round(*value))));
    }
    lines.join("\n")
}

/// A quadrant chart: the axes, four quadrant names (top-right, top-left, bottom-left, bottom-right)
/// and the points, each clamped into 0.02-0.98.
pub struct Quadrant {
    pub title: String,
    pub x_axis: [String; 2],
    pub y_axis: [String; 2],
    pub quadrants: [String; 4],
    pub points: Vec<(String, f64, f64)>,
}

fn clamp01(v: f64) -> f64 {
    js::min(0.98, js::max(0.02, v))
}

pub fn quadrant(spec: &Quadrant) -> String {
    let mut names: Vec<String> = Vec::new();
    let mut lines = vec![
        "quadrantChart".to_string(),
        format!("    title {}", spec.title),
        format!("    x-axis {} --> {}", spec.x_axis[0], spec.x_axis[1]),
        format!("    y-axis {} --> {}", spec.y_axis[0], spec.y_axis[1]),
    ];
    lines.extend(spec.quadrants.iter().enumerate().map(|(i, q)| format!("    quadrant-{} {q}", i + 1)));
    for (raw_name, x, y) in &spec.points {
        let mut name: String = js::prefix_lossy(&raw_name.chars().filter(|c| !matches!(c, ':' | '"' | '[' | ']')).collect::<String>(), 30);
        while names.contains(&name) {
            name.push(' ');
        }
        lines.push(format!("    {name}: [{}, {}]", js::to_fixed(clamp01(*x), 2), js::to_fixed(clamp01(*y), 2)));
        names.push(name);
    }
    lines.join("\n")
}

/// Maps a positive value to 0-1 on a log scale between `lo` and `hi`.
pub fn log_scale(value: f64, lo: f64, hi: f64) -> f64 {
    if value <= lo {
        return 0.0;
    }
    js::min(1.0, (value / lo).log10() / (hi / lo).log10())
}
