//! Ograniczenie częstotliwości prób logowania (per nazwa użytkownika) — ochrona przed brute-force.
//! Nie wymaga `ConnectInfo` (przydatne za reverse proxy bez konfiguracji IP).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

static BUCKETS: OnceLock<Mutex<HashMap<String, Vec<Instant>>>> = OnceLock::new();

fn buckets() -> &'static Mutex<HashMap<String, Vec<Instant>>> {
    BUCKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

const WINDOW: Duration = Duration::from_secs(180);
/// W oknie czasu — maks. prób zanim zwrócimy 429 (zbyt agresywny limit utrudnia zapominalskim).
const MAX_ATTEMPTS: usize = 45;

fn norm_key(username: &str) -> String {
    username.trim().to_ascii_lowercase()
}

pub fn record_login_attempt(username: &str) -> Result<(), ()> {
    let key = norm_key(username);
    if key.is_empty() {
        return Ok(());
    }
    let now = Instant::now();
    let mut g = buckets().lock().map_err(|_| ())?;
    let v = g.entry(key).or_default();
    v.retain(|t| now.duration_since(*t) < WINDOW);
    if v.len() >= MAX_ATTEMPTS {
        return Err(());
    }
    v.push(now);
    Ok(())
}

pub fn clear_login_attempts(username: &str) {
    let key = norm_key(username);
    if key.is_empty() {
        return;
    }
    if let Ok(mut g) = buckets().lock() {
        g.remove(&key);
    }
}
