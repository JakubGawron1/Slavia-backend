//! Zgłoszenia wyników (rekordy `Pending`) — osobna przestrzeń URL od [`crate::routes::results`],
//! które obsługuje także zatwierdzone wyniki i pełne operacje kadry na tabeli `results`.
//!
//! Handlery są współdzielone z modułem `results`; tu tylko eksport pod `/api/submissions/*`.

pub use crate::routes::results::{approve_result, delete_result, list_pending_results};
