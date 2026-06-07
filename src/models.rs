use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Hash)]
pub enum Role {
    SuperAdmin,
    Admin,
    Editor,
    Trainer,
    Athlete,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum Gender {
    Male,
    Female,
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Role::SuperAdmin => "SuperAdmin",
            Role::Admin => "Admin",
            Role::Editor => "Editor",
            Role::Trainer => "Trainer",
            Role::Athlete => "Athlete",
        };
        write!(f, "{}", s)
    }
}

impl std::str::FromStr for Role {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "SuperAdmin" => Ok(Role::SuperAdmin),
            "Admin" => Ok(Role::Admin),
            "Editor" => Ok(Role::Editor),
            "Trainer" => Ok(Role::Trainer),
            "Athlete" => Ok(Role::Athlete),
            _ => Err(format!("Invalid role: {}", s)),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct User {
    pub id: String,
    pub username: String,
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub is_banned: bool,
    #[serde(default)]
    pub banned_reason: Option<String>,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub roles: Vec<Role>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Athlete {
    pub id: String,
    pub user_id: Option<String>,
    pub full_name: String,
    pub birth_year: Option<i64>,
    pub gender: Option<String>, // "male" or "female"
    pub weight_category: Option<String>,
    pub bodyweight: Option<f64>,
    pub best_snatch_kg: Option<f64>,
    pub best_clean_jerk_kg: Option<f64>,
    pub total_kg: Option<f64>,
    pub image_url: Option<String>,
    pub notes: Option<String>,
    pub profile_tagline: Option<String>,
    pub public_bio: Option<String>,
    pub is_active: bool,
    /// Czy zawodnik ma zlecenie stałe na składkę — wtedy scheduler automatycznie tworzy
    /// Approved-payment dla bieżącego miesiąca (jeśli jeszcze go nie ma).
    #[serde(default)]
    pub has_standing_order: bool,
}

/// Widok publiczny profilu — bez `user_id` i bez notatek wewnętrznych (`notes`).
#[derive(Debug, Serialize, Clone)]
pub struct AthletePublic {
    pub id: String,
    pub full_name: String,
    pub birth_year: Option<i64>,
    pub gender: Option<String>,
    pub weight_category: Option<String>,
    pub bodyweight: Option<f64>,
    pub best_snatch_kg: Option<f64>,
    pub best_clean_jerk_kg: Option<f64>,
    pub total_kg: Option<f64>,
    pub image_url: Option<String>,
    pub profile_tagline: Option<String>,
    pub public_bio: Option<String>,
    pub is_active: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum ResultStatus {
    Pending,
    Approved,
    Rejected,
}

impl std::fmt::Display for ResultStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResultStatus::Pending => write!(f, "Pending"),
            ResultStatus::Approved => write!(f, "Approved"),
            ResultStatus::Rejected => write!(f, "Rejected"),
        }
    }
}

impl std::str::FromStr for ResultStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(ResultStatus::Pending),
            "Approved" => Ok(ResultStatus::Approved),
            "Rejected" => Ok(ResultStatus::Rejected),
            _ => Err(format!("Invalid status: {}", s)),
        }
    }
}

/// Rozróżnienie wpisu w `results`:
/// `Competition` — start zawodów (publiczne, ranking, public-board, wykres na karcie)
/// `Training` — wynik z treningu (widoczny po zalogowaniu, nie wpływa na PB w `athletes`).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ResultKind {
    #[default]
    Competition,
    Training,
}


impl std::fmt::Display for ResultKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResultKind::Competition => write!(f, "competition"),
            ResultKind::Training => write!(f, "training"),
        }
    }
}

impl std::str::FromStr for ResultKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "competition" | "comp" | "" => Ok(ResultKind::Competition),
            "training" | "train" => Ok(ResultKind::Training),
            other => Err(format!("Invalid result kind: {}", other)),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CompetitionResult {
    pub id: String,
    pub athlete_id: String,
    pub snatch: f64,
    pub clean_and_jerk: f64,
    pub total: f64,
    pub status: ResultStatus,
    pub date: String,
    #[serde(default)]
    pub kind: ResultKind,
    /// Miejsce zawodów — wypełniane tylko dla `kind = Competition`.
    #[serde(default)]
    pub location: Option<String>,
    /// Waga ciała na starcie (kg) — opcjonalna; używana do obliczeń (np. Sinclair) per start.
    #[serde(default)]
    pub bodyweight_kg: Option<f64>,
    #[serde(default)]
    pub squat_kg: Option<f64>,
    #[serde(default)]
    pub bench_kg: Option<f64>,
    #[serde(default)]
    pub deadlift_kg: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Competition {
    pub id: String,
    pub title: String,
    pub date: String,
    pub location: String,
    pub description: Option<String>,
    pub category: Option<String>, // "championship", "league", "club_event", "training"
    pub status: Option<String>,   // "scheduled", "cancelled", "moved"
    /// np. `pzpc`, `podnoszenieciezarow` — brak = zawody utworzone w klubie
    #[serde(default)]
    pub external_source: Option<String>,
    #[serde(default)]
    pub external_ref: Option<String>,
    #[serde(default)]
    pub external_url: Option<String>,
    /// Klub bierze udział w zawodach (niezależnie od przypisanych zawodników).
    #[serde(default)]
    pub club_participates: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TrainingLogEntry {
    pub id: String,
    pub athlete_id: String,
    pub session_date: String,
    pub title: Option<String>,
    pub notes: String,
    pub created_at: String,
    pub author_user_id: Option<String>,
    pub author_username: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Post {
    pub id: String,
    pub title: String,
    pub content: String,
    pub author_id: String,
    pub image_url: Option<String>,
    pub created_at: String,
    #[serde(default = "default_post_published")]
    pub published: bool,
}

fn default_post_published() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Announcement {
    pub id: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub sort_order: i64,
    #[serde(default = "default_post_published")]
    pub published: bool,
    pub author_id: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GalleryPhoto {
    pub id: String,
    pub image_url: String,
    #[serde(default)]
    pub media_type: String,
    pub caption: Option<String>,
    #[serde(default)]
    pub sort_order: i64,
    #[serde(default = "default_post_published")]
    pub published: bool,
    pub author_id: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ContactMessage {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub email: Option<String>,
    pub phone: Option<String>,
    pub message: String,
    pub created_at: String,
    #[serde(default)]
    pub is_read: bool,
}

/// Publiczna lista wyników z imieniem zawodnika i miejscem zawodów (bez edycji).
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PublicResultBoardRow {
    pub id: String,
    pub athlete_id: String,
    pub athlete_name: String,
    pub competition_id: Option<String>,
    pub competition_title: Option<String>,
    pub snatch: f64,
    pub clean_and_jerk: f64,
    pub total: f64,
    pub date: String,
    #[serde(default)]
    pub kind: ResultKind,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub squat_kg: Option<f64>,
    #[serde(default)]
    pub bench_kg: Option<f64>,
    #[serde(default)]
    pub deadlift_kg: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ExerciseBoardRow {
    pub athlete_id: String,
    pub athlete_name: String,
    pub squat_kg: Option<f64>,
    pub bench_kg: Option<f64>,
    pub deadlift_kg: Option<f64>,
    pub source_trainer_direct: bool,
    pub source_athlete_pending_count: i64,
    pub source_approved_results_count: i64,
    pub source_training_log_count: i64,
    pub source_last_approved_date: Option<String>,
}

/// Typ wartości zmiennej CMS.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CmsVariableType {
    Text,
    Html,
    Image,
    Number,
    Boolean,
}

impl std::fmt::Display for CmsVariableType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            CmsVariableType::Text => "text",
            CmsVariableType::Html => "html",
            CmsVariableType::Image => "image",
            CmsVariableType::Number => "number",
            CmsVariableType::Boolean => "boolean",
        };
        write!(f, "{}", s)
    }
}

impl std::str::FromStr for CmsVariableType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(CmsVariableType::Text),
            "html" => Ok(CmsVariableType::Html),
            "image" => Ok(CmsVariableType::Image),
            "number" => Ok(CmsVariableType::Number),
            "boolean" => Ok(CmsVariableType::Boolean),
            _ => Err(format!("Invalid CMS variable type: {}", s)),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CmsVariable {
    pub id: String,
    pub key: String,
    pub value: serde_json::Value,
    #[serde(rename = "type")]
    pub value_type: CmsVariableType,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CmsPage {
    pub id: String,
    pub page_name: String,
    pub fields: serde_json::Value,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CmsNavigationItem {
    pub id: String,
    pub role: String,
    pub label: String,
    pub icon: String,
    pub url: String,
    pub order_index: i64,
    #[serde(default)]
    pub group_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CmsVersionEntry {
    pub id: String,
    pub entity_type: String,
    pub entity_key: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
    pub changed_by: Option<String>,
    pub created_at: String,
}
