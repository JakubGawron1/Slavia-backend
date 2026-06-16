# Wdrożenie Slavia-backend na Hugging Face Spaces (Docker)

## Wymagania

- Konto [Hugging Face](https://huggingface.co/) z utworzonym **Docker Space**
- Baza **Turso** (lokalny SQLite na Space ginie po restarcie)
- `JWT_SECRET` ≥ 32 znaki
- Repozytorium **Slavia-backend** na GitHubie

## Deploy z GitHub (zalecane)

Każdy push na `main` (gdy zmienią się pliki backendu) automatycznie synchronizuje kod ze Space.

### 1. Jednorazowa konfiguracja

**Hugging Face**

1. Utwórz Space: **New Space** → SDK: **Docker** (nazwa np. `slavia-backend`).
2. W Space → **Settings → Variables and secrets** ustaw sekrety runtime (patrz sekcja poniżej).

**GitHub** (`Slavia-backend` → Settings → Secrets and variables → Actions)

| Typ | Nazwa | Wartość |
|-----|-------|---------|
| Secret | `HF_TOKEN` | Token HF z uprawnieniem **write** ([API Tokens](https://huggingface.co/settings/tokens)) |
| Variable | `HF_SPACE_REPO` | `TWOJ_USER/TWOJ_SPACE` — np. `jakubgawron/slavia-backend` (bez `spaces/`) |

**Uprawnienia tokenu:** token musi mieć dostęp do zapisu w repozytorium Space (Fine-grained: write do tego Space, lub klasyczny token z write).

### 2. Jak to działa

Workflow `.github/workflows/deploy-huggingface-space.yml`:

1. Kopiuje `deploy/huggingface/README.md` → `README.md` (wymagany YAML z `sdk: docker`).
2. Uruchamia oficjalną akcję [`huggingface/hub-sync`](https://huggingface.co/docs/hub/spaces-github-actions) — mirror plików na Space.
3. HF buduje obraz z `Dockerfile` i uruchamia kontener.

Ręczny deploy: **Actions** → **Deploy Hugging Face Space** → **Run workflow**.

### 3. Smoke test

```bash
curl -s "https://TWOJ-USER-TWOJ-SPACE.hf.space/api/posts" | head
```

URL publiczny: `https://{user}-{space}.hf.space` (myślnik zamiast `/`).

---

## Sekrety runtime w Space (HF panel)

**Settings → Variables and secrets** w Space (nie w GitHubie):

```
JWT_SECRET=...
TURSO_DATABASE_URL=libsql://...
TURSO_AUTH_TOKEN=...
CORS_ALLOWED_ORIGINS=https://cksslavia.vercel.app,http://localhost:3000
```

Opcjonalnie: `GROQ_API_KEY`, `CLOUDINARY_*`, `GITHUB_TOKEN` — patrz `.env.example`.

> **Uwaga:** sekrety aplikacji (JWT, Turso) trzymasz w panelu **Hugging Face Space**, nie w GitHub Actions. GitHub Actions wysyła tylko kod źródłowy.

---

## Podłączenie frontendu (Vercel)

| Zmienna | Przykład |
|---------|----------|
| `NUXT_PUBLIC_API_BASE_URL_HUGGINGFACE` | `https://twoj-user-slavia-backend.hf.space` |
| `DEFAULT_BACKEND_PROVIDER` | `huggingface` (opcjonalnie) |

Bez końcowego slasha w URL.

Globalne przełączenie: SuperAdmin → `/superadmin/developer` → **Hugging Face** → **Zapisz globalnie**.

---

## Ograniczenia HF Spaces

| Temat | Zalecenie |
|-------|-----------|
| Dysk | Tylko Turso — brak trwałego SQLite |
| Sleep / cold start | Pierwsze żądanie po bezczynności może trwać dłużej |
| Uploady | Cloudinary (jak na innych hostach) |
| Build Rusta | Pierwszy build (cargo-chef) ~10–20 min |
| Pliki > 10 MB | Wymagają Git LFS w repo GitHub |

---

## Ręczny deploy (bez GitHub Actions)

Jeśli nie chcesz CI, nadal możesz sklonować Space i pushować ręcznie:

```bash
git clone https://huggingface.co/spaces/TWOJ_USER/TWOJ_SPACE
# skopiuj pliki Slavia-backend, README z deploy/huggingface/README.md
git add . && git commit -m "Deploy" && git push
```

---

## Troubleshooting

| Problem | Rozwiązanie |
|---------|-------------|
| Workflow fail: brak `HF_TOKEN` / `HF_SPACE_REPO` | Uzupełnij secret i variable w GitHub |
| **403** przy sync | Token bez write lub brak dostępu do Space |
| **502** na Space | Logi w HF → często brak `JWT_SECRET` lub Turso |
| CORS w przeglądarce | Dodaj origin do `CORS_ALLOWED_ORIGINS` w panelu Space |
| Frontend na Leapcell | Ustaw `NUXT_PUBLIC_API_BASE_URL_HUGGINGFACE` + przełącz provider |
