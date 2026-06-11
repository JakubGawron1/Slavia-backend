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
        // Trener AI — konserwatywnie pod darmowy tier Groq (RPM/RPD)
        "ai_coach_chat" => (Duration::from_secs(60), 4),
        "ai_coach_chat_daily" => (Duration::from_secs(86_400), 40),
        "ai_coach_import" => (Duration::from_secs(3600), 3),
        "ai_coach_import_daily" => (Duration::from_secs(86_400), 10),
        // Wspólny klucz klubowy Groq — limit globalny (wszystkich użytkowników)
        "ai_coach_club_global_chat" => (Duration::from_secs(60), 8),
        "ai_coach_club_global_chat_daily" => (Duration::from_secs(86_400), 300),
        "ai_coach_club_global_import" => (Duration::from_secs(3600), 4),
        "ai_coach_club_global_import_daily" => (Duration::from_secs(86_400), 30),
        // Asystent publiczny (anonimowy, per IP)
        "ai_coach_public_chat" => (Duration::from_secs(60), 3),
        "ai_coach_public_chat_daily" => (Duration::from_secs(86_400), 25),
        // Formularz kontaktowy (anonimowy, per IP)
        "contact_submit" => (Duration::from_secs(300), 5),
        "contact_submit_daily" => (Duration::from_secs(86_400), 20),
        // Tor sztangi AI (vision — droższe niż zwykły czat; free tier Groq)
        "ai_coach_barbell_path" => (Duration::from_secs(60), 2),
        "ai_coach_barbell_path_daily" => (Duration::from_secs(86_400), 10),
        "ai_coach_club_global_barbell_path" => (Duration::from_secs(60), 3),
        "ai_coach_club_global_barbell_path_daily" => (Duration::from_secs(86_400), 45),
        _ => (Duration::from_secs(60), 45),
    }
}

/// Liczba żądań w bieżącym oknie (bez rejestracji nowego).
pub fn count_user_post_attempts(user_sub: &str, bucket: &str) -> usize {
    let sub = user_sub.trim();
    if sub.is_empty() {
        return 0;
    }
    let b = bucket.trim();
    if b.is_empty() {
        return 0;
    }
    let (window, _) = window_for_bucket(b);
    let key = format!("{sub}::{b}");
    let now = Instant::now();
    let Ok(g) = buckets().lock() else {
        return 0;
    };
    g.get(&key)
        .map(|v| {
            v.iter()
                .filter(|t| now.duration_since(**t) < window)
                .count()
        })
        .unwrap_or(0)
}

/// Limit żądań dla kubełka (bez rejestracji).
pub fn max_for_bucket(bucket: &str) -> usize {
    window_for_bucket(bucket).1
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

/// Identyfikator kubełka dla wspólnego klucza Groq klubu.
pub const AI_COACH_CLUB_GLOBAL_SUB: &str = "__ai_coach_club__";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiCoachLimitDeny {
    ChatMinute,
    ChatDaily,
    ImportHour,
    ImportDaily,
    ClubChatMinute,
    ClubChatDaily,
    ClubImportHour,
    ClubImportDaily,
    BarbellPathMinute,
    BarbellPathDaily,
    ClubBarbellPathMinute,
    ClubBarbellPathDaily,
}

fn count_in_window(g: &HashMap<String, Vec<Instant>>, sub: &str, bucket: &str, now: Instant) -> usize {
    let (window, _) = window_for_bucket(bucket);
    let key = format!("{sub}::{bucket}");
    g.get(&key)
        .map(|v| {
            v.iter()
                .filter(|t| now.duration_since(**t) < window)
                .count()
        })
        .unwrap_or(0)
}

fn push_in_window(
    g: &mut HashMap<String, Vec<Instant>>,
    sub: &str,
    bucket: &str,
    now: Instant,
) {
    let (window, _) = window_for_bucket(bucket);
    let key = format!("{sub}::{bucket}");
    let v = g.entry(key).or_default();
    v.retain(|t| now.duration_since(*t) < window);
    v.push(now);
}

/// Atomowo rezerwuje slot czatu (per użytkownik + opcjonalnie globalny klucz klubu).
pub fn reserve_ai_coach_chat(user_sub: &str, include_club_global: bool) -> Result<(), AiCoachLimitDeny> {
    let sub = user_sub.trim();
    if sub.is_empty() {
        return Err(AiCoachLimitDeny::ChatMinute);
    }
    let mut g = buckets().lock().map_err(|_| AiCoachLimitDeny::ChatMinute)?;
    let now = Instant::now();

    let user_checks = [
        ("ai_coach_chat", AiCoachLimitDeny::ChatMinute),
        ("ai_coach_chat_daily", AiCoachLimitDeny::ChatDaily),
    ];
    for (bucket, deny) in user_checks {
        let (_, max) = window_for_bucket(bucket);
        if count_in_window(&g, sub, bucket, now) >= max {
            return Err(deny);
        }
    }

    if include_club_global {
        let club_checks = [
            (
                "ai_coach_club_global_chat",
                AiCoachLimitDeny::ClubChatMinute,
            ),
            (
                "ai_coach_club_global_chat_daily",
                AiCoachLimitDeny::ClubChatDaily,
            ),
        ];
        for (bucket, deny) in club_checks {
            let (_, max) = window_for_bucket(bucket);
            if count_in_window(&g, AI_COACH_CLUB_GLOBAL_SUB, bucket, now) >= max {
                return Err(deny);
            }
        }
    }

    for (bucket, _) in user_checks {
        push_in_window(&mut g, sub, bucket, now);
    }
    if include_club_global {
        push_in_window(
            &mut g,
            AI_COACH_CLUB_GLOBAL_SUB,
            "ai_coach_club_global_chat",
            now,
        );
        push_in_window(
            &mut g,
            AI_COACH_CLUB_GLOBAL_SUB,
            "ai_coach_club_global_chat_daily",
            now,
        );
    }
    Ok(())
}

/// Atomowo rezerwuje slot importu planu (per użytkownik + opcjonalnie globalny klucz klubu).
pub fn reserve_ai_coach_import(user_sub: &str, include_club_global: bool) -> Result<(), AiCoachLimitDeny> {
    let sub = user_sub.trim();
    if sub.is_empty() {
        return Err(AiCoachLimitDeny::ImportHour);
    }
    let mut g = buckets().lock().map_err(|_| AiCoachLimitDeny::ImportHour)?;
    let now = Instant::now();

    let user_checks = [
        ("ai_coach_import", AiCoachLimitDeny::ImportHour),
        ("ai_coach_import_daily", AiCoachLimitDeny::ImportDaily),
    ];
    for (bucket, deny) in user_checks {
        let (_, max) = window_for_bucket(bucket);
        if count_in_window(&g, sub, bucket, now) >= max {
            return Err(deny);
        }
    }

    if include_club_global {
        let club_checks = [
            (
                "ai_coach_club_global_import",
                AiCoachLimitDeny::ClubImportHour,
            ),
            (
                "ai_coach_club_global_import_daily",
                AiCoachLimitDeny::ClubImportDaily,
            ),
        ];
        for (bucket, deny) in club_checks {
            let (_, max) = window_for_bucket(bucket);
            if count_in_window(&g, AI_COACH_CLUB_GLOBAL_SUB, bucket, now) >= max {
                return Err(deny);
            }
        }
    }

    for (bucket, _) in user_checks {
        push_in_window(&mut g, sub, bucket, now);
    }
    if include_club_global {
        push_in_window(
            &mut g,
            AI_COACH_CLUB_GLOBAL_SUB,
            "ai_coach_club_global_import",
            now,
        );
        push_in_window(
            &mut g,
            AI_COACH_CLUB_GLOBAL_SUB,
            "ai_coach_club_global_import_daily",
            now,
        );
    }
    Ok(())
}

/// Atomowo rezerwuje slot korekty toru sztangi (vision — osobne limity free tier).
pub fn reserve_ai_coach_barbell_path(
    user_sub: &str,
    include_club_global: bool,
) -> Result<(), AiCoachLimitDeny> {
    let sub = user_sub.trim();
    if sub.is_empty() {
        return Err(AiCoachLimitDeny::BarbellPathMinute);
    }
    let mut g = buckets().lock().map_err(|_| AiCoachLimitDeny::BarbellPathMinute)?;
    let now = Instant::now();

    let user_checks = [
        (
            "ai_coach_barbell_path",
            AiCoachLimitDeny::BarbellPathMinute,
        ),
        (
            "ai_coach_barbell_path_daily",
            AiCoachLimitDeny::BarbellPathDaily,
        ),
    ];
    for (bucket, deny) in user_checks {
        let (_, max) = window_for_bucket(bucket);
        if count_in_window(&g, sub, bucket, now) >= max {
            return Err(deny);
        }
    }

    if include_club_global {
        let club_checks = [
            (
                "ai_coach_club_global_barbell_path",
                AiCoachLimitDeny::ClubBarbellPathMinute,
            ),
            (
                "ai_coach_club_global_barbell_path_daily",
                AiCoachLimitDeny::ClubBarbellPathDaily,
            ),
        ];
        for (bucket, deny) in club_checks {
            let (_, max) = window_for_bucket(bucket);
            if count_in_window(&g, AI_COACH_CLUB_GLOBAL_SUB, bucket, now) >= max {
                return Err(deny);
            }
        }
    }

    for (bucket, _) in user_checks {
        push_in_window(&mut g, sub, bucket, now);
    }
    if include_club_global {
        push_in_window(
            &mut g,
            AI_COACH_CLUB_GLOBAL_SUB,
            "ai_coach_club_global_barbell_path",
            now,
        );
        push_in_window(
            &mut g,
            AI_COACH_CLUB_GLOBAL_SUB,
            "ai_coach_club_global_barbell_path_daily",
            now,
        );
    }
    Ok(())
}

/// Limit wysyłki formularza kontaktowego (per IP).
pub fn reserve_contact_submit(client_ip: &str) -> Result<(), ()> {
    let ip = client_ip.trim();
    if ip.is_empty() || ip == "unknown" {
        return Err(());
    }
    let sub = format!("ip::{ip}");
    let now = Instant::now();
    let mut g = buckets().lock().map_err(|_| ())?;
    for bucket in ["contact_submit", "contact_submit_daily"] {
        let (_, max) = window_for_bucket(bucket);
        if count_in_window(&g, &sub, bucket, now) >= max {
            return Err(());
        }
    }
    for bucket in ["contact_submit", "contact_submit_daily"] {
        push_in_window(&mut g, &sub, bucket, now);
    }
    Ok(())
}

/// Atomowa rezerwacja slotu czatu publicznego (per IP).
pub fn reserve_ai_coach_public_chat(client_ip: &str) -> Result<(), AiCoachLimitDeny> {
    let ip = client_ip.trim();
    if ip.is_empty() || ip == "unknown" {
        return Err(AiCoachLimitDeny::ChatMinute);
    }
    let sub = format!("ip::{ip}");
    let mut g = buckets().lock().map_err(|_| AiCoachLimitDeny::ChatMinute)?;
    let now = Instant::now();

    let checks = [
        ("ai_coach_public_chat", AiCoachLimitDeny::ChatMinute),
        ("ai_coach_public_chat_daily", AiCoachLimitDeny::ChatDaily),
        (
            "ai_coach_club_global_chat",
            AiCoachLimitDeny::ClubChatMinute,
        ),
        (
            "ai_coach_club_global_chat_daily",
            AiCoachLimitDeny::ClubChatDaily,
        ),
    ];
    for (bucket, deny) in checks {
        let owner = if bucket.starts_with("ai_coach_club_global") {
            AI_COACH_CLUB_GLOBAL_SUB
        } else {
            sub.as_str()
        };
        let (_, max) = window_for_bucket(bucket);
        if count_in_window(&g, owner, bucket, now) >= max {
            return Err(deny);
        }
    }
    for (bucket, _) in checks {
        let owner = if bucket.starts_with("ai_coach_club_global") {
            AI_COACH_CLUB_GLOBAL_SUB
        } else {
            sub.as_str()
        };
        push_in_window(&mut g, owner, bucket, now);
    }
    Ok(())
}

#[cfg(test)]
mod ai_coach_limit_tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn reset_buckets() {
        if let Ok(mut g) = buckets().lock() {
            g.clear();
        }
    }

    #[test]
    fn chat_limit_blocks_on_fourth_request_in_minute() {
        let _guard = test_lock().lock().expect("test lock");
        reset_buckets();
        let sub = "user-test-chat";
        for _ in 0..4 {
            assert!(reserve_ai_coach_chat(sub, false).is_ok());
        }
        assert_eq!(
            reserve_ai_coach_chat(sub, false),
            Err(AiCoachLimitDeny::ChatMinute)
        );
    }

    #[test]
    fn club_global_chat_limit_is_shared() {
        let _guard = test_lock().lock().expect("test lock");
        reset_buckets();
        for i in 0..8 {
            assert!(
                reserve_ai_coach_chat(&format!("user-{i}"), true).is_ok(),
                "request {i} should pass"
            );
        }
        assert_eq!(
            reserve_ai_coach_chat("user-extra", true),
            Err(AiCoachLimitDeny::ClubChatMinute)
        );
    }

    #[test]
    fn barbell_path_limit_blocks_on_third_request_in_minute() {
        let _guard = test_lock().lock().expect("test lock");
        reset_buckets();
        let sub = "user-test-barbell";
        for _ in 0..2 {
            assert!(reserve_ai_coach_barbell_path(sub, false).is_ok());
        }
        assert_eq!(
            reserve_ai_coach_barbell_path(sub, false),
            Err(AiCoachLimitDeny::BarbellPathMinute)
        );
    }

    #[test]
    fn club_global_barbell_path_limit_is_shared() {
        let _guard = test_lock().lock().expect("test lock");
        reset_buckets();
        for i in 0..3 {
            assert!(
                reserve_ai_coach_barbell_path(&format!("user-bp-{i}"), true).is_ok(),
                "request {i} should pass"
            );
        }
        assert_eq!(
            reserve_ai_coach_barbell_path("user-bp-extra", true),
            Err(AiCoachLimitDeny::ClubBarbellPathMinute)
        );
    }
}
