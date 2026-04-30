use shuttle_axum::ShuttleAxum;

#[shuttle_runtime::main]
async fn axum(
    #[shuttle_runtime::Secrets] secrets: shuttle_runtime::SecretStore,
) -> ShuttleAxum {
    let db_url = secrets
        .get("TURSO_DATABASE_URL")
        .expect("TURSO_DATABASE_URL is missing in Secrets.toml");
    let db_token = secrets.get("TURSO_AUTH_TOKEN").unwrap_or_default();
    let jwt_secret = secrets
        .get("JWT_SECRET")
        .unwrap_or_else(|| "default_secret_for_dev_only".to_string());

    let app = slavia_backend::create_app(&db_url, &db_token, jwt_secret)
        .await
        .expect("Failed to create application");

    Ok(app.into())
}
