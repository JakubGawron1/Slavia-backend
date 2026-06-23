//! Publiczne katalogi statyczne (embed JSON) — presety motywu, PZPC, odznaki.

fn embed_json_response(body: &'static str) -> (axum::http::HeaderMap, String) {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    (headers, body.to_string())
}

pub async fn theme_presets_handler() -> (axum::http::HeaderMap, String) {
    embed_json_response(include_str!("../embed/theme-presets.json"))
}

pub async fn pzpc_weight_classes_handler() -> (axum::http::HeaderMap, String) {
    embed_json_response(include_str!("../embed/pzpc-weight-classes.json"))
}

pub async fn athlete_badges_handler() -> (axum::http::HeaderMap, String) {
    embed_json_response(include_str!("../embed/athlete-badges.json"))
}
