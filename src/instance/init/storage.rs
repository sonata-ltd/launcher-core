use crate::{
    data::db::{Database, DbError},
    instance::{init::model::InstalledProfile, model::InstanceId},
};

pub async fn load(
    db: &Database,
    id: InstanceId,
) -> std::result::Result<Option<InstalledProfile>, DbError> {
    let body: Option<String> =
        sqlx::query_scalar("SELECT profile_json FROM instance_installations WHERE instance_id = ?")
            .bind(id.get())
            .fetch_optional(&db.pool)
            .await?;

    body.map(|body| {
        let profile: InstalledProfile =
            serde_json::from_str(&body).map_err(|_| DbError::ResultCorrupted)?;

        if profile.format_version != 1 {
            return Err(DbError::InvalidInput(
                "unsupported installed profile version".into(),
            ));
        }

        Ok(profile)
    })
    .transpose()
}

pub async fn publish(
    db: &Database,
    id: InstanceId,
    profile: &InstalledProfile,
) -> std::result::Result<(), DbError> {
    let body = serde_json::to_string(profile).map_err(|_| DbError::ResultCorrupted)?;
    sqlx::query(
        "INSERT INTO instance_installations(instance_id, profile_json)
        VALUES (?, ?)
        ON CONFLICT(instance_id) DO UPDATE SET
            profile_json = excluded.profile_json,
            installed_at = datetime('now')",
    )
    .bind(id.get())
    .bind(body)
    .execute(&db.pool)
    .await?;

    Ok(())
}
