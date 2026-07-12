//! Globalny mutex dla testów integracyjnych modyfikujących `REBUILD_DB` (env procesu).
use std::sync::{Mutex, MutexGuard};

static INTEGRATION_ENV_LOCK: Mutex<()> = Mutex::new(());

pub fn integration_env_guard() -> MutexGuard<'static, ()> {
    INTEGRATION_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
