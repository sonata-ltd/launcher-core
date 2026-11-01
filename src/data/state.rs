//! Small "done once" facts about this launcher install, kept in
//! `launcher_state`.

use crate::data::db::{Database, Result};

pub const JAVA_DISCOVERY_DONE: &str = "java_discovery_done";

pub async fn get_flag(db: &Database, key: &str) -> Result<bool> {
    let v: Option<String> =
        sqlx::query_scalar!("SELECT value FROM launcher_state WHERE key = ?", key)
            .fetch_optional(&db.pool)
            .await?;

    Ok(v.as_deref() == Some("1"))
}

pub async fn set_flag(db: &Database, key: &str) -> Result<()> {
    sqlx::query!(
        "INSERT INTO launcher_state(key, value) VALUES (?, '1')
        ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        key
    )
    .execute(&db.pool)
    .await?;
    Ok(())
}
