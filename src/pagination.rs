//! Wspólne parametry `limit` / `offset` dla list API.

use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct ListPaginationQuery {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

/// Domyślny limit, clamp do `[1, max_limit]`.
pub fn parse_list_pagination(
    query: &ListPaginationQuery,
    default_limit: u32,
    max_limit: u32,
) -> (u32, u32) {
    let limit = query
        .limit
        .unwrap_or(default_limit)
        .clamp(1, max_limit);
    let offset = query.offset.unwrap_or(0);
    (limit, offset)
}
