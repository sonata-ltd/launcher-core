//! Detach files published by the old hardlink installer.
use std::{fs, io, path::Path};

use crate::{
    data::{db::Database, layout::LauncherPaths},
    instance::{content::error::Result, model::InstanceId, service::InstanceService},
    utils::files::{self, digest::FileStamp},
};

#[derive(sqlx::FromRow)]
struct Placement {
    id: i64,
    instance_id: InstanceId,
    dir: String,
    relative_path: String,
    sha256: String,
}

pub(in crate::instance::content) async fn detach_registered_links(
    db: &Database,
    paths: &LauncherPaths,
    instances: &InstanceService,
) -> Result<()> {
    let placements: Vec<Placement> = sqlx::query_as(
        "SELECT content.id, content.instance_id, instance.dir, content.relative_path, object.sha256
         FROM instance_content AS content
         JOIN instances AS instance ON instance.id=content.instance_id
         JOIN content_objects AS object ON object.id=content.object_id
         WHERE content.placement='hardlink'",
    )
    .fetch_all(&db.pool)
    .await?;

    for placement in placements {
        let _activity = match instances.reserve_content_change(placement.instance_id) {
            Ok(guard) => guard,
            Err(error) => {
                tracing::warn!(instance = %placement.instance_id, %error, "legacy mod copy deferred");
                continue;
            }
        };
        let root = paths.instances().to_owned();
        let id = placement.id;
        let outcome = async_std::task::spawn_blocking(move || {
            let instance = files::child(&root, Path::new(&placement.dir))?;
            let path = files::child(&instance, Path::new(&placement.relative_path))?;
            Ok::<_, crate::utils::files::FileError>(detach(&path, &placement.sha256)?)
        })
        .await;
        match outcome {
            Ok(Some(true)) => {
                sqlx::query("UPDATE instance_content SET placement='copy' WHERE id=? AND placement='hardlink'")
                    .bind(id).execute(&db.pool).await?;
            }
            Ok(Some(false)) => {
                // Preserve the user's bytes and discard the stale provider identity.
                sqlx::query(
                    "UPDATE instance_content SET placement='external', object_id=NULL,
                         preferred_provider_file_id=NULL, indexed_fingerprint=NULL
                     WHERE id=? AND placement='hardlink'",
                )
                .bind(id)
                .execute(&db.pool)
                .await?;
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(content_id = id, %error, "legacy mod copy deferred"),
        }
    }
    Ok(())
}

/// Returns whether the isolated bytes still match the registered object.
fn detach(path: &Path, expected: &str) -> io::Result<Option<bool>> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => FileStamp::from_metadata(&metadata),
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut file = fs::File::open(path)?;
    if before != FileStamp::from_metadata(&file.metadata()?) {
        return Err(io::Error::other("legacy mod changed before copying"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("missing parent"))?;
    let mut copy = tempfile::NamedTempFile::new_in(parent)?;
    io::copy(&mut file, copy.as_file_mut())?;
    copy.as_file()
        .set_permissions(file.metadata()?.permissions())?;
    copy.as_file().sync_all()?;
    let matches = files::sha256_file(copy.path())?.eq_ignore_ascii_case(expected);
    drop(file);
    let current = fs::symlink_metadata(path)?;
    if !current.is_file() || before != FileStamp::from_metadata(&current) {
        return Err(io::Error::other("legacy mod changed while copying"));
    }
    copy.persist(path).map_err(|error| error.error)?;
    Ok(Some(matches))
}
