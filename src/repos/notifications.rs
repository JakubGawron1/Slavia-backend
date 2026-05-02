//! Zapytania SQL dla powiadomień — oddzielone od warstwy HTTP.

use libsql::{Connection, Row};

use crate::dto::notifications::NotificationDto;

pub const LIST_LIMIT: i64 = 200;

fn row_to_dto(row: &Row) -> Result<NotificationDto, libsql::Error> {
    Ok(NotificationDto {
        id: row.get(0)?,
        kind: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        payload: row.get(4).ok(),
        created_at: row.get(5)?,
    })
}

pub async fn list_for_user(conn: &Connection, user_id: &str) -> Result<Vec<NotificationDto>, libsql::Error> {
    let mut rows = conn
        .query(
            "SELECT id, kind, title, body, payload, created_at FROM notifications \
             WHERE user_id = ?1 ORDER BY created_at DESC LIMIT ?2",
            libsql::params!(user_id.to_string(), LIST_LIMIT),
        )
        .await?;

    let mut list = Vec::new();
    while let Some(row) = rows.next().await? {
        list.push(row_to_dto(&row)?);
    }
    Ok(list)
}

pub async fn delete_one(conn: &Connection, id: &str, user_id: &str) -> Result<u64, libsql::Error> {
    conn.execute(
        "DELETE FROM notifications WHERE id = ?1 AND user_id = ?2",
        (id.to_string(), user_id.to_string()),
    )
    .await
}
