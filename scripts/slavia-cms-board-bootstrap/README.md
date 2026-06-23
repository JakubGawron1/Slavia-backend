# Bootstrap folder `board/` w Slavia-cms

Skopiuj zawartość tego katalogu do root repozytorium [Slavia-cms](https://github.com/JakubGawron1/Slavia-cms) jako folder `board/`.

Wymagane podfoldery (puste pliki `.gitkeep` opcjonalnie):

- `athletes/`, `coaches/`, `competitions/`, `start-lists/`, `equipment/`
- `meeting-reports/`, `organizational/`, `financial/`, `hr/`, `legal/`, `marketing/`
- `templates/`, `archive/`

W `templates/` umieść pliki szablonów nazwane jak `id` typu z katalogu frontendu, np. `meeting_resolution.html`, `competition_start_list.csv`. API `GET /api/board/templates/{doc_type}` szuka kolejno w repo GitHub (`.html`, `.csv`, `.txt`, `.md`), a gdy brak — zwraca wbudowany katalog `src/embed/board-templates.json` (nagłówek `X-Slavia-Template-Source: embed`).

Wygeneruj pliki bootstrap z embed:

```bash
node scripts/generate-board-bootstrap-templates.mjs
```

Backend wymaga `GITHUB_TOKEN` (scope `repo`) i `SLAVIA_CMS_REPO`.
