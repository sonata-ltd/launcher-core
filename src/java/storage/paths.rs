//! Managed SQL placement paths are relative to an explicitly admitted Java root.
//! Source alone does not make an external/old relocated path owned. Never guess
//! an old root; unknown paths remain unchanged and must defer future retention.
use crate::{
    data::db::{Database, DbError, Result},
    utils::{files, object_store::ObjectStore},
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

#[derive(Default, Debug)]
pub(crate) struct Report {
    pub converted: usize,
    pub unresolved: usize,
}

pub(super) fn relative(root: &Path, path: &Path) -> Option<PathBuf> {
    let canonical = root.canonicalize().ok();
    let relative = if path.is_absolute() {
        path.strip_prefix(root).ok().or_else(|| {
            canonical
                .as_ref()
                .and_then(|root| path.strip_prefix(root).ok())
        })?
    } else {
        path
    };
    if !relative.starts_with("managed")
        || relative == Path::new("managed")
        || relative.to_str()?.contains('\\')
    {
        return None;
    }
    files::child(root, relative).ok()?;
    Some(relative.to_owned())
}

pub(super) fn pair(root: &Path, exec: &Path, home: &Path) -> Option<(String, String)> {
    let exec = relative(root, exec)?;
    let home = relative(root, home)?;
    let child = exec.strip_prefix(&home).ok()?;
    files::relative(child).ok()?;
    Some((exec.to_str()?.to_owned(), home.to_str()?.to_owned()))
}

pub(super) fn resolve(root: &Path, path: &Path) -> Result<String> {
    let relative = relative(root, path)
        .ok_or_else(|| DbError::InvalidInput("unsafe managed Java relative placement".into()))?;
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(_) if root.is_absolute() => root.to_owned(),
        Err(_) => std::env::current_dir()
            .map_err(|e| DbError::InvalidInput(e.to_string()))?
            .join(root),
    };
    let resolved =
        files::child(&root, &relative).map_err(|e| DbError::InvalidInput(e.to_string()))?;
    resolved
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| DbError::InvalidInput("non-Unicode Java placement".into()))
}

pub(crate) async fn migrate(db: Arc<Database>, configured: PathBuf) -> Result<Report> {
    async_std::task::spawn(async move {
        let scope = files::child(&configured, Path::new(".managed"))
            .map_err(|e| DbError::InvalidInput(e.to_string()))?;
        // This is the same resource admission held by managed install/validation.
        // No bytes/record/identity are deleted; SQL is locked until commit even
        // when a migration caller is dropped.
        let _maintenance = ObjectStore::new(scope)
            .try_maintenance()
            .map_err(|e| DbError::InvalidInput(format!("Java path upgrade: {e}")))?;
        let mut tx = db.pool.begin_with("BEGIN IMMEDIATE").await?;
        let rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT id,exec_path,home_path FROM java_runtimes WHERE source='managed'",
        )
        .fetch_all(&mut *tx)
        .await?;
        let mut report = Report::default();
        for (id, exec, home) in rows {
            let Some((relative_exec, relative_home)) =
                pair(&configured, Path::new(&exec), Path::new(&home))
            else {
                report.unresolved += 1;
                continue;
            };
            if exec == relative_exec && home == relative_home {
                continue;
            }
            let conflict: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM java_runtimes WHERE exec_path=? AND id!=?)",
            )
            .bind(&relative_exec)
            .bind(id)
            .fetch_one(&mut *tx)
            .await?;
            if conflict {
                report.unresolved += 1;
                continue;
            }
            sqlx::query("UPDATE java_runtimes SET exec_path=?,home_path=? WHERE id=?")
                .bind(relative_exec)
                .bind(relative_home)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            report.converted += 1;
        }
        #[cfg(test)]
        super::path_tests::checkpoint(&configured).await;
        tx.commit().await?;
        #[cfg(test)]
        super::path_tests::crash_checkpoint(&configured, "committed");
        Ok(report)
    })
    .await
}
