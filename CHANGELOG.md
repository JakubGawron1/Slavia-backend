# Changelog

## [3.2.4] — 2026-05-19

### Naprawione
- **CMS GitHub delete**: poprawne odczytywanie `sha` z odpowiedzi Contents API (pole na poziomie głównym), kodowanie ścieżki segmentami, normalizacja URL z `SLAVIA_CMS_BASE_URL`, logi przy nieudanym usuwaniu.
- **Cache HTTP**: endpointy `/manage` i `/admin` mają `private, no-store` (nie są już traktowane jak publiczna lista ogłoszeń/wpisów).
- **Ćwiczenia kadry**: `PATCH /api/exercises/{id}`, lepsze `404` przy usuwaniu nieistniejącego ćwiczenia.

### Ulepszone
- **Błędy API**: pole `message` po polsku dla typowych kodów, opcjonalne `code` i `detail` (`api_validation_error`).
- **Indeksy DB**: `posts(published, created_at)`, `announcements(published, pinned, sort_order, created_at)`.
