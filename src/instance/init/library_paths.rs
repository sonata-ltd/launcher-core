//! Root-aware upgrade of legacy placement paths. Only paths proven to belong to
//! the admitted library root become relative; external/relocated/invalid paths
//! stay untouched, and continue to defer collection. Never guess a former root.
use super::error::Result;
use crate::{
    data::db::{Database, DbError},
    utils::{
        files,
        object_store::{catalog::RootCatalog, ObjectStore},
    },
};
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Default, Debug)]
pub(super) struct Report {
    pub converted: usize,
    pub unresolved: usize,
}

pub(super) async fn migrate(db: Arc<Database>, configured: PathBuf) -> Result<Report> {
    async_std::task::spawn(async move {
        let maintenance = ObjectStore::new(configured.clone()).try_maintenance()?;
        let owner = db.retention_owner().await.map_err(DbError::from)?;
        if !RootCatalog::covers(maintenance.root(), owner.as_ref())? {
            return Err(io::Error::other("uncatalogued library path migration").into());
        }
        let mut tx = db.pool.begin_with("BEGIN IMMEDIATE").await.map_err(DbError::from)?;
        let mut report = Report::default();
        // Both tables participate in the retention gate. The old table remains
        // an upgrade snapshot; normal writes still target instance placements.
        let placements: Vec<(i64, i64, String)> = sqlx::query_as(
            "SELECT instance_id,position,path FROM instance_library_placements",
        ).fetch_all(&mut *tx).await.map_err(DbError::from)?;
        for (instance, position, path) in placements {
            match relative(maintenance.root(), &configured, Path::new(&path)) {
                Some(relative) if Path::new(&path).is_absolute() => {
                    sqlx::query("UPDATE instance_library_placements SET path=? WHERE instance_id=? AND position=?")
                        .bind(relative).bind(instance).bind(position)
                        .execute(&mut *tx).await.map_err(DbError::from)?;
                    report.converted += 1;
                }
                Some(_) => {},
                None => report.unresolved += 1,
            }
        }
        let legacy: Vec<(i64, String)> = sqlx::query_as("SELECT id,path FROM libraries")
            .fetch_all(&mut *tx).await.map_err(DbError::from)?;
        for (id, path) in legacy {
            match relative(maintenance.root(), &configured, Path::new(&path)) {
                Some(relative) if Path::new(&path).is_absolute() => {
                    sqlx::query("UPDATE libraries SET path=? WHERE id=?").bind(relative).bind(id)
                        .execute(&mut *tx).await.map_err(DbError::from)?;
                    report.converted += 1;
                }
                Some(_) => {},
                None => report.unresolved += 1,
            }
        }
        #[cfg(test)]
        tests::checkpoint(maintenance.root()).await;
        tx.commit().await.map_err(DbError::from)?;
        Ok(report)
    }).await
}

fn relative(canonical: &Path, configured: &Path, path: &Path) -> Option<String> {
    let relative = if path.is_absolute() {
        path.strip_prefix(canonical)
            .or_else(|_| path.strip_prefix(configured))
            .ok()?
    } else {
        path
    };
    let text = relative.to_str()?;
    if text.contains('\\') {
        return None;
    }
    files::child(canonical, relative).ok()?;
    Some(text.to_owned())
}

#[cfg(test)]
mod tests;
