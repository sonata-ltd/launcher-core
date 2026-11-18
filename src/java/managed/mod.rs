mod bytes;
mod catalog;
mod extract;
pub(crate) mod gc;
pub mod model;
mod ownership;
pub(crate) mod process;
pub(crate) mod retention;
#[cfg(test)]
mod tests;
mod transfer;

use async_std::{fs, sync::Mutex as AsyncMutex, task};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
pub use uuid::Uuid as ManagedJavaId;
use uuid::Uuid;

use crate::java::{
    model::{JavaRuntime, JavaSource, NewJavaRuntime},
    service::JavaService,
    utils::detect_properties_retained,
};
use crate::utils::cancellation::Cancellation;
pub use model::*;

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    snapshot: ManagedJavaSnapshot,
    home: Option<PathBuf>,
    #[serde(default)]
    owner: Option<ownership::Owner>,
}

struct Entry {
    record: Record,
    cancel: Cancellation,
    owned: bool,
}

pub struct ManagedJavaService {
    root: PathBuf,
    database: Option<Arc<crate::data::db::Database>>,
    entries: Mutex<HashMap<Uuid, Entry>>,
    initialized: AsyncMutex<bool>,
    catalog: AsyncMutex<Option<ManagedJavaCatalog>>,
}

impl ManagedJavaService {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            database: None,
            entries: Mutex::new(HashMap::new()),
            initialized: AsyncMutex::new(false),
            catalog: AsyncMutex::new(None),
        }
    }

    pub(crate) fn at(root: PathBuf, database: Arc<crate::data::db::Database>) -> Self {
        Self {
            database: Some(database),
            ..Self::new(root)
        }
    }

    pub async fn catalog(&self, refresh: bool) -> Result<Vec<ManagedJavaPackage>> {
        let report = self.catalog_report(refresh).await?;
        for warning in &report.warnings {
            tracing::warn!(major = warning.major, error = %warning.error.diagnostic(), "Java catalog is incomplete");
        }
        Ok(report.packages)
    }

    /// Returns the available packages along with failures for individual Java versions.
    pub async fn catalog_report(&self, refresh: bool) -> Result<ManagedJavaCatalog> {
        let mut cache = self.catalog.lock().await;
        if !refresh {
            if let Some(report) = cache.as_ref() {
                return Ok(report.clone());
            }
        }
        let report = catalog::fetch(JavaTarget::host()?).await?;
        *cache = Some(report.clone());
        Ok(report)
    }

    pub async fn installations(&self) -> Result<Vec<ManagedJavaSnapshot>> {
        self.initialize().await?;
        let mut snapshots: Vec<_> = self
            .entries
            .lock()
            .expect("managed entries poisoned")
            .values()
            .map(|e| e.record.snapshot.clone())
            .collect();
        snapshots.sort_by_key(|s| s.created_at);
        Ok(snapshots)
    }

    pub async fn installation(&self, id: Uuid) -> Result<ManagedJavaSnapshot> {
        self.initialize().await?;
        self.entries
            .lock()
            .expect("managed entries poisoned")
            .get(&id)
            .map(|e| e.record.snapshot.clone())
            .ok_or_else(|| ManagedJavaError::new("not_found", id))
    }

    pub async fn start_install(self: &Arc<Self>, package_id: &str) -> Result<Uuid> {
        self.initialize().await?;
        let package = self
            .catalog
            .lock()
            .await
            .as_ref()
            .and_then(|report| report.packages.iter().find(|p| p.id == package_id))
            .cloned()
            .ok_or_else(|| {
                ManagedJavaError::new(
                    "not_found",
                    "Refresh the managed Java catalog before installing",
                )
            })?;
        self.start_package(package).await
    }

    async fn start_package(self: &Arc<Self>, package: ManagedJavaPackage) -> Result<Uuid> {
        let service = Arc::clone(self);
        task::spawn(async move { service.start_package_owned(package).await }).await
    }

    async fn start_package_owned(self: &Arc<Self>, package: ManagedJavaPackage) -> Result<Uuid> {
        self.initialize().await?;
        let admission = ownership::access(&self.root).await?;
        let id = Uuid::new_v4();
        let operation = ownership::operation(&self.root, id)?;
        let cancel = Cancellation::new();
        {
            let mut entries = self.entries.lock().expect("managed entries poisoned");
            if entries.values().any(|e| {
                e.record.snapshot.package.id == package.id
                    && (e.record.snapshot.stage.is_active()
                        || matches!(
                            e.record.snapshot.stage,
                            ManagedJavaStage::AwaitingValidation | ManagedJavaStage::Ready
                        ))
            }) {
                return Err(ManagedJavaError::new("busy", &package.id));
            }
            entries.insert(
                id,
                Entry {
                    record: Record {
                        snapshot: ManagedJavaSnapshot {
                            id,
                            total_bytes: Some(package.size),
                            package: package.clone(),
                            created_at: chrono::Utc::now(),
                            stage: ManagedJavaStage::Downloading,
                            downloaded_bytes: 0,
                            runtime_id: None,
                            error: None,
                        },
                        home: None,
                        owner: Some(ownership::Owner::new(id)),
                    },
                    cancel: cancel.clone(),
                    owned: true,
                },
            );
        }
        if let Err(error) = self.persist(id).await {
            self.entries
                .lock()
                .expect("managed entries poisoned")
                .remove(&id);
            return Err(error);
        }
        #[cfg(test)]
        tests::ownership_async_checkpoint(&self.root, "started").await;
        let service = Arc::clone(self);
        task::spawn(async move {
            let _admission = admission;
            let _operation = operation;
            let result = service.install(id, &package, &cancel).await;
            if let Err(error) = result {
                service.fail(id, error).await;
            }
        });
        Ok(id)
    }

    pub async fn cancel(&self, id: Uuid) -> Result<CancelDisposition> {
        self.initialize().await?;
        let disposition = {
            let mut entries = self.entries.lock().expect("managed entries poisoned");
            let entry = entries
                .get_mut(&id)
                .ok_or_else(|| ManagedJavaError::new("not_found", id))?;
            let stage = entry.record.snapshot.stage;
            if !matches!(
                stage,
                ManagedJavaStage::Downloading
                    | ManagedJavaStage::VerifyingChecksum
                    | ManagedJavaStage::Extracting
                    | ManagedJavaStage::Cancelling
            ) {
                return Ok(CancelDisposition::AlreadyFinished);
            }
            if !entry.owned {
                return Err(ManagedJavaError::new(
                    "busy",
                    "Installation belongs to another or unknown owner",
                ));
            }
            entry.cancel.cancel();
            entry.record.snapshot.stage = ManagedJavaStage::Cancelling;
            CancelDisposition::Requested
        };
        Ok(disposition)
    }

    pub async fn validate(
        self: &Arc<Self>,
        id: Uuid,
        java: &JavaService,
    ) -> Result<Arc<JavaRuntime>> {
        self.initialize().await?;
        let snapshot = self.installation(id).await?;
        if snapshot.stage == ManagedJavaStage::Ready {
            return java
                .list_all()
                .await
                .map_err(|e| ManagedJavaError::new("registration", e))?
                .into_iter()
                .find(|j| Some(j.id) == snapshot.runtime_id)
                .ok_or_else(|| {
                    ManagedJavaError::new("registration", "Registered Java is missing")
                });
        }
        let admission = ownership::access(&self.root).await?;
        let operation = ownership::operation(&self.root, id)?;
        // Another initialized service may have completed/started this operation
        // since our snapshot. Never claim a stale Failed/AwaitingValidation entry.
        if let Some(current) = read_record(&self.root, id)? {
            let mut entries = self.entries.lock().expect("managed entries poisoned");
            if let Some(entry) = entries.get_mut(&id) {
                entry.record = current;
                entry.owned = false;
            }
        }
        let current = self.installation(id).await?;
        if current.stage == ManagedJavaStage::Ready {
            return java
                .list_all()
                .await
                .map_err(|e| ManagedJavaError::new("registration", e))?
                .into_iter()
                .find(|j| Some(j.id) == current.runtime_id)
                .ok_or_else(|| {
                    ManagedJavaError::new("registration", "Registered Java is missing")
                });
        }
        let home = {
            let mut entries = self.entries.lock().expect("managed entries poisoned");
            let entry = entries
                .get_mut(&id)
                .ok_or_else(|| ManagedJavaError::new("not_found", id))?;
            if !matches!(
                entry.record.snapshot.stage,
                ManagedJavaStage::AwaitingValidation | ManagedJavaStage::Failed
            ) {
                return Err(ManagedJavaError::new(
                    "busy",
                    "Installation is not waiting for validation",
                ));
            }
            let home = entry.record.home.clone().ok_or_else(|| {
                ManagedJavaError::new("validation", "Installation has no Java home")
            })?;
            entry.record.snapshot.stage = ManagedJavaStage::Validating;
            entry.record.snapshot.error = None;
            entry.record.owner = Some(ownership::Owner::new(id));
            entry.owned = true;
            home
        };
        // Validation is owned by the core even if the caller's future is dropped.
        let service = Arc::clone(self);
        let registry = java.get_registry();
        let bus = java.managed_bus();
        let (tx, rx) = async_channel::bounded(1);
        task::spawn(async move {
            let _operation = operation;
            let java_use = process::JavaUse::admitted(admission);
            let result: Result<Arc<JavaRuntime>> = async {
                let java_use = java_use.as_ref().map_err(storage)?;
                service.persist(id).await?;
                let snapshot = service.installation(id).await?;
                let home = crate::utils::files::child(&service.root, &home).map_err(storage)?;
                let executable = crate::utils::files::child(
                    &home,
                    Path::new(if cfg!(windows) {
                        "bin/java.exe"
                    } else {
                        "bin/java"
                    }),
                )
                .map_err(storage)?;
                let detected = detect_properties_retained(executable, java_use)
                    .await
                    .map_err(|e| {
                        ManagedJavaError::new(
                            "validation",
                            format!(
                            "Executable: {}\nArguments: -XshowSettings:properties -version\n{e}",
                            extract::executable(&home).display()
                        ),
                        )
                    })?;
                validate_detected(&home, &snapshot.package, &detected)?;
                let exec = detected.exec_path.clone();
                let existing = registry
                    .list_all()
                    .await
                    .map_err(|e| ManagedJavaError::new("registration", e))?
                    .into_iter()
                    .find(|j| Path::new(&j.exec_path) == exec);
                let runtime = match existing {
                    Some(runtime) => runtime,
                    None => {
                        let mut new = NewJavaRuntime::from(detected);
                        new.source = JavaSource::Managed;
                        registry
                            .register(new)
                            .await
                            .map_err(|e| ManagedJavaError::new("registration", e))?
                    }
                };
                let mut ready_record = service
                    .entries
                    .lock()
                    .expect("managed entries poisoned")
                    .get(&id)
                    .map(|entry| entry.record.clone())
                    .ok_or_else(|| ManagedJavaError::new("not_found", id))?;
                ready_record.snapshot.stage = ManagedJavaStage::Ready;
                ready_record.snapshot.runtime_id = Some(runtime.id);
                ready_record.snapshot.error = None;
                // Ready must also be recoverable from disk when polling callers
                // observe it, including after the validation caller is dropped.
                let root = service.root.clone();
                let persisted_record = ready_record.clone();
                task::spawn_blocking(move || {
                    #[cfg(test)]
                    tests::ownership_checkpoint(&root, "validation-ready");
                    write_record(&root, &persisted_record)
                })
                .await?;
                service.update(id, |record| *record = ready_record);
                bus.publish(crate::java::model::JavaEvent::ListChanged);
                Ok(runtime)
            }
            .await;
            let result = match result {
                Ok(runtime) => Ok(runtime),
                Err(error) => Err(service.fail(id, error).await),
            };
            if let Ok(java_use) = java_use {
                java_use.finish(true);
            }
            let _ = tx.send(result).await;
        });
        rx.recv()
            .await
            .map_err(|e| ManagedJavaError::new("validation", e))?
    }

    async fn install(
        &self,
        id: Uuid,
        package: &ManagedJavaPackage,
        cancel: &Cancellation,
    ) -> Result<()> {
        let root = self.root.clone();
        let owner = self.owner(id)?;
        let staging =
            task::spawn_blocking(move || ownership::create_staging(&root, id, &owner)).await?;
        if let Some(archive) = bytes::cached(&self.root, package, cancel).await? {
            self.update(id, |record| record.snapshot.downloaded_bytes = package.size);
            self.stage(id, ManagedJavaStage::VerifyingChecksum, cancel)
                .await?;
            return self
                .install_verified_archive(id, package, cancel, archive)
                .await;
        }
        let archive = staging.join("archive.part");
        let digest = transfer::download(package, &archive, cancel, |bytes| {
            self.update(id, |record| record.snapshot.downloaded_bytes = bytes);
        })
        .await?;
        self.install_archive(id, package, cancel, archive, digest)
            .await
    }

    async fn install_archive(
        &self,
        id: Uuid,
        package: &ManagedJavaPackage,
        cancel: &Cancellation,
        archive: PathBuf,
        digest: String,
    ) -> Result<()> {
        self.stage(id, ManagedJavaStage::VerifyingChecksum, cancel)
            .await?;
        if digest != package.sha256 {
            return Err(ManagedJavaError::new(
                "checksum",
                format!(
                    "URL: {}\nExpected SHA-256: {}\nActual SHA-256: {digest}",
                    package.url, package.sha256
                ),
            ));
        }
        let archive = bytes::import(&self.root, archive, package, cancel).await?;
        self.install_verified_archive(id, package, cancel, archive)
            .await
    }

    async fn install_verified_archive(
        &self,
        id: Uuid,
        package: &ManagedJavaPackage,
        cancel: &Cancellation,
        archive: bytes::Archive,
    ) -> Result<()> {
        let staging = self.staging(id);
        self.stage(id, ManagedJavaStage::Extracting, cancel).await?;
        let destination = staging.join("contents");
        let worker_cancel = cancel.clone();
        let worker_destination = destination.clone();
        let format = package.format;
        #[cfg(test)]
        let checkpoint_root = self.root.clone();
        let home = task::spawn_blocking(move || {
            // Move the whole capability, not only its path (precise closure
            // capture must not leave admission in a cancellable caller future).
            let archive = archive;
            #[cfg(test)]
            tests::ownership_checkpoint(&checkpoint_root, "extracting");
            extract::extract(&archive.path, &worker_destination, format, &worker_cancel)
        })
        .await?;
        cancel.check()?;
        let final_relative = PathBuf::from("managed")
            .join(&package.id)
            .join(id.to_string());
        let final_path =
            crate::utils::files::child(&self.root, &final_relative).map_err(storage)?;
        let relative_home = home
            .strip_prefix(&destination)
            .map_err(|e| ManagedJavaError::new("extract", e))?;
        let database = match &self.database {
            Some(database) => database
                .retention_owner()
                .await
                .map_err(storage)?
                .map(|owner| owner.identity),
            None => None,
        };
        let owner = ownership::HomeOwner::new(id, package, relative_home.to_owned(), database)?;
        let owner_path = destination.clone();
        task::spawn_blocking(move || owner.write(&owner_path)).await?;
        #[cfg(test)]
        tests::ownership_checkpoint(&self.root, "home-owned");
        self.update(id, |record| {
            record.home = Some(final_relative.join(relative_home))
        });
        self.persist(id).await?;
        fs::create_dir_all(final_path.parent().expect("installation has parent"))
            .await
            .map_err(storage)?;
        cancel.check()?;
        if fs::symlink_metadata(&final_path).await.is_ok() {
            return Err(storage("managed destination already exists"));
        }
        fs::rename(&destination, &final_path)
            .await
            .map_err(storage)?;
        #[cfg(test)]
        tests::ownership_checkpoint(&self.root, "published");
        // The commit and cancellation request share the lock: exactly one wins.
        let cancelled = {
            let mut entries = self.entries.lock().expect("managed entries poisoned");
            let record = &mut entries.get_mut(&id).expect("installation exists").record;
            if cancel.check().is_err() {
                true
            } else {
                record.snapshot.stage = ManagedJavaStage::AwaitingValidation;
                false
            }
        };
        if cancelled {
            fs::remove_dir_all(&final_path).await.map_err(storage)?;
            return cancel.check().map_err(Into::into);
        }
        self.persist(id).await?;
        if let Err(error) = self.cleanup_staging(id).await {
            tracing::warn!(%id, error = %error.diagnostic(), "failed to clean Java staging directory");
        }
        Ok(())
    }

    async fn stage(&self, id: Uuid, stage: ManagedJavaStage, cancel: &Cancellation) -> Result<()> {
        {
            let mut entries = self.entries.lock().expect("managed entries poisoned");
            cancel.check()?;
            entries
                .get_mut(&id)
                .expect("installation exists")
                .record
                .snapshot
                .stage = stage;
        }
        self.persist(id).await
    }

    fn update(&self, id: Uuid, update: impl FnOnce(&mut Record)) {
        if let Some(entry) = self
            .entries
            .lock()
            .expect("managed entries poisoned")
            .get_mut(&id)
        {
            update(&mut entry.record);
        }
    }

    async fn fail(&self, id: Uuid, mut error: ManagedJavaError) -> ManagedJavaError {
        if let Err(cleanup) = self.cleanup_staging(id).await {
            error
                .details
                .push_str(&format!("\nStaging cleanup deferred: {}", cleanup.details));
        }
        self.update(id, |record| {
            error.stage = Some(record.snapshot.stage);
            record.snapshot.stage = if error.code == "cancelled" {
                ManagedJavaStage::Cancelled
            } else {
                ManagedJavaStage::Failed
            };
            record.snapshot.error = Some(error.clone());
        });
        if let Err(persist_error) = self.persist(id).await {
            error.details.push_str(&format!(
                "\nSaving failure details failed: {}",
                persist_error.details
            ));
            self.update(id, |record| record.snapshot.error = Some(error.clone()));
        }
        error
    }

    fn staging(&self, id: Uuid) -> PathBuf {
        self.root.join(".managed/staging").join(id.to_string())
    }

    fn owner(&self, id: Uuid) -> Result<ownership::Owner> {
        self.entries
            .lock()
            .expect("managed entries poisoned")
            .get(&id)
            .filter(|entry| entry.owned)
            .and_then(|entry| entry.record.owner.clone())
            .filter(|owner| owner.local(id))
            .ok_or_else(|| storage("managed operation has no local owner"))
    }

    async fn cleanup_staging(&self, id: Uuid) -> Result<()> {
        let owner = self.owner(id)?;
        let root = self.root.clone();
        task::spawn_blocking(move || ownership::cleanup_staging(&root, id, &owner, false)).await
    }

    async fn persist(&self, id: Uuid) -> Result<()> {
        let record = self
            .entries
            .lock()
            .expect("managed entries poisoned")
            .get(&id)
            .map(|e| e.record.clone())
            .ok_or_else(|| ManagedJavaError::new("not_found", id))?;
        let root = self.root.clone();
        task::spawn_blocking(move || write_record(&root, &record)).await
    }

    async fn initialize(&self) -> Result<()> {
        let mut initialized = self.initialized.lock().await;
        if *initialized {
            return Ok(());
        }
        retention::attach(self.database.clone(), self.root.clone()).await?;
        let root = self.root.clone();
        let records = task::spawn_blocking(move || -> Result<Vec<Record>> {
            let _maintenance = ownership::maintenance(&root)?;
            #[cfg(test)]
            tests::ownership_checkpoint(&root, "recovery");
            let directory = crate::utils::files::child(&root, Path::new(".managed/operations"))
                .map_err(storage)?;
            std::fs::create_dir_all(&directory).map_err(storage)?;
            let archive_root = crate::utils::files::child(&root, Path::new(".managed/bytes")).map_err(storage)?;
            crate::utils::object_store::ObjectStore::new(archive_root).sweep_staging().map_err(storage)?;
            let mut records = Vec::new();
            for entry in std::fs::read_dir(&directory).map_err(storage)? {
                let path = entry.map_err(storage)?.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let mut record: Record = serde_json::from_slice(
                    &crate::utils::object_store::catalog::read_regular(&path, 1024 * 1024)
                        .map_err(storage)?,
                )
                .map_err(storage)?;
                if path.file_name().and_then(|name| name.to_str())
                    != Some(&format!("{}.json", record.snapshot.id))
                    || record.home.as_ref().is_some_and(|home| {
                        home.to_string_lossy().contains('\\')
                            || crate::utils::files::child(&root, home).is_err()
                    })
                {
                    return Err(ManagedJavaError::new(
                        "storage",
                        format!("Invalid Java home in {}", path.display()),
                    ));
                }
                // A terminal record can leave crash scratch, but never grants
                // permission to delete its published home.
                if let Some(owner) = record.owner.as_ref().filter(|owner| owner.dead(record.snapshot.id)) {
                    if let Err(error) = ownership::cleanup_staging(&root, record.snapshot.id, owner, true) {
                        tracing::warn!(id = %record.snapshot.id, error = %error.diagnostic(), "managed recovery deferred for unknown staging ownership");
                        records.push(record);
                        continue;
                    }
                }
                if record.snapshot.stage.is_active() {
                    if !record.owner.as_ref().is_some_and(|owner| owner.dead(record.snapshot.id)) {
                        // Old writers did not publish operation ownership. Do not
                        // overwrite their active record or touch private bytes.
                        records.push(record);
                        continue;
                    }
                    let installed = record
                        .home
                        .as_ref()
                        .and_then(|h| crate::utils::files::child(&root, &h.join(if cfg!(windows) { "bin/java.exe" } else { "bin/java" })).ok())
                        .is_some_and(|exec| exec.is_file());
                    record.snapshot.stage = if installed {
                        ManagedJavaStage::AwaitingValidation
                    } else {
                        ManagedJavaStage::Failed
                    };
                    record.snapshot.error = if installed {
                        None
                    } else {
                        Some(ManagedJavaError::new(
                            "interrupted",
                            "The launcher stopped before installation completed",
                        ))
                    };
                    write_record(&root, &record)?;
                }
                records.push(record);
            }
            Ok(records)
        })
        .await?;
        let mut entries = self.entries.lock().expect("managed entries poisoned");
        for record in records {
            entries.insert(
                record.snapshot.id,
                Entry {
                    record,
                    cancel: Cancellation::new(),
                    owned: false,
                },
            );
        }
        *initialized = true;
        Ok(())
    }
}

fn storage(error: impl std::fmt::Display) -> ManagedJavaError {
    ManagedJavaError::new("storage", error)
}

fn read_record(root: &Path, id: Uuid) -> Result<Option<Record>> {
    let path = crate::utils::files::child(
        root,
        &PathBuf::from(".managed/operations").join(format!("{id}.json")),
    )
    .map_err(storage)?;
    let bytes = match crate::utils::object_store::catalog::read_regular(&path, 1024 * 1024) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(storage(error)),
    };
    let record: Record = serde_json::from_slice(&bytes).map_err(storage)?;
    if record.snapshot.id != id
        || record.home.as_ref().is_some_and(|home| {
            home.to_string_lossy().contains('\\') || crate::utils::files::child(root, home).is_err()
        })
    {
        return Err(storage("invalid managed operation identity or home"));
    }
    Ok(Some(record))
}

fn write_record(root: &Path, record: &Record) -> Result<()> {
    let directory =
        crate::utils::files::child(root, Path::new(".managed/operations")).map_err(storage)?;
    std::fs::create_dir_all(&directory).map_err(storage)?;
    let mut file = tempfile::NamedTempFile::new_in(&directory).map_err(storage)?;
    serde_json::to_writer_pretty(&mut file, record).map_err(storage)?;
    file.as_file().sync_all().map_err(storage)?;
    file.persist(directory.join(format!("{}.json", record.snapshot.id)))
        .map_err(storage)?;
    Ok(())
}

fn validate_detected(
    home: &Path,
    package: &ManagedJavaPackage,
    detected: &crate::java::utils::DetectedJavaRuntime,
) -> Result<()> {
    let actual_home = detected
        .home_path
        .as_ref()
        .ok_or_else(|| ManagedJavaError::new("validation", "Java did not report java.home"))?;
    let home = dunce::canonicalize(home).map_err(|e| ManagedJavaError::new("validation", e))?;
    if !actual_home.starts_with(&home) || !detected.exec_path.starts_with(&home) {
        return Err(ManagedJavaError::new(
            "validation",
            format!(
                "Java home is outside installation: {}",
                actual_home.display()
            ),
        ));
    }
    let expected_arch = if package.target.arch == "x64" {
        "x86_64"
    } else if package.target.arch == "x32" {
        "x86"
    } else {
        &package.target.arch
    };
    // Catalog versions can omit build components; SHA-256 pins the downloaded archive.
    if detected.major != Some(package.major)
        || detected
            .version
            .as_deref()
            .and_then(crate::java::version::parse_major)
            != Some(package.major)
        || detected
            .arch
            .as_deref()
            .map(crate::java::select::normalize_arch)
            != Some(expected_arch)
    {
        return Err(ManagedJavaError::new(
            "validation",
            format!(
                "Expected Java {} ({expected_arch}); detected {detected:?}",
                package.major
            ),
        ));
    }
    Ok(())
}
