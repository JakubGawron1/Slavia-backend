use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct NotificationDto {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<String>,
    pub created_at: String,
    pub is_read: bool,
}
