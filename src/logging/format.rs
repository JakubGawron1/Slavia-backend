//! Format logów Slavia — tryb pełny lub kompaktowy (HF Spaces).

use std::fmt;
use std::sync::OnceLock;

use chrono::Utc;

use tracing::field::{Field, Visit};
use tracing::Level;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::registry::LookupSpan;

static LOG_STYLE: OnceLock<LogStyle> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogStyle {
    /// `[LEVEL]:    {where}   (because)   {[fix]} extra | fields…`
    Standard,
    /// Jedna linia + kontynuacja dla WARN/ERROR (czytelne w HF Space Logs).
    HfCompact,
}

pub fn log_style() -> LogStyle {
    *LOG_STYLE.get_or_init(detect_log_style)
}

fn detect_log_style() -> LogStyle {
    log_style_from_env(
        std::env::var("SLAVIA_LOG_STYLE").ok().as_deref(),
        std::env::var("SPACE_ID").is_ok()
            || std::env::var("SYSTEM")
                .map(|v| v.eq_ignore_ascii_case("spaces"))
                .unwrap_or(false),
    )
}

fn log_style_from_env(style_env: Option<&str>, hf_detected: bool) -> LogStyle {
    match style_env.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("compact") | Some("hf") => LogStyle::HfCompact,
        Some("full") | Some("standard") => LogStyle::Standard,
        _ if hf_detected => LogStyle::HfCompact,
        _ => LogStyle::Standard,
    }
}

pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Składa linię logu (używane też w testach / dokumentacji).
pub fn format_line(level: &str, wh: &str, because: &str, fix: &str, extra: Option<&str>) -> String {
    let mut out = format!("[{level}]:    {{{wh}}}   ({because})   {{[{fix}]}}");
    if let Some(x) = extra {
        let t = x.trim();
        if !t.is_empty() {
            out.push(' ');
            out.push_str(t);
        }
    }
    out
}

fn format_hf_compact(
    level: &str,
    wh: &str,
    because: &str,
    fix: &str,
    http_method: Option<&str>,
    http_path: Option<&str>,
    status: Option<&str>,
    latency_ms: Option<&str>,
    request_id: Option<&str>,
    other_extras: &[(String, String)],
) -> String {
    let lvl = match level {
        "ERROR" => "ERR",
        "WARN" => "WRN",
        "INFO" => "INF",
        "DEBUG" => "DBG",
        _ => level,
    };

    let mut head = format!("{lvl} | {wh}");

    if let (Some(m), Some(p)) = (http_method, http_path) {
        head.push_str(&format!(" | {m} {p}"));
        if let Some(s) = status {
            head.push_str(&format!(" → {s}"));
        }
        if let Some(ms) = latency_ms {
            head.push_str(&format!(" ({ms}ms)"));
        }
    } else if !other_extras.is_empty() {
        let preview: Vec<_> = other_extras
            .iter()
            .take(3)
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        head.push_str(&format!(" | {}", preview.join(" ")));
    }

    let is_http = http_method.is_some();
    let is_errorish = matches!(level, "ERROR" | "WARN");

    if is_errorish {
        let mut out = format!("{head}\n  cause:  {because}\n  action: {fix}");
        if let Some(rid) = request_id.filter(|r| *r != "-") {
            out.push_str(&format!("\n  id:     {rid}"));
        } else if !is_http && !other_extras.is_empty() {
            let tail: Vec<_> = other_extras
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            out.push_str(&format!("\n  extra:  {}", tail.join(", ")));
        }
        out
    } else {
        if let Some(rid) = request_id.filter(|r| *r != "-") {
            head.push_str(&format!(" | id={rid}"));
        }
        if !is_http {
            head.push_str(&format!(" | {because}"));
        }
        head
    }
}

#[derive(Default)]
struct SlaviaFieldVisitor {
    where_: Option<String>,
    because: Option<String>,
    fix: Option<String>,
    extra: Option<String>,
    message: Option<String>,
    http_method: Option<String>,
    http_path: Option<String>,
    request_id: Option<String>,
    status: Option<String>,
    latency_ms: Option<String>,
    extras: Vec<(String, String)>,
}

impl SlaviaFieldVisitor {
    fn store_str(&mut self, name: &str, value: &str) {
        match name {
            "slavia_where" => self.where_ = Some(value.to_string()),
            "slavia_because" => self.because = Some(value.to_string()),
            "slavia_fix" => self.fix = Some(value.to_string()),
            "slavia_extra" => self.extra = Some(value.to_string()),
            "message" => self.message = Some(value.to_string()),
            "http_method" => self.http_method = Some(value.to_string()),
            "http_path" => self.http_path = Some(value.to_string()),
            "request_id" => self.request_id = Some(value.to_string()),
            "status" => self.status = Some(value.to_string()),
            "latency_ms" => self.latency_ms = Some(value.to_string()),
            _ if name.starts_with("slavia_") => {}
            _ => self.extras.push((name.to_string(), value.to_string())),
        }
    }

    fn store_debug(&mut self, name: &str, value: &dyn fmt::Debug) {
        let rendered = format!("{value:?}");
        let trimmed = rendered.trim_matches('"');
        self.store_str(name, trimmed);
    }
}

impl Visit for SlaviaFieldVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.store_str(field.name(), value);
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.store_debug(field.name(), value);
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "latency_ms" {
            self.latency_ms = Some(value.to_string());
        } else if field.name() == "status" {
            self.status = Some(value.to_string());
        } else {
            self.extras
                .push((field.name().to_string(), value.to_string()));
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "latency_ms" {
            self.latency_ms = Some(value.to_string());
        } else if field.name() == "status" {
            self.status = Some(value.to_string());
        } else {
            self.extras
                .push((field.name().to_string(), value.to_string()));
        }
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.extras
            .push((field.name().to_string(), value.to_string()));
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.extras
            .push((field.name().to_string(), value.to_string()));
    }
}

fn level_tag(level: &Level) -> &'static str {
    match *level {
        Level::ERROR => "ERROR",
        Level::WARN => "WARN",
        Level::INFO => "INFO",
        Level::DEBUG => "DEBUG",
        Level::TRACE => "TRACE",
    }
}

fn write_extras(writer: &mut Writer<'_>, pairs: &[(String, String)]) -> fmt::Result {
    if pairs.is_empty() {
        return Ok(());
    }
    write!(writer, " |")?;
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            write!(writer, ",")?;
        }
        write!(writer, " {k}={v}")?;
    }
    Ok(())
}

fn hf_timestamp_prefix() -> String {
    Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

fn format_legacy_compact(level: &str, msg: &str, extras: &[(String, String)]) -> String {
    let mut line = format!("{level} | legacy | {msg}");
    if !extras.is_empty() {
        let tail: Vec<_> = extras
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        line.push_str(&format!(" | {}", tail.join(" ")));
    }
    line
}

pub struct SlaviaEventFormatter {
    style: LogStyle,
}

impl SlaviaEventFormatter {
    pub fn new() -> Self {
        Self {
            style: log_style(),
        }
    }
}

impl Default for SlaviaEventFormatter {
    fn default() -> Self {
        Self::new()
    }
}

impl<S, N> FormatEvent<S, N> for SlaviaEventFormatter
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &tracing::Event<'_>,
    ) -> fmt::Result {
        let meta = event.metadata();
        let level = level_tag(meta.level());

        let mut fields = SlaviaFieldVisitor::default();
        event.record(&mut fields);

        if let (Some(w), Some(b), Some(f)) = (&fields.where_, &fields.because, &fields.fix) {
            match self.style {
                LogStyle::HfCompact => {
                    let ts = hf_timestamp_prefix();
                    let body = format_hf_compact(
                        level,
                        w,
                        b,
                        f,
                        fields.http_method.as_deref(),
                        fields.http_path.as_deref(),
                        fields.status.as_deref(),
                        fields.latency_ms.as_deref(),
                        fields.request_id.as_deref(),
                        &fields.extras,
                    );
                    write!(writer, "{ts} {body}")?;
                }
                LogStyle::Standard => {
                    let line = format_line(level, w, b, f, fields.extra.as_deref());
                    write!(writer, "{line}")?;
                    let mut tail = fields.extras.clone();
                    if let Some(m) = fields.http_method {
                        tail.insert(0, ("http_method".into(), m));
                    }
                    if let Some(p) = fields.http_path {
                        tail.insert(0, ("http_path".into(), p));
                    }
                    if let Some(rid) = fields.request_id {
                        tail.push(("request_id".into(), rid));
                    }
                    if let Some(s) = fields.status {
                        tail.push(("status".into(), s));
                    }
                    if let Some(ms) = fields.latency_ms {
                        tail.push(("latency_ms".into(), ms));
                    }
                    write_extras(&mut writer, &tail)?;
                }
            }
        } else if let Some(msg) = fields.message.filter(|m| !m.is_empty()) {
            match self.style {
                LogStyle::HfCompact => {
                    let ts = hf_timestamp_prefix();
                    let line = format_legacy_compact(level, &msg, &fields.extras);
                    write!(writer, "{ts} {line}")?;
                }
                LogStyle::Standard => {
                    write!(
                        writer,
                        "[{level}]:    {{legacy}}   (unstructured log)   {{[use slavia_*! macros]}} {msg}"
                    )?;
                    write_extras(&mut writer, &fields.extras)?;
                }
            }
        } else if !fields.extras.is_empty() {
            match self.style {
                LogStyle::HfCompact => {
                    let tail: Vec<_> = fields
                        .extras
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect();
                    write!(writer, "{level} | {tail}", tail = tail.join(" "))?;
                }
                LogStyle::Standard => {
                    write!(writer, "[{level}]:")?;
                    write_extras(&mut writer, &fields.extras)?;
                }
            }
        } else {
            write!(
                writer,
                "[{level}]:    {{?}}   (empty event)   {{[use slavia_*! macros]}}"
            )?;
        }

        writeln!(writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_line_matches_style() {
        let s = format_line(
            "ERROR",
            "main.rs",
            "not enough memory",
            "add more memory",
            Some("sometimes i need more info"),
        );
        assert_eq!(
            s,
            "[ERROR]:    {main.rs}   (not enough memory)   {[add more memory]} sometimes i need more info"
        );
    }

    #[test]
    fn file_name_strips_path() {
        assert_eq!(file_name("src/state.rs"), "state.rs");
        assert_eq!(file_name("C:\\proj\\src\\main.rs"), "main.rs");
    }

    #[test]
    fn hf_compact_http_error_multiline() {
        let s = format_hf_compact(
            "ERROR",
            "http",
            "HTTP request returned 5xx",
            "HF: retry after cold start",
            Some("GET"),
            Some("/api/health"),
            Some("502"),
            Some("420"),
            Some("abc-123"),
            &[],
        );
        assert!(s.starts_with("ERR | http | GET /api/health → 502 (420ms)"));
        assert!(s.contains("cause:  HTTP request returned 5xx"));
        assert!(s.contains("id:     abc-123"));
    }

    #[test]
    fn hf_compact_info_single_line() {
        let s = format_hf_compact(
            "INFO",
            "db.rs",
            "database ready",
            "no action needed",
            None,
            None,
            None,
            None,
            None,
            &[],
        );
        assert_eq!(s, "INF | db.rs | database ready");
    }

    #[test]
    fn log_style_respects_compact_env() {
        assert_eq!(
            log_style_from_env(Some("compact"), false),
            LogStyle::HfCompact
        );
        assert_eq!(
            log_style_from_env(Some("full"), true),
            LogStyle::Standard
        );
    }
}
