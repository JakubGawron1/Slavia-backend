pub const ATHLETES_PAYMENT_STATUS_FOR_MONTH: &str = "SELECT a.id, a.full_name,
    COALESCE(SUM(CASE WHEN p.status = 'Approved' THEN 1 ELSE 0 END), 0) AS paid_count
 FROM athletes a
 LEFT JOIN membership_payments p
   ON p.athlete_id = a.id AND p.month = ?1
 WHERE (a.is_active IS NULL OR a.is_active = 1)
 GROUP BY a.id, a.full_name
 ORDER BY a.full_name ASC";

pub const PAYMENTS_OVERVIEW_FOR_MONTH: &str = "SELECT
    a.id,
    a.full_name,
    COALESCE(SUM(CASE WHEN p.status = 'Approved' THEN 1 ELSE 0 END), 0) AS approved_count,
    COALESCE(SUM(CASE WHEN p.status = 'Pending' THEN 1 ELSE 0 END), 0) AS pending_count,
    COALESCE(SUM(CASE WHEN p.status = 'Approved' THEN COALESCE(p.amount_pln, 0) ELSE 0 END), 0) AS approved_sum,
    COALESCE(SUM(CASE WHEN p.status = 'Pending' THEN COALESCE(p.amount_pln, 0) ELSE 0 END), 0) AS pending_sum
 FROM athletes a
 LEFT JOIN membership_payments p
   ON p.athlete_id = a.id AND p.month = ?1
 WHERE (a.is_active IS NULL OR a.is_active = 1)
 GROUP BY a.id, a.full_name
 ORDER BY a.full_name ASC";
