//! Sanityzacja HTML dla treści CMS — usuwa skrypty i niebezpieczne fragmenty.

use regex::Regex;
use std::sync::LazyLock;

static SCRIPT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap());
static STYLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap());
static ON_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\s+on\w+\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)"#).unwrap());
static JS_HREF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(href|src)\s*=\s*"(javascript:[^"]*|data:[^"]*)""#).unwrap()
});

/// Usuwa niebezpieczne tagi i atrybuty z HTML przed zapisem w CMS.
pub fn sanitize_cms_html(raw: &str) -> String {
    let mut out = raw.trim().to_string();
    if out.is_empty() {
        return out;
    }
    out = SCRIPT_RE.replace_all(&out, "").to_string();
    out = STYLE_RE.replace_all(&out, "").to_string();
    out = ON_ATTR_RE.replace_all(&out, "").to_string();
    out = JS_HREF_RE.replace_all(&out, "").to_string();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_script_tags() {
        let out = sanitize_cms_html("<p>OK</p><script>alert(1)</script>");
        assert!(!out.contains("script"));
        assert!(out.contains("OK"));
    }

    #[test]
    fn blocks_javascript_href() {
        let out = sanitize_cms_html(r#"<a href="javascript:alert(1)">x</a>"#);
        assert!(!out.contains("javascript:"));
    }
}
