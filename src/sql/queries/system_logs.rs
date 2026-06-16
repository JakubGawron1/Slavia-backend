pub const SYSTEM_METRICS_COUNTS: &str = "SELECT
    (SELECT COUNT(*) FROM athletes WHERE is_active IS NULL OR is_active = 1),
    (SELECT COUNT(*) FROM training_plans WHERE status IN ('planned','active')),
    (SELECT COUNT(*) FROM results WHERE status = 'Pending'),
    (SELECT COUNT(*) FROM notifications WHERE is_read = 0),
    (SELECT COUNT(*) FROM recovery_logs WHERE date >= date('now', '-7 day'))";

/// Kolumny: source, at, athlete_id, num1, num2, str1, str2
pub const EVENT_FEED_UNION: &str = "WITH
    result_events AS (
        SELECT 'results' AS source, date AS at, athlete_id,
               total AS num1, 0 AS num2, status AS str1, '' AS str2
        FROM results
        ORDER BY date DESC
        LIMIT 40
    ),
    attendance_events AS (
        SELECT 'attendance' AS source, session_date AS at, athlete_id,
               0 AS num1, 0 AS num2, status AS str1, verification_state AS str2
        FROM attendance_records
        ORDER BY session_date DESC
        LIMIT 40
    ),
    recovery_events AS (
        SELECT 'recovery' AS source, date AS at, athlete_id,
               sleep_hours AS num1, CAST(readiness_level AS REAL) AS num2, '' AS str1, '' AS str2
        FROM recovery_logs
        ORDER BY date DESC
        LIMIT 40
    )
SELECT source, at, athlete_id, num1, num2, str1, str2 FROM result_events
UNION ALL
SELECT source, at, athlete_id, num1, num2, str1, str2 FROM attendance_events
UNION ALL
SELECT source, at, athlete_id, num1, num2, str1, str2 FROM recovery_events
ORDER BY at DESC
LIMIT 120";
