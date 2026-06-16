# SQL — migracje i schemat (Slavia-backend)

Backend używa **libsql** (SQLite / Turso). Logika biznesowa pozostaje w Rust; pliki `.sql` służą do:

- **wersjonowanych migracji** (`migrations/`) — idempotentne DDL (indeksy, kolumny),
- **czytelności** — zapytania można przeglądać i diffować bez przeszukiwania `db.rs`.

## Kolejność przy starcie (`init_db`)

1. `CREATE TABLE IF NOT EXISTS` + legacy `ALTER TABLE` w `src/db.rs` (istniejące instalacje).
2. **`schema_migrations`** + pliki z `sql/migrations/` (`src/db_migrations.rs`).
3. Seed i migracje Rust (`migrate_*`, ćwiczenia, CMS).

Nowe zmiany schematu **preferuj** jako plik `sql/migrations/YYYYMMDD_NNN_opis.sql`, rejestrowany w `MIGRATIONS` w `db_migrations.rs`.

## Zasady migracji SQL

- Każdy plik musi być **idempotentny** (`IF NOT EXISTS`, bezpieczne `CREATE INDEX`).
- Jedna migracja = jeden logiczny krok (np. paczka indeksów wydajnościowych).
- Złożone migracje danych (rebuild tabeli `users`, CMS legacy) zostają w Rust — SQL tylko tam, gdzie to czyste DDL.
- **Nie** ustawiaj `REBUILD_DB=true` na produkcji.

## Śledzenie wersji

Tabela `schema_migrations (version PRIMARY KEY, applied_at)` — każdy plik uruchamiany co najwyżej raz.

## Zapytania w trasach

Ciężkie zapytania listowe trzymaj w `src/sql/queries/` (stałe `pub const …: &str`) i importuj w handlerach. Preferuj `JOIN` + `GROUP BY` zamiast skorelowanych podzapytań w pętli.
