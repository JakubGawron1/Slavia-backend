use chrono::Utc;
use libsql::Connection;
use uuid::Uuid;

#[allow(clippy::too_many_arguments)]
pub async fn write_audit_log(
    conn: &Connection,
    actor_user_id: Option<&str>,
    actor_role: Option<&str>,
    category: &str,
    action: &str,
    target_type: Option<&str>,
    target_id: Option<&str>,
    details: Option<&str>,
) -> Result<(), libsql::Error> {
    conn.execute(
        "INSERT INTO system_audit_logs (id, actor_user_id, actor_role, category, action, target_type, target_id, details, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        (
            Uuid::new_v4().to_string(),
            actor_user_id.map(|s| s.to_string()),
            actor_role.map(|s| s.to_string()),
            category.to_string(),
            action.to_string(),
            target_type.map(|s| s.to_string()),
            target_id.map(|s| s.to_string()),
            details.map(|s| s.to_string()),
            Utc::now().to_rfc3339(),
        ),
    )
    .await?;
    Ok(())
}
