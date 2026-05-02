//! Powiadomienia in-app (per `users.id`). Błędy zapisu nie blokują głównej operacji API.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use libsql::Connection;
use uuid::Uuid;

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
    let mut rows = conn
        .query("SELECT id FROM users WHERE role = 'SuperAdmin'", ())
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

/// Trenerzy i wyżej — m.in. zgłoszenia wyników, wpisy zawodników w dzienniku, zawody.
pub async fn trainer_staff_ids(conn: &Connection) -> Result<Vec<String>, libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT id FROM users WHERE role IN ('Trainer', 'TrainerAdmin', 'Admin', 'SuperAdmin')",
            (),
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(row.get(0)?);
    }
    Ok(out)
}

/// Kadra administracyjna (bez zwykłego Trenera).
pub async fn admin_staff_ids(conn: &Connection) -> Result<Vec<String>, libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT id FROM users WHERE role IN ('Admin', 'TrainerAdmin', 'SuperAdmin')",
            (),
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push(row.get(0)?);
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
        let name = athlete_full_name(conn, &aid).await?.unwrap_or_else(|| aid.clone());
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

pub fn notify_result_pending(state: &AppState, athlete_id: &str, athlete_name: &str, total: f64, date: &str) {
    let st = state.clone();
    let aid = athlete_id.to_string();
    let name = athlete_name.to_string();
    let date = date.to_string();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let title = "Nowy wynik do zatwierdzenia";
        let body = format!("{} zgłosił(a) wynik {:.1} kg ({}).", name, total, date);
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
        let title_a = "Przypisano do zawodów";
        let body_a = format!("Jesteś zapisany(a) na: {}.", ct);
        let title_s = "Przypisanie zawodnika";
        let body_s = format!(
            "{} → „{}” ({})",
            athlete_full_name(st.db.as_ref(), &aid)
                .await?
                .unwrap_or_else(|| aid.clone()),
            ct,
            cid
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
        let title_a = "Usunięto przypisanie";
        let body_a = format!("Nie jesteś już zapisany(a) na: {}.", ct);
        let title_s = "Usunięto przypisanie zawodnika";
        let body_s = format!(
            "{} — „{}” ({})",
            athlete_full_name(st.db.as_ref(), &aid)
                .await?
                .unwrap_or_else(|| aid.clone()),
            ct,
            cid
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
            athlete_full_name(st.db.as_ref(), &aid)
                .await?
                .unwrap_or_else(|| aid.clone()),
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
        let body = format!("{} — {} ({})", title_ev, date, loc);
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
        let body = format!("„{}” — zmieniono szczegóły.", title_ev);
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
        let body = format!("„{}” zostało usunięte.", title_ev);
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

pub fn notify_competition_roster_updated(state: &AppState, competition_id: &str, competition_title_str: &str) {
    let st = state.clone();
    let cid = competition_id.to_string();
    let ct = competition_title_str.to_string();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let title = "Lista zapisów na zawody";
        let body = format!("Zaktualizowano zapisy na „{}”.", ct);
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

pub fn notify_competitions_synced(state: &AppState, pzpc: usize, pc: usize, upserts: usize) {
    let st = state.clone();
    spawn_notify(async move {
        let conn = st.db.as_ref();
        let title = "Synchronizacja kalendarza";
        let body = format!(
            "Zsynchronizowano zawody zewnętrzne (PZPC: {}, PC: {}, zmian w bazie: {}).",
            pzpc, pc, upserts
        );
        for uid in trainer_staff_ids(conn).await? {
            insert_notification(conn, &uid, "competitions_synced", title, &body, None).await?;
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
        let title_c = titles.get(removed).map(|s| s.as_str()).unwrap_or(removed.as_str());
        notify_competition_unassigned_from_athlete(state, athlete_id, removed, title_c);
    }
    for added in new_competition_ids.difference(&old_competition_ids) {
        let title_c = titles.get(added).map(|s| s.as_str()).unwrap_or(added.as_str());
        notify_competition_assigned_to_athlete(state, athlete_id, added, title_c);
    }
}
