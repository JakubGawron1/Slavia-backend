use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum Role {
    SuperAdmin,
    Admin,
    TrainerAdmin,
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
            Role::TrainerAdmin => "TrainerAdmin",
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
            "TrainerAdmin" => Ok(Role::TrainerAdmin),
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
    pub email: Option<String>,
    pub avatar_url: Option<String>,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: Role,
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
    pub is_active: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum ResultStatus {
    Pending,
    Approved,
}

impl std::fmt::Display for ResultStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResultStatus::Pending => write!(f, "Pending"),
            ResultStatus::Approved => write!(f, "Approved"),
        }
    }
}

impl std::str::FromStr for ResultStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Pending" => Ok(ResultStatus::Pending),
            "Approved" => Ok(ResultStatus::Approved),
            _ => Err(format!("Invalid status: {}", s)),
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
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Competition {
    pub id: String,
    pub title: String,
    pub date: String,
    pub location: String,
    pub description: Option<String>,
    pub category: Option<String>, // "championship", "league", "club_event", "training"
    pub status: Option<String>, // "scheduled", "cancelled", "moved"
    /// np. `pzpc`, `podnoszenieciezarow` — brak = zawody utworzone w klubie
    #[serde(default)]
    pub external_source: Option<String>,
    #[serde(default)]
    pub external_ref: Option<String>,
    #[serde(default)]
    pub external_url: Option<String>,
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
