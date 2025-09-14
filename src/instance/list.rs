use async_std::stream::StreamExt;
use serde::Serialize;

use crate::
    data::{
        db::{Database, Result},
        GlobalDataState,
    }
;

#[derive(Debug, Serialize)]
pub struct InstanceDataRow {
    pub id: i64,
    pub name: Option<String>,
    pub version: String,
    pub loader: String,
}

pub async fn get_instances<'a>(
    db: &Database,
    global_data_state: &GlobalDataState<'a>
) -> Result<()> {
    let mut stream = sqlx::query_as!(
        InstanceDataRow,
        r#"
        SELECT i.id, o.name, i.version, i.loader
        FROM instances i
        LEFT JOIN instances_overview o ON o.instance_id = i.id
        "#
    )
    .fetch(&db.pool);

    while let Some(row_res) = stream.next().await {
        let row = row_res?;
        println!("Added instance: {:#?}", row);

        global_data_state
            .add_instance(InstanceDataRow::new_shared(
                row.id,
                row.name,
                row.version,
                row.loader,
            ))
            .await.unwrap();
    }

    Ok(())
}
