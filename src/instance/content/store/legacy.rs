//! Detach files published by the old hardlink installer.
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    data::{db::Database, layout::LauncherPaths},
    instance::{
        activity::InstanceActivity,
        content::{error::Result, inventory, journal::Placement, model::ContentKind},
        model::InstanceId,
    },
    utils::files::{self, digest::FileStamp},
    utils::{
        cancellation::Cancellation,
        jobs::supervisor::Tasks,
        object_store::{ObjectStore, StoreLock},
    },
};

#[derive(sqlx::FromRow)]
struct LegacyPlacement {
    id: i64,
    object_id: i64,
    instance_id: InstanceId,
    dir: String,
    relative_path: String,
    path_key: Vec<u8>,
    sha256: String,
    kind: String,
}

pub(in crate::instance::content) async fn detach_registered_links(
    db: Arc<Database>,
    paths: Arc<LauncherPaths>,
    activity: Arc<InstanceActivity>,
    store: Arc<ObjectStore>,
    ownership: [Arc<StoreLock>; 3],
    tasks: &Tasks,
) -> Result<()> {
    tasks
        .spawn("legacy hardlink migration", None, async move {
            // Already admitted migration owns lifetime locks, CAS access, blocking
            // copies and SQL completion even if its bootstrap caller disappears.
            let _ownership = ownership;
            let access = store.access(&Cancellation::new()).await?;
            let result = migrate(&db, &paths, &activity, &access).await;
            if let Err(error) = &result {
                tracing::warn!(%error, "legacy hardlink migration failed");
            }
            result
        })?
        .await?
}

async fn migrate(
    db: &Database,
    paths: &LauncherPaths,
    activity: &InstanceActivity,
    store: &crate::utils::object_store::StoreAccess,
) -> Result<()> {
    let placements: Vec<LegacyPlacement> = sqlx::query_as(
        "SELECT content.id, content.object_id, content.instance_id, instance.dir,
                content.relative_path, content.path_key, object.sha256, content.kind
         FROM instance_content AS content
         JOIN instances AS instance ON instance.id=content.instance_id
         JOIN content_objects AS object ON object.id=content.object_id
         WHERE content.placement=?",
    )
    .bind(Placement::Hardlink)
    .fetch_all(&db.pool)
    .await?;

    for placement in placements {
        let _activity = match activity.reserve_content_change(placement.instance_id) {
            Ok(guard) => guard,
            Err(error) => {
                tracing::warn!(instance = %placement.instance_id, %error, "legacy mod copy deferred");
                continue;
            }
        };
        let id = placement.id;
        let instance = match files::child(paths.instances(), Path::new(&placement.dir)) {
            Ok(instance) => instance,
            Err(error) => {
                tracing::warn!(content_id = id, %error, "legacy content copy deferred");
                continue;
            }
        };
        let root = instance.clone();
        let recorded = PathBuf::from(&placement.relative_path);
        let expected = placement.sha256.clone();
        let kind = match placement.kind.as_str() {
            "mod" => ContentKind::Mod,
            "resource_pack" => ContentKind::ResourcePack,
            "shader_pack" => ContentKind::ShaderPack,
            _ => {
                tracing::warn!(content_id = id, kind = %placement.kind, "unknown legacy content kind");
                continue;
            }
        };
        let access = store.clone();
        let plan = async_std::task::spawn_blocking(move || {
            find_placement(&root, &recorded, &expected, kind, &access)
        })
        .await;
        let plan = match plan {
            Ok(Some(plan)) => plan,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(content_id = id, %error, "legacy mod copy deferred");
                continue;
            }
        };
        let Some(relative) = plan.relative_path.to_str().map(str::to_owned) else {
            continue; // SQL cannot represent this name; do not invent a lossy path.
        };
        let key = files::path_key(&plan.relative_path);
        let relocated = plan.relative_path != Path::new(&placement.relative_path);
        if relocated {
            // One inode alias is still ambiguous if several historical bindings
            // could claim it. Do not assign it to the first row returned by SQL.
            let claims: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM instance_content
                 WHERE instance_id=? AND object_id=? AND kind=? AND placement=?",
            )
            .bind(placement.instance_id)
            .bind(placement.object_id)
            .bind(&placement.kind)
            .bind(Placement::Hardlink)
            .fetch_one(&db.pool)
            .await?;
            if claims != 1 {
                continue;
            }
        }
        let occupied: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM instance_content WHERE instance_id=? AND path_key=? AND id<>?)",
        ).bind(placement.instance_id).bind(&key).bind(id).fetch_one(&db.pool).await?;
        if occupied {
            continue; // Physical identity does not allow merging two bindings.
        }
        #[cfg(test)]
        crate::instance::content::install::checkpoints::wait(
            checkpoint(paths.instances(), &placement.dir),
            "legacy-identified",
        )
        .await;
        let enabled = plan.mod_enabled;
        let expected = placement.sha256.clone();
        let access = store.clone();
        let outcome = async_std::task::spawn_blocking(move || {
            let _access = access;
            let path = files::child(&instance, &plan.relative_path)?;
            Ok::<_, crate::utils::files::FileError>(detach(&path, &expected, &plan.stamp)?)
        })
        .await;
        match outcome {
            Ok(Some(matches)) => {
                #[cfg(test)]
                crate::instance::content::install::checkpoints::wait(
                    checkpoint(paths.instances(), &placement.dir),
                    "legacy-sql",
                )
                .await;
                // File-preserving isolation precedes SQL. A failed SQL update
                // leaves independent bytes, never a replacement of the CAS.
                let updated = sqlx::query(
                    "UPDATE instance_content SET relative_path=?, path_key=?,
                         mod_enabled=COALESCE(?,mod_enabled), presence='present', placement=?,
                         object_id=CASE WHEN ? THEN object_id ELSE NULL END,
                         preferred_provider_file_id=CASE WHEN ? THEN preferred_provider_file_id ELSE NULL END,
                         indexed_fingerprint=NULL, observed_size=NULL, observed_modified=NULL, observed_identity=NULL
                     WHERE id=? AND instance_id=? AND placement=? AND object_id=? AND relative_path=? AND path_key=? AND kind=?
                       AND NOT EXISTS(SELECT 1 FROM instance_content WHERE instance_id=? AND path_key=? AND id<>?)
                       AND (?=0 OR NOT EXISTS(SELECT 1 FROM instance_content
                           WHERE instance_id=? AND object_id=? AND kind=? AND placement=? AND id<>?))",
                )
                .bind(relative).bind(&key).bind(enabled)
                .bind(if matches { Placement::Copy } else { Placement::External })
                .bind(matches).bind(matches).bind(id).bind(placement.instance_id)
                .bind(Placement::Hardlink).bind(placement.object_id).bind(&placement.relative_path)
                .bind(&placement.path_key).bind(&placement.kind)
                .bind(placement.instance_id).bind(&key).bind(id)
                .bind(relocated).bind(placement.instance_id).bind(placement.object_id)
                .bind(&placement.kind).bind(Placement::Hardlink).bind(id)
                .execute(&db.pool)
                .await?;
                if updated.rows_affected() != 1 {
                    tracing::warn!(
                        content_id = id,
                        "legacy binding changed before SQL completion; reconciliation deferred"
                    );
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(content_id = id, %error, "legacy mod copy deferred"),
        }
    }
    Ok(())
}

struct MigrationPlan {
    relative_path: PathBuf,
    stamp: FileStamp,
    mod_enabled: Option<bool>,
}

fn find_placement(
    instance: &Path,
    recorded: &Path,
    expected: &str,
    kind: ContentKind,
    store: &ObjectStore,
) -> Result<Option<MigrationPlan>> {
    let path = files::child(instance, recorded)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {
            return Ok(Some(MigrationPlan {
                relative_path: recorded.to_owned(),
                stamp: FileStamp::from_metadata(&metadata),
                mod_enabled: None,
            }))
        }
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let object = store.path_for(expected)?;
    let object = files::child(store.root(), object.strip_prefix(store.root()).unwrap())?;
    match fs::symlink_metadata(&object) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let mut found = None;
    for entry in inventory::inventory_scope(instance, kind)?.entries {
        if let inventory::ObservedType::Archive(candidate) = entry.entry_type {
            if same_physical_file(&object, &files::child(instance, &entry.relative_path)?) {
                if found.is_some() {
                    return Ok(None); // Multiple aliases do not identify a unique binding.
                }
                found = Some(MigrationPlan {
                    relative_path: entry.relative_path,
                    stamp: candidate,
                    mod_enabled: entry.mod_enabled,
                });
            }
        }
    }
    Ok(found)
}

pub(in crate::instance::content) fn same_physical_file(left: &Path, right: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let left = fs::symlink_metadata(left);
        let right = fs::symlink_metadata(right);
        match (left, right) {
            (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        match (file_id(left), file_id(right)) {
            (Some(left), Some(right)) => left == right,
            _ => false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left, right);
        false
    } // Size/timestamps are not physical identity.
}

/// The (volume, 128-bit file ID) pair Windows treats as physical identity.
/// Query failures return `None`: an unknown identity must fail closed, and the
/// handle mirrors `symlink_metadata` semantics by not following reparse points.
#[cfg(windows)]
fn file_id(path: &Path) -> Option<(u64, [u8; 16])> {
    use std::{os::windows::fs::OpenOptionsExt, os::windows::io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FileIdInfo, GetFileInformationByHandleEx, FILE_FLAG_OPEN_REPARSE_POINT, FILE_ID_INFO,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    let mut options = std::fs::OpenOptions::new();
    options
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    let file = options.open(path).ok()?;
    let mut info: FILE_ID_INFO = Default::default();
    let queried = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            &mut info as *mut _ as *mut core::ffi::c_void,
            size_of::<FILE_ID_INFO>() as u32,
        )
    };
    (queried != 0).then(|| (info.VolumeSerialNumber, info.FileId.Identifier))
}

#[cfg(test)]
pub(in crate::instance::content) fn checkpoint(instances: &Path, dir: &str) -> uuid::Uuid {
    uuid::Uuid::new_v5(
        &uuid::Uuid::NAMESPACE_OID,
        &files::path_key(&instances.join(dir)),
    )
}

/// Returns whether the isolated bytes still match the registered object.
fn detach(path: &Path, expected: &str, planned: &FileStamp) -> io::Result<Option<bool>> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => FileStamp::from_metadata(&metadata),
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if before != *planned {
        return Err(io::Error::other(
            "legacy mod changed after identity verification",
        ));
    }
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
