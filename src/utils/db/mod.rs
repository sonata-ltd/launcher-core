use crate::{
    data::db::{Database, Result},
    instance::{
        options::pages::{overview::{ExportTypes, Overview}, settings::Settings},
        Instance,
    },
};

/// Register instance in DB and retrieve its id
pub async fn register_instance(db: &Database, instance: &Instance) -> Result<i64> {
    let instance_name = &instance.name;
    let version = instance.version_id();
    let dir = instance.paths().instance();

    let rec = sqlx::query!(
        r#"
        INSERT INTO instances (version, loader)
        VALUES (?1, ?2)
        RETURNING id
        "#,
        version,
        "vanilla"
    )
    .fetch_one(&db.pool)
    .await?;

    Settings::upset(&db, rec.id, dir).await?;
    Overview::new(
        instance_name.clone(),
        String::new(),
        ExportTypes::Sonata,
        0 as i64,
    ).insert(&db, rec.id).await?;

    Ok(rec.id)
}
