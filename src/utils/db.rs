// use crate::data::db::{DBError, Database, Result};

// pub async fn find_instance_by_hash(db: &Database, hash: &str) -> Result<i64> {
//     let rec = sqlx::query!(r#"SELECT id FROM instances WHERE hash = ?"#, hash)
//         .fetch_optional(&db.pool)
//         .await?;

//     let row = rec.ok_or_else(|| DBError::NotFound(format!("Instance not found: {}", hash)))?;
//     Ok(row.id)
// }
