# Observability — Prometheus (`GET /metrics`)

Stub metryk HTTP (OBS-1 / H-4): liczniki żądań i błędów 4xx/5xx. Włączany **opcjonalnie** zmienną `PROMETHEUS_METRICS=1`.

> **Nie mylić** z `GET /api/system/metrics` — to autoryzowany JSON KPI dla panelu trenera (`RequireTrainerOrHigher`), nie format Prometheus.

---

## Włączenie

| Środowisko | Gdzie ustawić |
|------------|---------------|
| Lokalnie | `.env` → `PROMETHEUS_METRICS=1` (patrz `.env.example`) |
| Hugging Face Space | **Settings → Variables and secrets** → `PROMETHEUS_METRICS=1` |
| Render (deprecated) | Panel env / `Secrets.toml` |

Akceptowane wartości (case-insensitive): `1`, `true`, `yes`, `on`. Domyślnie **wyłączone**.

Po starcie backend loguje:

```text
Prometheus: GET /metrics włączony (PROMETHEUS_METRICS)
```

---

## Endpoint

| Właściwość | Wartość |
|------------|---------|
| Metoda / ścieżka | `GET /metrics` (root aplikacji, **nie** pod `/api`) |
| Content-Type | `text/plain; version=0.0.4; charset=utf-8` |
| Auth | **Brak** — endpoint publiczny (tylko agregaty liczników) |
| Kod źródłowy | `src/http_metrics.rs`, rejestracja w `src/router.rs` |

Przykład URL na HF (bez końcowego slasha w bazie):

```text
https://koliber-cks-slavia.hf.space/metrics
```

### Eksportowane metryki

| Nazwa | Typ | Opis |
|-------|-----|------|
| `slavia_http_requests_total` | counter | Wszystkie obsłużone żądania HTTP (middleware) |
| `slavia_http_errors_total` | counter | Odpowiedzi ze statusem ≥ 400 |

Przykładowa odpowiedź:

```text
# HELP slavia_http_requests_total Total HTTP requests served.
# TYPE slavia_http_requests_total counter
slavia_http_requests_total 42
# HELP slavia_http_errors_total HTTP responses with status >= 400.
# TYPE slavia_http_errors_total counter
slavia_http_errors_total 3
```

Liczniki są **in-memory per proces** — po restarcie Space zerują się. Przy wielu replikach każda instancja ma własny zestaw (brak agregacji cross-instance w stubie).

---

## Weryfikacja (curl)

```bash
# Lokalnie (backend na :8080)
curl -sS http://127.0.0.1:8080/metrics | head

# Hugging Face (po ustawieniu PROMETHEUS_METRICS=1 i redeploy)
curl -sS "https://koliber-cks-slavia.hf.space/metrics" | grep slavia_http
```

Gdy `PROMETHEUS_METRICS` nie jest ustawione, `GET /metrics` zwraca **404** (trasa nie jest rejestrowana).

---

## Scrape Prometheus (zewnętrzny)

HF Space nie hostuje wbudowanego Prometheusa — scraper musi działać **na zewnątrz** (VPS, Grafana Cloud, self-hosted).

### Przykład `prometheus.yml`

```yaml
scrape_configs:
  - job_name: slavia-backend-hf
    scrape_interval: 60s
    scrape_timeout: 30s
    metrics_path: /metrics
    scheme: https
    static_configs:
      - targets:
          - koliber-cks-slavia.hf.space
        labels:
          env: production
          service: slavia-backend
```

### Uwagi operacyjne (HF Spaces)

| Temat | Zalecenie |
|-------|-----------|
| Cold start | Pierwsze scrape po uśpieniu Space może timeoutować — rozważ `keep-warm.yml` (`GET /api/system/ping`) obok scrapera |
| Interwał | ≥ 60 s — unikaj agresywnego scrapingu na free tier |
| Bezpieczeństwo | Endpoint bez auth; eksponuje tylko liczniki — **nie** włączaj na instancjach z wrażliwymi debug endpointami |
| TLS | HF terminuje HTTPS — używaj `scheme: https` |

### Grafana

Po skonfigurowaniu datasource Prometheus typowe zapytania:

```promql
rate(slavia_http_requests_total[5m])
rate(slavia_http_errors_total[5m])
```

---

## Powiązane endpointy (nie Prometheus)

| Ścieżka | Cel |
|---------|-----|
| `GET /api/health` | Liveness — JSON ping (`ping_backend`) |
| `GET /api/system/ping` | Ping systemowy (keep-warm, smoke) |
| `GET /api/system/metrics` | JSON KPI + audit (JWT, trener+) |

Frontend: [`Slavia-frontend/docs/observability.md`](../../Slavia-frontend/docs/observability.md) — smoke po deployu i linki operatorskie.

---

## CI — stub zewnętrznego pinga

Workflow `.github/workflows/metrics-scrape-stub.yml` (opcjonalny):

- Secret `HF_API_BASE_URL` — ten sam co w `keep-warm.yml`
- `GET {base}/metrics` co godzinę; job pomijany gdy sekret pusty
- Służy jako **canary** dostępności metryk, nie zastępuje Prometheusa

Ręcznie: **Actions → Metrics scrape stub (HF) → Run workflow**.

---

## Roadmap (poza stubem)

Pełniejszy OBS-1 (histogram latency, metryki workerów Groq, OpenTelemetry) — backlog w `Slavia-frontend/improve.md` (BE-G11, OBS-3).
