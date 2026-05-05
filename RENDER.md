# Render: deploy Slavia-backend

## Co wybrać: Dockerfile czy Native Build?

W tym projekcie najlepszy będzie **Dockerfile**:

- Rust ma ciężki cold build; `cargo-chef` lepiej wykorzystuje cache warstw.
- Masz pełną kontrolę nad środowiskiem runtime i etapami builda.
- Zachowanie builda jest bardziej przewidywalne między deployami.

## Konfiguracja usługi na Render

1. Utwórz **Web Service** z repo backendu.
2. **Environment**: `Docker`.
3. **Dockerfile Path**: `./Dockerfile`.
4. **Instance Port**: `8080`.
5. Health check (opcjonalnie): `/`.

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

## Uwaga o przełączaniu Leapcell/Render

Przełącznik źródła backendu jest utrzymywany po stronie **frontendu** (Nuxt server API + Blob),
więc backend Rust nie musi udostępniać osobnych endpointów konfiguracyjnych.
