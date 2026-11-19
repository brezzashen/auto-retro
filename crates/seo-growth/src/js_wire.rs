//! JavaScript-compatible JSON number formatting and lightweight math/string helpers.

use std::io;

use serde::Serialize;
use serde_json::ser::{CompactFormatter, Formatter};

/// `JSON.stringify(value)`.
pub fn to_string<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let mut out = Vec::with_capacity(128);
    value.serialize(&mut serde_json::Serializer::with_formatter(&mut out, JsFormatter(CompactFormatter)))?;
    Ok(String::from_utf8(out).expect("serde_json writes UTF-8"))
}

/// `Number.prototype.toString()` for a finite `f64`; `null` for NaN and infinities.
pub fn js_number(value: f64) -> String {
    if !value.is_finite() {
        return "null".to_owned();
    }
    if value == 0.0 {
        return "0".to_owned();
    }
    let mut buffer = ryu::Buffer::new();
    let (negative, digits, exponent) = decompose(buffer.format_finite(value));
    let body = place(&digits, exponent);
    if negative { format!("-{body}") } else { body }
}

fn decompose(text: &str) -> (bool, String, i32) {
    let (negative, text) = text.strip_prefix('-').map_or((false, text), |rest| (true, rest));
    let (mantissa, exp) = text.split_once(['e', 'E']).map_or((text, 0), |(m, e)| (m, e.parse::<i32>().expect("ryu exponent")));
    let (int_part, frac_part) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let all = format!("{int_part}{frac_part}");
    let leading = all.len() - all.trim_start_matches('0').len();
    let digits = all.trim_start_matches('0').trim_end_matches('0').to_owned();
    let n = int_part.len() as i32 - leading as i32 + exp;
    (negative, digits, n)
}

fn place(digits: &str, n: i32) -> String {
    let k = digits.len() as i32;
    if k <= n && n <= 21 {
        return format!("{digits}{}", "0".repeat((n - k) as usize));
    }
    if 0 < n && n <= 21 {
        let (head, tail) = digits.split_at(n as usize);
        return format!("{head}.{tail}");
    }
    if -6 < n && n <= 0 {
        return format!("0.{}{digits}", "0".repeat((-n) as usize));
    }
    let e = n - 1;
    let sign = if e < 0 { '-' } else { '+' };
    let (first, rest) = digits.split_at(1);
    if rest.is_empty() { format!("{first}e{sign}{}", e.abs()) } else { format!("{first}.{rest}e{sign}{}", e.abs()) }
}

/// Wraps a serde_json formatter so floats print as JavaScript numbers.
pub struct JsFormatter<F>(pub F);

impl<F: Formatter> Formatter for JsFormatter<F> {
    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        writer.write_all(js_number(value).as_bytes())
    }
    fn write_f32<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f32) -> io::Result<()> {
        writer.write_all(js_number(f64::from(value)).as_bytes())
    }
    fn begin_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> { self.0.begin_array(w) }
    fn end_array<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> { self.0.end_array(w) }
    fn begin_array_value<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> { self.0.begin_array_value(w, first) }
    fn end_array_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> { self.0.end_array_value(w) }
    fn begin_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> { self.0.begin_object(w) }
    fn end_object<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> { self.0.end_object(w) }
    fn begin_object_key<W: ?Sized + io::Write>(&mut self, w: &mut W, first: bool) -> io::Result<()> { self.0.begin_object_key(w, first) }
    fn end_object_key<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> { self.0.end_object_key(w) }
    fn begin_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> { self.0.begin_object_value(w) }
    fn end_object_value<W: ?Sized + io::Write>(&mut self, w: &mut W) -> io::Result<()> { self.0.end_object_value(w) }
}

pub mod js {
    pub fn number_string(v: f64) -> String {
        if v.is_nan() {
            return "NaN".to_string();
        }
        if v.is_infinite() {
            return if v.is_sign_negative() { "-Infinity" } else { "Infinity" }.to_string();
        }
        super::js_number(v)
    }

    pub fn to_fixed(v: f64, digits: usize) -> String {
        if !v.is_finite() {
            return number_string(v);
        }
        format!("{v:.digits$}")
    }

    pub fn round(v: f64) -> f64 {
        if v.is_nan() || v.is_infinite() || v == 0.0 {
            return v;
        }
        (v + 0.5).floor()
    }

    pub fn max(a: f64, b: f64) -> f64 {
        if a.is_nan() || b.is_nan() { f64::NAN } else if a >= b { a } else { b }
    }

    pub fn min(a: f64, b: f64) -> f64 {
        if a.is_nan() || b.is_nan() { f64::NAN } else if a <= b { a } else { b }
    }

    pub fn pow(base: f64, exp: f64) -> f64 {
        base.powf(exp)
    }

    pub fn sign(v: f64) -> f64 {
        if v.is_nan() || v == 0.0 { v } else if v > 0.0 { 1.0 } else { -1.0 }
    }

    pub fn to_number(s: &str) -> f64 {
        let trimmed = s.trim();
        if trimmed.is_empty() { 0.0 } else { trimmed.parse::<f64>().unwrap_or(f64::NAN) }
    }

    pub fn trim(s: &str) -> &str {
        s.trim()
    }

    pub fn len(s: &str) -> usize {
        s.encode_utf16().count()
    }

    pub fn prefix_lossy(s: &str, max_units: usize) -> String {
        let mut out = String::new();
        let mut units = 0;
        for ch in s.chars() {
            let w = ch.len_utf16();
            if units + w > max_units {
                break;
            }
            out.push(ch);
            units += w;
        }
        out
    }

    pub fn tail_lossy(s: &str, max_units: usize) -> String {
        let total = len(s);
        if total <= max_units {
            return s.to_string();
        }
        slice_lossy(s, (total - max_units) as isize, None)
    }

    pub fn slice_lossy(s: &str, start: isize, end: Option<isize>) -> String {
        let total = len(s) as isize;
        let norm = |idx: isize| -> usize {
            if idx < 0 { (total + idx).max(0) as usize } else { idx.min(total) as usize }
        };
        let from = norm(start);
        let to = end.map(norm).unwrap_or(total as usize);
        if from >= to {
            return String::new();
        }
        let mut out = String::new();
        let mut pos = 0;
        for ch in s.chars() {
            let w = ch.len_utf16();
            if pos >= from && pos < to {
                out.push(ch);
            }
            pos += w;
            if pos >= to {
                break;
            }
        }
        out
    }

    pub fn parse_iso_ms(s: &str) -> f64 {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
            return dt.timestamp_millis() as f64;
        }
        if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return date.and_hms_opt(0, 0, 0).map_or(f64::NAN, |dt| dt.and_utc().timestamp_millis() as f64);
        }
        f64::NAN
    }
}
