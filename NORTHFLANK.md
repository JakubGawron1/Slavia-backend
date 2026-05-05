# Northflank: najszybszy build dla Slavia-backend

## Co wybrać: Dockerfile czy Buildpack?

W tym projekcie szybszy i bardziej przewidywalny będzie **Dockerfile**:

- Rust ma ciężki cold build; `cargo-chef` w Dockerfile dobrze wykorzystuje cache warstw.
- Buildpack dla Rust bywa wygodny, ale zwykle mniej kontrolowalny i częściej przebudowuje zależności.
- Masz pełną kontrolę nad obrazem runtime i debugowaniem etapów.

## Konfiguracja usługi na Northflank

1. **Build type**: Dockerfile
2. **Dockerfile path**: `./Dockerfile`
3. **Port**: `8080`
4. **Start command**: zostaw domyślne z `CMD`

## Wymagane zmienne środowiskowe

- `PORT=8080`
- `DATABASE_MODE=turso`
- `TURSO_DATABASE_URL=...`
- `TURSO_AUTH_TOKEN=...`
- `JWT_SECRET=...`
- `CLOUDINARY_CLOUD_NAME=...` (opcjonalnie)
- `CLOUDINARY_API_KEY=...` (opcjonalnie)
- `CLOUDINARY_API_SECRET=...` (opcjonalnie)
- `REBUILD_DB=false`

## Uwaga o przełączaniu Leapcell/Northflank

Przełącznik źródła backendu jest utrzymywany po stronie **frontendu** (Nuxt server API + Blob),
więc backend Rust nie musi już udostępniać osobnych endpointów konfiguracyjnych.
