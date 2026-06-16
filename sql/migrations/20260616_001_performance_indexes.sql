-- Paczka indeksów wydajnościowych (idempotentna).
-- Uruchamiana przez schema_migrations; bezpieczna na istniejących bazach Turso/SQLite.

CREATE INDEX IF NOT EXISTS idx_athletes_user_id ON athletes(user_id);

CREATE INDEX IF NOT EXISTS idx_results_date ON results(date DESC);

CREATE INDEX IF NOT EXISTS idx_results_athlete_bests
  ON results(athlete_id, total DESC, date DESC);

CREATE INDEX IF NOT EXISTS idx_chat_messages_thread_created
  ON chat_messages(thread_id, created_at ASC);

CREATE INDEX IF NOT EXISTS idx_chat_threads_athlete_updated
  ON chat_threads(athlete_user_id, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_chat_threads_trainer_updated
  ON chat_threads(trainer_user_id, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_chat_reactions_message
  ON chat_message_reactions(message_id);

CREATE INDEX IF NOT EXISTS idx_comp_participants_athlete
  ON competition_participants(athlete_id);

CREATE INDEX IF NOT EXISTS idx_training_log_athlete_date
  ON training_log_entries(athlete_id, session_date DESC, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_attendance_session_date
  ON attendance_records(session_date DESC);

CREATE INDEX IF NOT EXISTS idx_notifications_unread
  ON notifications(is_read)
  WHERE is_read = 0;

CREATE INDEX IF NOT EXISTS idx_training_plans_status
  ON training_plans(status);

CREATE INDEX IF NOT EXISTS idx_recovery_date
  ON recovery_logs(date DESC);
