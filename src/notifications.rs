//! Powiadomienia in-app (per `users.id`). Błędy zapisu nie blokują głównej operacji API.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use libsql::Connection;
use uuid::Uuid;

use crate::models::Role;
use crate::state::AppState;

pub async fn insert_notification(
    conn: &Connection,
    user_id: &str,
    kind: &str,
    title: &str,
    body: &str,
    payload_json: Option<&str>,
) -> Result<(), libsql::Error> {
    let id = Uuid::new_v4().to_string();
    let created = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO notifications (id, user_id, kind, title, body, payload, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            id,
            user_id.to_string(),
            kind.to_string(),
            title.to_string(),
            body.to_string(),
            payload_json.map(|s| s.to_string()),
            created,
        ),
    )
    .await?;
    Ok(())
}

pub fn spawn_notify<F>(fut: F)
where
    F: std::future::Future<Output = Result<(), libsql::Error>> + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(e) = fut.await {
            eprintln!("[notifications] {e}");
        }
    });
}

pub async fn superadmin_ids(conn: &Connection) -> Result<Vec<String>, libsql::Error> {
    let mut rows = conn.query("SELECT id, roles FROM users", ()).await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let roles_json: String = row.get(1)?;
        let roles: Vec<Role> = serde_json::from_str(&roles_json).unwrap_or_default();
        if roles.contains(&Role::SuperAdmin) {
            out.push(id);
        }
    }
    Ok(out)
}

/// Trenerzy i wyżej — m.in. zgłoszenia wyników, wpisy zawodników w dzienniku, zawody.
pub async fn trainer_staff_ids(conn: &Connection) -> Result<Vec<String>, libsql::Error> {
    let mut rows = conn.query("SELECT id, roles FROM users", ()).await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let roles_json: String = row.get(1)?;
        let roles: Vec<Role> = serde_json::from_str(&roles_json).unwrap_or_default();
        if roles
            .iter()
            .any(|r| matches!(r, Role::Trainer | Role::Admin | Role::SuperAdmin))
        {
            out.push(id);
        }
    }
    Ok(out)
}

/// Kadra administracyjna (bez samego Trenera bez Admina).
pub async fn admin_staff_ids(conn: &Connection) -> Result<Vec<String>, libsql::Error> {
    let mut rows = conn.query("SELECT id, roles FROM users", ()).await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let roles_json: String = row.get(1)?;
        let roles: Vec<Role> = serde_json::from_str(&roles_json).unwrap_or_default();
        if roles.iter().any(|r| matches!(r, Role::Admin | Role::SuperAdmin)) {
            out.push(id);
        }
    }
    Ok(out)
}

pub async fn all_user_ids(conn: &Connection) -> Result<Vec<String>, libsql::Error> {
    let mut rows = conn.query("SELECT id FROM users", ()).await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        out.push(id);
    }
    Ok(out)
}

pub async fn athlete_user_id(conn: &Connection, athlete_id: &str) -> Result<Option<String>, libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT user_id FROM athletes WHERE id = ?1",
            [athlete_id.to_string()],
        )
        .await?;
    let row = rows.next().await?;
    Ok(row.and_then(|r| r.get::<String>(0).ok()))
}

pub async fn athlete_full_name(conn: &Connection, athlete_id: &str) -> Result<Option<String>, libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT full_name FROM athletes WHERE id = ?1",
            [athlete_id.to_string()],
        )
        .await?;
    let row = rows.next().await?;
    Ok(row.and_then(|r| r.get::<String>(0).ok()))
}

pub async fn username_by_id(conn: &Connection, user_id: &str) -> Result<Option<String>, libsql::Error> {
    let mut rows = conn
        .query("SELECT username FROM users WHERE id = ?1", [user_id.to_string()])
        .await?;
    let row = rows.next().await?;
    Ok(row.and_then(|r| r.get::<String>(0).ok()))
}

pub async fn competition_title(conn: &Connection, competition_id: &str) -> Result<Option<String>, libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT title FROM competitions WHERE id = ?1",
            [competition_id.to_string()],
        )
        .await?;
    let row = rows.next().await?;
    Ok(row.and_then(|r| r.get::<String>(0).ok()))
}

/// Imię i nazwisko do treści powiadomień — bez surowych UUID w UI.
pub async fn athlete_display_for_notification(conn: &Connection, athlete_id: &str) -> Result<String, libsql::Error> {
    Ok(athlete_full_name(conn, athlete_id)
        .await?
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Zawodnik".to_string()))
}

/// Tytuł zawodów w treści powiadomień — jeśli w bazie brak tytułu albo został podany sam UUID, nie pokazujemy hasha.
pub fn competition_title_for_notification(raw: &str) -> String {
    let t = raw.trim();
    if t.is_empty() {
        return "Wydarzenie w kalendarzu".to_string();
    }
    if Uuid::parse_str(t).is_ok() {
        return "Wydarzenie w kalendarzu".to_string();
    }
    t.to_string()
}

async fn athlete_plus_superadmins(
    state: &AppState,
    athlete_id: &str,
    kind_athlete: &str,
    title_athlete: &str,
    body_athlete: &str,
    kind_staff: &str,
    title_staff: &str,
    body_staff: &str,
    payload: Option<&str>,
) -> Result<(), libsql::Error> {
    let conn = state.db.as_ref();
    let uid_opt = athlete_user_id(conn, athlete_id).await?;
    if let Some(ref uid) = uid_opt {
        insert_notification(conn, uid, kind_athlete, title_athlete, body_athlete, payload).await?;
    }
    for sid in superadmin_ids(conn).await? {
        if uid_opt.as_ref() == Some(&sid) {
            continue;
        }
        insert_notification(conn, &sid, kind_staff, title_staff, body_staff, payload).await?;
    }
    Ok(())
}

pub fn notify_result_approved(state: &AppState, athlete_id: &str, total: f64, date: &str) {
    let st = state.clone();
    let aid = athlete_id.to_string();
    let date = date.to_string();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let name = athlete_display_for_notification(conn, &aid).await?;
        let title_a = "Wynik zatwierdzony";
        let body_a = format!(
            "Kadra zatwierdziła Twój wynik z {}: {:.1} kg (dwubój).",
            date, total
        );
        let title_s = format!("Zatwierdzono wynik zawodnika ({})", name);
        let body_s = format!("Dwubój {:.1} kg — data {}.", total, date);
        let payload = serde_json::json!({
            "athlete_id": aid,
            "total": total,
            "date": date,
        })
        .to_string();
        athlete_plus_superadmins(
            &st,
            &aid,
            "result_approved",
            title_a,
            &body_a,
            "result_approved_staff",
            &title_s,
            &body_s,
            Some(&payload),
        )
        .await?;
        Ok(())
    });
}

pub fn notify_result_pending(
    state: &AppState,
    athlete_id: &str,
    athlete_name: &str,
    total: f64,
    date: &str,
    strength_only: bool,
) {
    let st = state.clone();
    let aid = athlete_id.to_string();
    let name = athlete_name.to_string();
    let date = date.to_string();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let title = "Nowy wynik do zatwierdzenia";
        let body = if strength_only {
            format!(
                "{} — zgłoszenie przysiadu/wycisku/martwego do weryfikacji ({}). Wpis dwubojowy: {:.1} kg (z profilu, jeśli nie podano nowych rwanie/podrzut).",
                name, date, total
            )
        } else {
            format!("{} zgłosił(a) wynik {:.1} kg ({}).", name, total, date)
        };
        let payload = serde_json::json!({
            "athlete_id": aid,
            "total": total,
            "date": date,
        })
        .to_string();
        for uid in trainer_staff_ids(conn).await? {
            insert_notification(conn, &uid, "result_pending", title, &body, Some(&payload)).await?;
        }
        Ok(())
    });
}

pub fn notify_competition_assigned_to_athlete(state: &AppState, athlete_id: &str, competition_id: &str, competition_title_str: &str) {
    let st = state.clone();
    let aid = athlete_id.to_string();
    let cid = competition_id.to_string();
    let ct = competition_title_str.to_string();
    spawn_notify(async move {
        let ct_h = competition_title_for_notification(&ct);
        let title_a = "Przypisano do zawodów";
        let body_a = format!("Jesteś zapisany(a) na: {}.", ct_h);
        let title_s = "Przypisanie zawodnika";
        let body_s = format!(
            "{} — zapis na „{}”.",
            athlete_display_for_notification(st.db.as_ref(), &aid).await?,
            ct_h
        );
        let payload = serde_json::json!({
            "athlete_id": aid,
            "competition_id": cid,
        })
        .to_string();
        athlete_plus_superadmins(
            &st,
            &aid,
            "competition_assigned",
            title_a,
            &body_a,
            "competition_assigned_staff",
            title_s,
            &body_s,
            Some(&payload),
        )
        .await?;
        Ok(())
    });
}

pub fn notify_competition_unassigned_from_athlete(
    state: &AppState,
    athlete_id: &str,
    competition_id: &str,
    competition_title_str: &str,
) {
    let st = state.clone();
    let aid = athlete_id.to_string();
    let cid = competition_id.to_string();
    let ct = competition_title_str.to_string();
    spawn_notify(async move {
        let ct_h = competition_title_for_notification(&ct);
        let title_a = "Usunięto przypisanie";
        let body_a = format!("Nie jesteś już zapisany(a) na: {}.", ct_h);
        let title_s = "Usunięto przypisanie zawodnika";
        let body_s = format!(
            "{} — usunięto z „{}”.",
            athlete_display_for_notification(st.db.as_ref(), &aid).await?,
            ct_h
        );
        let payload = serde_json::json!({
            "athlete_id": aid,
            "competition_id": cid,
        })
        .to_string();
        athlete_plus_superadmins(
            &st,
            &aid,
            "competition_unassigned",
            title_a,
            &body_a,
            "competition_unassigned_staff",
            title_s,
            &body_s,
            Some(&payload),
        )
        .await?;
        Ok(())
    });
}

pub fn notify_training_log_trainer_note(
    state: &AppState,
    athlete_id: &str,
    author_username: Option<&str>,
    session_date: &str,
    preview: &str,
) {
    let st = state.clone();
    let aid = athlete_id.to_string();
    let sd = session_date.to_string();
    let au = author_username.unwrap_or("Kadra").to_string();
    let pv = truncate_preview(preview);
    spawn_notify(async move {
        let title_a = "Nowa notatka trenera";
        let body_a = format!("{} — {} ({}).", au, pv, sd);
        let title_s = "Dziennik: notatka kadry";
        let body_s = format!(
            "{} — {} dodał(a) wpis ({})",
            athlete_display_for_notification(st.db.as_ref(), &aid).await?,
            au,
            sd
        );
        let payload = serde_json::json!({
            "athlete_id": aid,
            "session_date": sd,
        })
        .to_string();
        athlete_plus_superadmins(
            &st,
            &aid,
            "training_log_trainer_note",
            title_a,
            &body_a,
            "training_log_trainer_note_staff",
            title_s,
            &body_s,
            Some(&payload),
        )
        .await?;
        Ok(())
    });
}

fn truncate_preview(s: &str) -> String {
    let count = s.chars().count();
    if count <= 120 {
        return s.to_string();
    }
    let t: String = s.chars().take(120).collect();
    format!("{t}…")
}

pub fn notify_training_log_athlete_note(
    state: &AppState,
    athlete_id: &str,
    athlete_display_name: &str,
    author_username: &str,
    session_date: &str,
    preview: &str,
) {
    let st = state.clone();
    let aid = athlete_id.to_string();
    let adn = athlete_display_name.to_string();
    let au = author_username.to_string();
    let sd = session_date.to_string();
    let pv = truncate_preview(preview);
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let title = "Nowy wpis zawodnika w dzienniku";
        let body = format!("{} — {} ({})", adn, pv, sd);
        let payload = serde_json::json!({
            "athlete_id": aid,
            "session_date": sd,
            "author_username": au,
        })
        .to_string();
        let mut seen = HashSet::<String>::new();
        for uid in trainer_staff_ids(conn).await? {
            if seen.insert(uid.clone()) {
                insert_notification(conn, &uid, "training_log_athlete_note", title, &body, Some(&payload)).await?;
            }
        }
        Ok(())
    });
}

pub fn notify_competition_created(state: &AppState, title_ev: &str, date: &str, location: &str, competition_id: &str) {
    let st = state.clone();
    let title_ev = title_ev.to_string();
    let date = date.to_string();
    let loc = location.to_string();
    let cid = competition_id.to_string();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let title = "Nowe wydarzenie w kalendarzu";
        let ev = competition_title_for_notification(&title_ev);
        let body = format!("{} — {} ({})", ev, date, loc);
        let payload = serde_json::json!({ "competition_id": cid }).to_string();
        for uid in trainer_staff_ids(conn).await? {
            insert_notification(conn, &uid, "competition_created", title, &body, Some(&payload)).await?;
        }
        Ok(())
    });
}

pub fn notify_competition_updated(state: &AppState, title_ev: &str, competition_id: &str) {
    let st = state.clone();
    let title_ev = title_ev.to_string();
    let cid = competition_id.to_string();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let title = "Zaktualizowano zawody";
        let ev = competition_title_for_notification(&title_ev);
        let body = format!("„{}” — zmieniono szczegóły.", ev);
        let payload = serde_json::json!({ "competition_id": cid }).to_string();
        for uid in trainer_staff_ids(conn).await? {
            insert_notification(conn, &uid, "competition_updated", title, &body, Some(&payload)).await?;
        }
        Ok(())
    });
}

pub fn notify_competition_deleted(state: &AppState, title_ev: &str, competition_id: &str) {
    let st = state.clone();
    let title_ev = title_ev.to_string();
    let cid = competition_id.to_string();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let title = "Usunięto zawody z kalendarza";
        let ev = competition_title_for_notification(&title_ev);
        let body = format!("„{}” zostało usunięte.", ev);
        let payload = serde_json::json!({ "competition_id": cid }).to_string();
        for uid in trainer_staff_ids(conn).await? {
            insert_notification(conn, &uid, "competition_deleted", title, &body, Some(&payload)).await?;
        }
        Ok(())
    });
}

pub fn notify_admin_broadcast(state: &AppState, kind: &str, title: &str, body: &str, payload: Option<String>) {
    let st = state.clone();
    let kind = kind.to_string();
    let title = title.to_string();
    let body = body.to_string();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let payload_ref = payload.as_deref();
        for uid in admin_staff_ids(conn).await? {
            insert_notification(conn, &uid, &kind, &title, &body, payload_ref).await?;
        }
        Ok(())
    });
}

pub fn notify_announcement_published(state: &AppState, announcement_id: &str, announcement_title: &str) {
    let st = state.clone();
    let aid = announcement_id.to_string();
    let at = announcement_title.trim().to_string();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let title = "Nowe ogłoszenie";
        let body = if at.is_empty() {
            "Dodano nowe ogłoszenie w systemie.".to_string()
        } else {
            at
        };
        let payload = serde_json::json!({ "announcement_id": aid }).to_string();
        for uid in all_user_ids(conn).await? {
            insert_notification(
                conn,
                &uid,
                "announcement_published",
                title,
                &body,
                Some(&payload),
            )
            .await?;
        }
        Ok(())
    });
}

pub fn notify_competition_roster_updated(state: &AppState, competition_id: &str, competition_title_str: &str) {
    let st = state.clone();
    let cid = competition_id.to_string();
    let ct = competition_title_str.to_string();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let title = "Lista zapisów na zawody";
        let ev = competition_title_for_notification(&ct);
        let body = format!("Zaktualizowano zapisy na „{}”.", ev);
        let payload = serde_json::json!({ "competition_id": cid }).to_string();
        for uid in trainer_staff_ids(conn).await? {
            insert_notification(
                conn,
                &uid,
                "competition_roster_updated",
                title,
                &body,
                Some(&payload),
            )
            .await?;
        }
        Ok(())
    });
}

pub fn notify_competitions_synced(
    state: &AppState,
    pzpc: usize,
    pc: usize,
    upserts: usize,
    stale_removed: u64,
    stale_import: u64,
    stale_manual: u64,
) {
    let st = state.clone();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let title = "Synchronizacja kalendarza";
        let body = format!(
            "Zsynchronizowano zawody zewnętrzne (PZPC: {}, PC: {}, merge w bazie: {}). \
             Usunięto wpisy spoza bieżącego i następnego roku (łącznie: {} — importy: {}, ręczne: {}).",
            pzpc, pc, upserts, stale_removed, stale_import, stale_manual
        );
        for uid in trainer_staff_ids(conn).await? {
            insert_notification(conn, &uid, "competitions_synced", title, &body, None).await?;
        }
        Ok(())
    });
}

pub fn notify_training_plan_assigned(
    state: &AppState,
    athlete_id: &str,
    plan_title: &str,
    week_start: &str,
) {
    let st = state.clone();
    let aid = athlete_id.to_string();
    let title = plan_title.to_string();
    let week = week_start.to_string();
    spawn_notify(async move {
        let title_a = "Nowy plan treningowy";
        let body_a = format!("Dodano plan: „{}” (start tygodnia: {}).", title, week);
        let title_s = "Plan treningowy przypisany";
        let body_s = format!(
            "{} — plan „{}”, tydzień {}.",
            athlete_display_for_notification(st.db.as_ref(), &aid).await?,
            title,
            week
        );
        let payload = serde_json::json!({ "athlete_id": aid, "week_start": week }).to_string();
        athlete_plus_superadmins(
            &st,
            &aid,
            "training_plan_assigned",
            title_a,
            &body_a,
            "training_plan_assigned_staff",
            title_s,
            &body_s,
            Some(&payload),
        )
        .await?;
        Ok(())
    });
}

pub fn notify_training_plan_progress_updated(
    state: &AppState,
    athlete_id: &str,
    plan_title: &str,
    progress_percent: i64,
) {
    let st = state.clone();
    let aid = athlete_id.to_string();
    let title = plan_title.to_string();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let n_title = "Aktualizacja progresu planu";
        let n_body = format!(
            "{}: {}% — plan „{}”.",
            athlete_display_for_notification(conn, &aid).await?,
            progress_percent,
            title
        );
        let payload = serde_json::json!({ "athlete_id": aid, "progress_percent": progress_percent }).to_string();
        for uid in trainer_staff_ids(conn).await? {
            insert_notification(
                conn,
                &uid,
                "training_plan_progress",
                n_title,
                &n_body,
                Some(&payload),
            )
            .await?;
        }
        Ok(())
    });
}

pub async fn diff_notify_athlete_competition_assignments(
    state: &AppState,
    athlete_id: &str,
    old_competition_ids: HashSet<String>,
    new_competition_ids: HashSet<String>,
) {
    let conn = state.db.as_ref();
    let mut titles: HashMap<String, String> = HashMap::new();
    let all_ids: HashSet<String> = old_competition_ids.union(&new_competition_ids).cloned().collect();
    for cid in all_ids {
        if let Ok(Some(t)) = competition_title(conn, &cid).await {
            titles.insert(cid, t);
        }
    }
    for removed in old_competition_ids.difference(&new_competition_ids) {
        let title_c = titles
            .get(removed)
            .map(|s| s.as_str())
            .unwrap_or("Wydarzenie w kalendarzu");
        notify_competition_unassigned_from_athlete(state, athlete_id, removed, title_c);
    }
    for added in new_competition_ids.difference(&old_competition_ids) {
        let title_c = titles
            .get(added)
            .map(|s| s.as_str())
            .unwrap_or("Wydarzenie w kalendarzu");
        notify_competition_assigned_to_athlete(state, athlete_id, added, title_c);
    }
}
