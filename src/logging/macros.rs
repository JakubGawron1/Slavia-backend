// Makra logów Slavia — format: `[LEVEL]:    {where}   (because)   {[fix]} extra | fields`.

/// Log INFO w stylu Slavia.
#[macro_export]
macro_rules! slavia_info {
    ($where:expr, $because:expr, $fix:expr $(, $($rest:tt)* )? ) => {{
        tracing::info!(
            slavia_where = $where,
            slavia_because = $because,
            slavia_fix = $fix
            $(, $($rest)* )?
        )
    }};
}

/// Log WARN w stylu Slavia.
#[macro_export]
macro_rules! slavia_warn {
    ($where:expr, $because:expr, $fix:expr $(, $($rest:tt)* )? ) => {{
        tracing::warn!(
            slavia_where = $where,
            slavia_because = $because,
            slavia_fix = $fix
            $(, $($rest)* )?
        )
    }};
}

/// Log ERROR w stylu Slavia.
#[macro_export]
macro_rules! slavia_error {
    ($where:expr, $because:expr, $fix:expr $(, $($rest:tt)* )? ) => {{
        tracing::error!(
            slavia_where = $where,
            slavia_because = $because,
            slavia_fix = $fix
            $(, $($rest)* )?
        )
    }};
}

/// Log DEBUG w stylu Slavia.
#[macro_export]
macro_rules! slavia_debug {
    ($where:expr, $because:expr, $fix:expr $(, $($rest:tt)* )? ) => {{
        tracing::debug!(
            slavia_where = $where,
            slavia_because = $because,
            slavia_fix = $fix
            $(, $($rest)* )?
        )
    }};
}

/// Skrót: `where` = nazwa pliku z `file!()`.
#[macro_export]
macro_rules! slavia_info_here {
    ($because:expr, $fix:expr $(, $($rest:tt)* )? ) => {{
        tracing::info!(
            slavia_where = $crate::logging::file_name(file!()),
            slavia_because = $because,
            slavia_fix = $fix
            $(, $($rest)* )?
        )
    }};
}

#[macro_export]
macro_rules! slavia_warn_here {
    ($because:expr, $fix:expr $(, $($rest:tt)* )? ) => {{
        tracing::warn!(
            slavia_where = $crate::logging::file_name(file!()),
            slavia_because = $because,
            slavia_fix = $fix
            $(, $($rest)* )?
        )
    }};
}

#[macro_export]
macro_rules! slavia_error_here {
    ($because:expr, $fix:expr $(, $($rest:tt)* )? ) => {{
        tracing::error!(
            slavia_where = $crate::logging::file_name(file!()),
            slavia_because = $because,
            slavia_fix = $fix
            $(, $($rest)* )?
        )
    }};
}

#[macro_export]
macro_rules! slavia_debug_here {
    ($because:expr, $fix:expr $(, $($rest:tt)* )? ) => {{
        tracing::debug!(
            slavia_where = $crate::logging::file_name(file!()),
            slavia_because = $because,
            slavia_fix = $fix
            $(, $($rest)* )?
        )
    }};
}
