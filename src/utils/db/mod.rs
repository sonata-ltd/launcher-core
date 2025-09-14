use crate::{
    data::{
        db::{Database, Result},
        GlobalDataState,
    },
    instance::{
        list::InstanceDataRow,
        options::pages::{
            overview::{ExportTypes, Overview},
            settings::Settings,
        },
        Instance,
    },
};

/// Register instance in DB and retrieve its id
pub async fn register_instance<'a>(
    db: &Database,
    global_data_state: &GlobalDataState<'a>,
    instance: &Instance,
) -> Result<i64> {
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
    )
    .insert(&db, rec.id)
    .await?;

    let _ = &global_data_state
        .add_instance(InstanceDataRow::new_shared(
            rec.id,
            Some(instance_name.clone()),
            version.clone(),
            String::from("vanilla"),
        ))
        .await;

    Ok(rec.id)
}
