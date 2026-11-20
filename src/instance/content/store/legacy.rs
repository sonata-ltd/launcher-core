//! Detach files published by the old hardlink installer.
use std::{
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use super::super::storage;
use crate::{
    data::{db::Database, layout::LauncherPaths, ownership::LayoutLease},
    instance::{
        activity::InstanceActivity,
        content::{error::Result, inventory, journal::Placement, model::ContentKind},
        model::InstanceId,
    },
    utils::files::{self, digest::FileStamp, directory::Directory, identity::FileIdentity},
    utils::{cancellation::Cancellation, jobs::supervisor::Tasks, object_store::ObjectStore},
};

#[derive(sqlx::FromRow, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
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

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DetachIntent {
    original: LegacyPlacement,
    destination: String,
    witness: uuid::Uuid,
    file_identity: String,
    sha256: String,
    enabled: Option<bool>,
}

impl DetachIntent {
    fn witness_name(&self) -> String {
        format!(".sonata-detach-{}.pending", self.witness)
    }
}

pub(in crate::instance::content) async fn detach_registered_links(
    db: Arc<Database>,
    paths: Arc<LauncherPaths>,
    activity: Arc<InstanceActivity>,
    store: Arc<ObjectStore>,
    ownership: Arc<LayoutLease>,
    tasks: &Tasks,
) -> Result<()> {
    tasks
        .spawn("legacy hardlink migration", None, async move {
            // Already admitted migration owns lifetime locks, CAS access, blocking
            // copies and SQL completion even if its bootstrap caller disappears.
            let access = store.access(&Cancellation::new()).await?;
            let result = migrate(&db, &paths, &activity, &access, &ownership).await;
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
    ownership: &LayoutLease,
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
        let instance_directory = match ownership.open_instance_directory(&instance) {
            Ok(directory) => directory,
            Err(error) => {
                tracing::warn!(content_id = id, %error, "legacy instance directory unavailable");
                continue;
            }
        };
        let instance = instance_directory.path().to_owned();
        if let Some(body) = sqlx::query_scalar::<_, String>(
            "SELECT intent_json FROM legacy_detach_intents WHERE binding_id=?",
        )
        .bind(id)
        .fetch_optional(&db.pool)
        .await?
        {
            let intent: DetachIntent = serde_json::from_str(&body)?;
            if intent.original != placement {
                tracing::warn!(
                    content_id = id,
                    "legacy binding changed; previous migration deferred"
                );
                continue;
            }
            let recovered = files::child(&instance, Path::new(&intent.destination))?;
            if let Some(witness) = publication_witness(&recovered, &intent)? {
                finish_detach(db, &instance, &intent, &witness).await?;
                continue;
            }
            // A fresh migration may proceed only if selection below can still
            // independently prove an original hardlink. Unknown bytes stay put.
        }
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
        if let Err(error) = storage::guard_binding_aliases(
            db,
            placement.instance_id,
            &instance,
            &plan.relative_path,
            Some(id),
        )
        .await
        {
            tracing::warn!(content_id = id, %error, "legacy binding deferred");
            continue;
        }
        #[cfg(test)]
        crate::instance::content::install::checkpoints::wait(
            checkpoint(paths.instances(), &placement.dir),
            "legacy-identified",
        )
        .await;
        let enabled = plan.mod_enabled;
        let access = store.clone();
        let target = files::child(&instance, &plan.relative_path)?;
        let Some(relative_parent) = plan.relative_path.parent() else {
            continue;
        };
        let directory = instance_directory.open_relative(relative_parent)?;
        let destination = target.clone();
        let outcome = async_std::task::spawn_blocking(move || {
            let _access = access;
            stage_detach(&destination, plan, directory)
        })
        .await;
        match outcome {
            Ok(Some(staged)) => {
                let intent = DetachIntent {
                    original: placement,
                    destination: relative,
                    witness: uuid::Uuid::new_v4(),
                    file_identity: FileIdentity::from_handle(&staged.file)?.storage_key(),
                    sha256: staged.sha256.clone(),
                    enabled,
                };
                let source = OsStr::new(&staged.name);
                staged.verify()?;
                staged
                    .directory
                    .hard_link(source, std::ffi::OsStr::new(&intent.witness_name()))?;
                sqlx::query(
                    "INSERT INTO legacy_detach_intents(binding_id,intent_json) VALUES(?,?)
                     ON CONFLICT(binding_id) DO UPDATE SET intent_json=excluded.intent_json",
                )
                .bind(id)
                .bind(serde_json::to_string(&intent)?)
                .execute(&db.pool)
                .await?;
                let published =
                    async_std::task::spawn_blocking(move || staged.publish(&target)).await;
                if let Err(error) = published {
                    tracing::warn!(content_id = id, %error, "legacy mod publication deferred");
                    continue;
                }
                #[cfg(test)]
                crate::instance::content::install::checkpoints::wait(
                    checkpoint(paths.instances(), &intent.original.dir),
                    "legacy-sql",
                )
                .await;
                if let Some(witness) = publication_witness(
                    &files::child(&instance, Path::new(&intent.destination))?,
                    &intent,
                )? {
                    finish_detach(db, &instance, &intent, &witness).await?;
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(content_id = id, %error, "legacy mod copy deferred"),
        }
    }
    Ok(())
}

fn publication_witness(target: &Path, intent: &DetachIntent) -> io::Result<Option<PathBuf>> {
    let witness = target.with_file_name(intent.witness_name());
    for path in [target, witness.as_path()] {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        }
    }
    let target_identity = FileIdentity::at(target)?;
    if target_identity != FileIdentity::at(&witness)?
        || target_identity.storage_key() != intent.file_identity
        || files::sha256_file(target)? != intent.sha256
    {
        return Ok(None);
    }
    Ok(Some(witness))
}

async fn finish_detach(
    db: &Database,
    root: &Path,
    intent: &DetachIntent,
    witness: &Path,
) -> Result<()> {
    let placement = &intent.original;
    let id = placement.id;
    let destination = Path::new(&intent.destination);
    if let Err(error) =
        storage::guard_binding_aliases(db, placement.instance_id, root, destination, Some(id)).await
    {
        tracing::warn!(content_id = id, %error, "legacy completion deferred");
        return Ok(());
    }
    let relocated = destination != Path::new(&placement.relative_path);
    let matches = intent.sha256.eq_ignore_ascii_case(&placement.sha256);
    let mut transaction = db.pool.begin().await?;
    let rows = storage::binding_paths(&mut transaction, placement.instance_id).await?;
    let key = match storage::existing_binding_key(&rows, destination, Some(id)) {
        Ok(key) => key,
        Err(crate::instance::content::error::ContentError::AmbiguousBinding { .. }) => {
            return Ok(())
        }
        Err(error) => return Err(error),
    };
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
                .bind(&intent.destination).bind(&key).bind(intent.enabled)
                .bind(if matches { Placement::Copy } else { Placement::External })
                .bind(matches).bind(matches).bind(id).bind(placement.instance_id)
                .bind(Placement::Hardlink).bind(placement.object_id).bind(&placement.relative_path)
                .bind(&placement.path_key).bind(&placement.kind)
                .bind(placement.instance_id).bind(&key).bind(id)
                .bind(relocated).bind(placement.instance_id).bind(placement.object_id)
                .bind(&placement.kind).bind(Placement::Hardlink).bind(id)
                .execute(&mut *transaction)
                .await?;
    if updated.rows_affected() != 1 {
        tracing::warn!(
            content_id = id,
            "legacy binding changed before SQL completion; reconciliation deferred"
        );
        return Ok(());
    }
    sqlx::query("DELETE FROM legacy_detach_intents WHERE binding_id=?")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    if let Some(target) = witness
        .parent()
        .and_then(|p| destination.file_name().map(|name| p.join(name)))
    {
        if publication_witness(&target, intent)
            .ok()
            .flatten()
            .is_some()
        {
            let cleanup = Directory::open(
                witness
                    .parent()
                    .ok_or_else(|| io::Error::other("missing witness parent"))?,
            )
            .and_then(|directory| directory.unlink(std::ffi::OsStr::new(&intent.witness_name())));
            if let Err(error) = cleanup {
                tracing::warn!(content_id = id, %error, "could not remove legacy migration witness");
            }
        }
    }
    Ok(())
}

struct MigrationPlan {
    relative_path: PathBuf,
    stamp: FileStamp,
    mod_enabled: Option<bool>,
    object: PathBuf,
    identity: FileIdentity,
    _object_handle: fs::File,
}

fn find_placement(
    instance: &Path,
    recorded: &Path,
    expected: &str,
    kind: ContentKind,
    store: &ObjectStore,
) -> Result<Option<MigrationPlan>> {
    let path = files::child(instance, recorded)?;
    let recorded_stamp = match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => Some(FileStamp::from_metadata(&metadata)),
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let object = store.path_for(expected)?;
    let object = files::child(store.root(), object.strip_prefix(store.root()).unwrap())?;
    match fs::symlink_metadata(&object) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let object_handle = fs::File::open(&object)?;
    let identity = FileIdentity::from_handle(&object_handle)?;
    if FileIdentity::at(&object)? != identity {
        return Ok(None);
    }
    if let Some(stamp) = recorded_stamp {
        if FileIdentity::at(&path)? != identity {
            return Ok(None);
        }
        return Ok(Some(MigrationPlan {
            relative_path: recorded.to_owned(),
            stamp,
            mod_enabled: None,
            object,
            identity,
            _object_handle: object_handle,
        }));
    }
    let candidates = inventory::inventory_scope(instance, kind)?.entries;
    #[cfg(test)]
    async_std::task::block_on(crate::instance::content::install::checkpoints::wait(
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, &files::path_key(instance)),
        "legacy-before-identity",
    ));
    let mut found = None;
    for entry in candidates {
        if let inventory::ObservedType::Archive(candidate) = entry.entry_type {
            // Treat an unobservable candidate as unknown, including a file
            // that went away after scanning the scope. It cannot prove uniqueness.
            let candidate_path = files::child(instance, &entry.relative_path)?;
            if FileIdentity::at(&candidate_path)? == identity {
                if found.is_some() {
                    return Ok(None); // Multiple aliases do not identify a unique binding.
                }
                found = Some((entry.relative_path, candidate, entry.mod_enabled));
            }
        }
    }
    Ok(
        found.map(|(relative_path, stamp, mod_enabled)| MigrationPlan {
            relative_path,
            stamp,
            mod_enabled,
            object,
            identity,
            _object_handle: object_handle,
        }),
    )
}

#[cfg(test)]
pub(in crate::instance::content) fn same_physical_file(left: &Path, right: &Path) -> bool {
    matches!((FileIdentity::at(left), FileIdentity::at(right)), (Ok(left), Ok(right)) if left == right)
}

#[cfg(test)]
pub(in crate::instance::content) fn checkpoint(instances: &Path, dir: &str) -> uuid::Uuid {
    let path = instances.join(dir);
    let path = path.canonicalize().unwrap_or(path);
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, &files::path_key(&path))
}

/// Returns whether the isolated bytes still match the registered object.
struct StagedDetach {
    plan: MigrationPlan,
    file: fs::File,
    name: String,
    sha256: String,
    directory: Arc<Directory>,
}

impl StagedDetach {
    fn verify(&self) -> io::Result<()> {
        self.directory.verify_file(
            OsStr::new(&self.name),
            FileIdentity::from_handle(&self.file)?,
        )
    }

    fn publish(self, path: &Path) -> io::Result<()> {
        self.verify()?;
        let current = fs::symlink_metadata(path)?;
        if !current.is_file()
            || self.plan.stamp != FileStamp::from_metadata(&current)
            || FileIdentity::at(path)? != self.plan.identity
            || FileIdentity::at(&self.plan.object)? != self.plan.identity
        {
            return Err(io::Error::other("legacy mod changed while copying"));
        }
        let source = OsStr::new(&self.name);
        let destination = path
            .file_name()
            .ok_or_else(|| io::Error::other("missing legacy file name"))?;
        self.directory.replace(source, destination)
    }
}

impl Drop for StagedDetach {
    fn drop(&mut self) {
        if self.verify().is_ok() {
            let _ = self.directory.unlink(OsStr::new(&self.name));
        }
    }
}

fn stage_detach(
    path: &Path,
    planned: MigrationPlan,
    directory: Arc<Directory>,
) -> io::Result<Option<StagedDetach>> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => FileStamp::from_metadata(&metadata),
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if before != planned.stamp || FileIdentity::at(&planned.object)? != planned.identity {
        return Err(io::Error::other(
            "legacy mod changed after identity verification",
        ));
    }
    let name = path.file_name().ok_or(io::ErrorKind::InvalidInput)?;
    let mut file = directory.open_file(name)?;
    if before != FileStamp::from_metadata(&file.metadata()?)
        || FileIdentity::from_handle(&file)? != planned.identity
    {
        return Err(io::Error::other("legacy mod changed before copying"));
    }
    let name = format!(".sonata-detach-{}", uuid::Uuid::new_v4());
    let mut staged = StagedDetach {
        plan: planned,
        file: directory.create_private_file(OsStr::new(&name))?,
        name,
        sha256: String::new(),
        directory,
    };
    io::copy(&mut file, &mut staged.file)?;
    staged
        .file
        .set_permissions(file.metadata()?.permissions())?;
    staged.file.sync_all()?;
    let name = OsStr::new(&staged.name);
    let copy = staged.directory.open_file(name)?;
    staged
        .directory
        .verify_file(name, FileIdentity::from_handle(&staged.file)?)?;
    if FileIdentity::from_handle(&copy)? != FileIdentity::from_handle(&staged.file)? {
        return Err(io::Error::other("staged legacy file changed"));
    }
    staged.sha256 = files::sha256_open_file(&copy)?;
    Ok(Some(staged))
}
