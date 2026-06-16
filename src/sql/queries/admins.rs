pub const ADMIN_ACCOUNTS: &str = "SELECT u.id, u.username, u.avatar_url, u.roles, u.is_banned, u.banned_reason,
    la.id AS athlete_id,
    la.image_url AS athlete_image_url,
    la.full_name AS athlete_full_name
    FROM users u
    LEFT JOIN (
        SELECT user_id, id, image_url, full_name,
               ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY id ASC) AS rn
        FROM athletes
        WHERE user_id IS NOT NULL
    ) la ON la.user_id = u.id AND la.rn = 1
    ORDER BY u.username ASC";
