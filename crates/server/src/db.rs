//! Database setup: bootstrap (role + database, with an admin DSN) and embedded migrations.

use std::str::FromStr;

use anyhow::{Context, bail};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, Connection, Executor, PgConnection, PgPool};

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!();

pub async fn connect(database_url: &str) -> anyhow::Result<PgPool> {
    PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await
        .context("connect to Postgres")
}

/// Applies pending migrations as the app role.
pub async fn migrate(database_url: &str) -> anyhow::Result<()> {
    let pool = connect(database_url).await?;
    MIGRATOR.run(&pool).await.context("run migrations")?;
    pool.close().await;
    Ok(())
}

/// Whether every embedded migration has been applied.
pub async fn migrations_current(pool: &PgPool) -> anyhow::Result<bool> {
    let applied: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations WHERE success")
        .fetch_one(pool)
        .await?;
    Ok(applied as usize >= MIGRATOR.iter().count())
}

/// Creates (or updates) the app role and database named in `app_url`, using `admin_url`, whose
/// role needs only CREATEROLE and CREATEDB. Idempotent.
pub async fn bootstrap(admin_url: &str, app_url: &str) -> anyhow::Result<()> {
    let app = PgConnectOptions::from_str(app_url).context("parse BLUE_DATABASE_URL")?;
    let role = app.get_username().to_owned();
    let database = app
        .get_database()
        .context("BLUE_DATABASE_URL must name a database")?
        .to_owned();
    let password = url::Url::parse(app_url)
        .ok()
        .and_then(|url| url.password().map(percent_decode))
        .context("BLUE_DATABASE_URL must include the app role's password")?;
    if role.is_empty() {
        bail!("BLUE_DATABASE_URL must include a user name");
    }

    let mut admin = PgConnectOptions::from_str(admin_url)
        .context("parse the admin DSN")?
        .connect()
        .await
        .context("connect with the admin DSN")?;

    let role_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1)")
            .bind(&role)
            .fetch_one(&mut admin)
            .await?;
    // Always set the password, so the role matches the app Secret after it changes.
    let verb = if role_exists { "ALTER" } else { "CREATE" };
    admin
        .execute(
            format!(
                "{verb} ROLE {} LOGIN PASSWORD {}",
                ident(&role),
                literal(&password)
            )
            .as_str(),
        )
        .await
        .with_context(|| format!("{verb} ROLE {role}"))?;
    tracing::info!(%role, "role ready");

    // Since Postgres 16 a CREATEROLE admin can only make a role own a database if it can SET
    // ROLE to it, which takes membership. Superusers are members of everything already.
    let is_member: bool = sqlx::query_scalar("SELECT pg_has_role(current_user, $1, 'SET')")
        .bind(&role)
        .fetch_one(&mut admin)
        .await?;
    if !is_member {
        admin
            .execute(format!("GRANT {} TO current_user", ident(&role)).as_str())
            .await
            .context("grant the app role to the admin role")?;
    }

    let database_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(&database)
            .fetch_one(&mut admin)
            .await?;
    if !database_exists {
        admin
            .execute(
                format!(
                    "CREATE DATABASE {} OWNER {}",
                    ident(&database),
                    ident(&role)
                )
                .as_str(),
            )
            .await
            .with_context(|| format!("CREATE DATABASE {database}"))?;
    }
    tracing::info!(%database, "database ready");
    admin.close().await?;

    // Lock down the public schema. Done as the app role, which owns the database and therefore
    // the schema (pg_database_owner), so the admin needs no rights inside it.
    let mut app = PgConnection::connect_with(&app)
        .await
        .context("connect as the app role")?;
    app.execute(
        format!(
            "REVOKE ALL ON SCHEMA public FROM PUBLIC; ALTER SCHEMA public OWNER TO {}",
            ident(&role)
        )
        .as_str(),
    )
    .await
    .context("lock down the public schema")?;
    app.close().await?;
    Ok(())
}

fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn percent_decode(value: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", value.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, value)| value.into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(ident(r#"we"ird"#), r#""we""ird""#);
        assert_eq!(literal("it's"), "'it''s'");
        assert_eq!(percent_decode("p%40ss+word"), "p@ss+word");
    }
}
