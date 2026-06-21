# Changelog

## [5.1.0] — 2026-06-21

### improvements/all (API, observability)
- Agregaty `GET /api/athletes/me/dashboard` i `GET /api/trainer/dashboard`.
- Role preview (read-only), cleanup ACL i unwrap w routes.
- `/metrics` (Prometheus), kompresja gzip/br, workflow keep-warm HF, alias `GET /api/health`.

### Wydajność i API publiczne
- **`list_public_results_board`**: `LIMIT 500` na tablicy wyników publicznych.
- **`sync_all_athletes_bests_from_results`**: uruchamiane tylko przy `REBUILD_DB=true` (szybszy zwykły start).

## [5.0.0] — 2026-06-07

### Trener AI (Groq / LLaMA)
- **`/api/ai/coach/*`**: czat, status, import planu treningowego; provider **Groq** (`GROQ_API_KEY`, model `llama-3.1-70b-versatile` z fallbackiem 3.3).
- **Limity**: throttling per użytkownik (4/min, 40/dzień czat; 3/h, 10/dzień import) + globalny limit klucza klubu.
- **Asystent publiczny**: `GET /api/ai/coach/public/status`, `POST /api/ai/coach/public/chat` (bez JWT, limit per IP).
- Kontekst zawodnika w prompcie: dziennik, wyniki z zawodów, obecności, aktywny plan klubowy.
- Usunięto endpointy BYOK (`/my-key`).

## [3.2.5] — 2026-05-19

### Ulepszone
- **Wpisy CMS (`/api/posts`)** — walidacja pustego tytułu lub treści przy tworzeniu i edycji (komunikat po polsku, `validation_error`).
- **Motywy profilu (ui_theme_preset)** — glass, sport-tech, neon-brutalism.
- **GET /api/auth/me** — pole roles w odpowiedzi.

## [3.2.4] — 2026-05-19

### Naprawione
- **CMS GitHub delete**: poprawne odczytywanie `sha` z odpowiedzi Contents API (pole na poziomie głównym), kodowanie ścieżki segmentami, normalizacja URL z `SLAVIA_CMS_BASE_URL`, logi przy nieudanym usuwaniu.
- **Cache HTTP**: endpointy `/manage` i `/admin` mają `private, no-store` (nie są już traktowane jak publiczna lista ogłoszeń/wpisów).
- **Ćwiczenia kadry**: `PATCH /api/exercises/{id}`, lepsze `404` przy usuwaniu nieistniejącego ćwiczenia.

### Ulepszone
- **Błędy API**: pole `message` po polsku dla typowych kodów, opcjonalne `code` i `detail` (`api_validation_error`).
- **Indeksy DB**: `posts(published, created_at)`, `announcements(published, pinned, sort_order, created_at)`.
