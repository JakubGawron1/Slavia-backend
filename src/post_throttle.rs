//! Limit częstotliwości wybranych mutacji POST dla zalogowanego użytkownika (`JWT sub` + kubełek).
//! Uzupełnienie `login_throttle` (który jest per nazwa użytkownika przy logowaniu).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static BUCKETS: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();

fn buckets() -> &'static Mutex<HashMap<String, Vec<Instant>>> {
    BUCKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn window_for_bucket(bucket: &str) -> (Duration, usize) {
    match bucket {
        "results_create" => (Duration::from_secs(120), 40),
        "results_batch_approve" => (Duration::from_secs(120), 15),
        "payments_my_create" => (Duration::from_secs(300), 12),
        "totp_mutations" => (Duration::from_secs(600), 18),
        _ => (Duration::from_secs(60), 45),
    }
}

/// Zwraca `Err(())` gdy przekroczono limit — mapuj na 429 z komunikatem PL.
pub fn record_user_post_attempt(user_sub: &str, bucket: &str) -> Result<(), ()> {
    let sub = user_sub.trim();
    if sub.is_empty() {
        return Ok(());
    }
    let b = bucket.trim();
    if b.is_empty() {
        return Ok(());
    }
    let (window, max) = window_for_bucket(b);
    let key = format!("{sub}::{b}");
    let now = Instant::now();
    let mut g = buckets().lock().map_err(|_| ())?;
    let v = g.entry(key).or_default();
    v.retain(|t| now.duration_since(*t) < window);
    if v.len() >= max {
        return Err(());
    }
    v.push(now);
    Ok(())
}
