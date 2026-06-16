---
title: Slavia Backend
emoji: 🏋️
colorFrom: green
colorTo: green
sdk: docker
app_port: 8080
pinned: false
license: mit
---

# Slavia Backend (Rust / Axum)

API klubu CKS Slavia Ruda Śląska — Docker Space na Hugging Face.

Publiczny URL Space (bez końcowego slasha):

`https://koliber-cks-slavia.hf.space`

## Wymagane zmienne (Settings → Variables and secrets)

| Klucz | Opis |
|-------|------|
| `JWT_SECRET` | Min. 32 znaki — **wymagane** przy Turso |
| `TURSO_DATABASE_URL` | URL bazy Turso |
| `TURSO_AUTH_TOKEN` | Token Turso |
| `CORS_ALLOWED_ORIGINS` | Np. `https://cksslavia.vercel.app,http://localhost:3000` |
| `PORT` | Ustawiane automatycznie przez HF (`8080` przy `app_port: 8080`) |

Opcjonalnie: `GROQ_API_KEY`, `CLOUDINARY_*`, `GITHUB_TOKEN` (upload CMS) — patrz `.env.example` w repo.

## Uwagi produkcyjne

- **Nie używaj** lokalnego SQLite na Space — dysk jest efemeryczny. Zawsze **Turso**.
- Nie ustawiaj `REBUILD_DB=true` na produkcji.
- Po pierwszym deployu sprawdź `GET /api/posts` (publiczny endpoint).

## Frontend (Vercel)

W env frontendu dodaj:

```
NUXT_PUBLIC_API_BASE_URL_HUGGINGFACE=https://koliber-cks-slavia.hf.space
```

Przełącznik globalny: `/superadmin/developer` → **Hugging Face** → Zapisz globalnie.

Deploy ze źródła: push na `main` w GitHub (`Slavia-backend`) — workflow `.github/workflows/deploy-huggingface-space.yml`.  
Pełna instrukcja: `docs/DEPLOY_HUGGINGFACE.md`.
