//! Pomocnicze budowanie zapytań SQL (placeholdery `?1..?N` dla libsql).

/// `?1, ?2, … ?count` — do klauzuli `IN (...)`.
pub fn in_placeholders(count: usize) -> String {
    (1..=count)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}
