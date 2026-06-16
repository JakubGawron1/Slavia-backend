pub const BATCH_APPROVE_SELECT_APPROVED: &str =
    "SELECT athlete_id, total, date FROM results WHERE status = 'Approved' AND id IN ";
