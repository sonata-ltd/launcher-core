mod bytes;
mod catalog;
mod extract;
pub(crate) mod gc;
pub mod model;
mod ownership;
pub(crate) mod process;
mod recovery;
pub mod registrar;
pub(crate) mod retention;
#[cfg(test)]
mod tests;
mod transfer;

use async_std::{fs, sync::Mutex as AsyncMutex, task};
use futures::FutureExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
pub use uuid::Uuid as ManagedJavaId;
use uuid::Uuid;

use crate::java::{model::JavaRuntime, utils::detect_properties_retained};
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
    job_id: Option<crate::data::registry::operation::JobId>,
}

pub struct ManagedJavaService {
    operations: Arc<crate::data::registry::operation::OperationRegistry>,
    tasks: crate::utils::jobs::supervisor::Tasks,
    root: PathBuf,
    database: Option<Arc<crate::data::db::Database>>,
    entries: Mutex<HashMap<Uuid, Entry>>,
    recovery_diagnostics: Mutex<Vec<ManagedRecoveryDiagnostic>>,
    initialized: AsyncMutex<bool>,
    catalog: AsyncMutex<Option<ManagedJavaCatalog>>,
}

impl ManagedJavaService {
    pub(crate) fn new(root: PathBuf) -> Self {
        let operations = Arc::new(crate::data::registry::operation::OperationRegistry::new(
            crate::bus::EventBus::new(16),
        ));
        Self {
            tasks: operations.tasks.clone(),
            operations,
            root,
            database: None,
            entries: Mutex::new(HashMap::new()),
            recovery_diagnostics: Mutex::new(Vec::new()),
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

    pub(crate) fn with_operations(
        mut self,
        operations: Arc<crate::data::registry::operation::OperationRegistry>,
    ) -> Self {
        self.tasks = operations.tasks.clone();
        self.operations = operations;
        self
    }

    /// Current-session correlation only; recovered records have no session job.
    pub async fn job_id(
        &self,
        id: Uuid,
    ) -> Result<Option<crate::data::registry::operation::JobId>> {
        self.initialize().await?;
        self.entries
            .lock()
            .expect("managed entries poisoned")
            .get(&id)
            .map(|entry| entry.job_id)
            .ok_or_else(|| ManagedJavaError::new("not_found", id))
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
        Ok(self.installations_report().await?.installations)
    }

    /// Queries healthy records alongside record-local recovery diagnostics.
    /// Bad records remain on disk and continue to block conservative GC.
    pub async fn installations_report(&self) -> Result<ManagedInstallationsReport> {
        self.initialize().await?;
        let mut snapshots: Vec<_> = self
            .entries
            .lock()
            .expect("managed entries poisoned")
            .values()
            .map(|e| e.record.snapshot.clone())
            .collect();
        snapshots.sort_by_key(|s| s.created_at);
        Ok(ManagedInstallationsReport {
            installations: snapshots,
            diagnostics: self
                .recovery_diagnostics
                .lock()
                .expect("managed diagnostics poisoned")
                .clone(),
        })
    }

    pub async fn installation(&self, id: Uuid) -> Result<ManagedJavaSnapshot> {
        self.initialize().await?;
        if let Some(diagnostic) = self
            .recovery_diagnostics
            .lock()
            .expect("managed diagnostics poisoned")
            .iter()
            .find(|diagnostic| diagnostic.operation == Some(id))
        {
            // A cleanup diagnostic may coexist with a readable record. Only a
            // record excluded from entries should fail the specific query.
            if !self
                .entries
                .lock()
                .expect("managed entries poisoned")
                .contains_key(&id)
            {
                return Err(diagnostic.error.clone());
            }
        }
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
        let cancel = Cancellation::new();
        let ticket = self
            .tasks
            .admit("managed Java installation", Some(cancel.clone()))
            .map_err(storage)?;
        task::spawn(async move { service.start_package_owned(package, cancel, ticket).await }).await
    }

    async fn start_package_owned(
        self: &Arc<Self>,
        package: ManagedJavaPackage,
        cancel: Cancellation,
        ticket: crate::utils::jobs::supervisor::Ticket,
    ) -> Result<Uuid> {
        self.initialize().await?;
        let admission = ownership::access(&self.root).await?;
        let id = Uuid::new_v4();
        let job = self
            .operations
            .begin_for(
                format!("java.managed.install:{id}"),
                vec![crate::data::registry::operation::message::stage::OperationStage::InstallJava],
                crate::operations::OperationSubject::ManagedInstall { installation: id },
            )
            .owned_worker();
        let operation = ownership::operation(&self.root, id)?;
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
                    job_id: Some(job.id()),
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
        ticket.spawn(async move {
            let _admission = admission;
            let _operation = operation;
            let result = std::panic::AssertUnwindSafe(service.install(id, &package, &cancel))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    Err(ManagedJavaError::new(
                        "worker_panic",
                        "Managed Java installation panicked",
                    ))
                });
            if let Err(error) = result {
                let error = service.fail(id, error).await;
                job.finish(if error.code == ManagedJavaErrorKind::Cancelled {
                    crate::data::registry::operation::message::status::Outcome::Cancelled
                } else {
                    crate::data::registry::operation::message::status::Outcome::Failed
                });
            } else {
                job.finish(crate::data::registry::operation::message::status::Outcome::Completed);
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

    pub async fn validate_with(
        self: &Arc<Self>,
        id: Uuid,
        registrar: Arc<dyn registrar::RuntimeRegistrar>,
    ) -> Result<Arc<JavaRuntime>> {
        let ticket = self
            .tasks
            .admit("managed Java validation", None)
            .map_err(storage)?;
        self.initialize().await?;
        let snapshot = self.installation(id).await?;
        if snapshot.stage == ManagedJavaStage::Ready {
            let id = snapshot.runtime_id.ok_or_else(|| {
                ManagedJavaError::new("registration", "Ready record has no runtime identity")
            })?;
            return registrar.runtime(id).await?.ok_or_else(|| {
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
            let id = current.runtime_id.ok_or_else(|| {
                ManagedJavaError::new("registration", "Ready record has no runtime identity")
            })?;
            return registrar.runtime(id).await?.ok_or_else(|| {
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
        let job = self
            .operations
            .begin_for(
                format!("java.managed.validate:{id}"),
                vec![
                    crate::data::registry::operation::message::stage::OperationStage::ValidateJava,
                ],
                crate::operations::OperationSubject::ManagedValidation { installation: id },
            )
            .owned_worker();
        self.entries
            .lock()
            .expect("managed entries poisoned")
            .get_mut(&id)
            .expect("validated entry")
            .job_id = Some(job.id());
        // Validation is owned by the core even if the caller's future is dropped.
        let service = Arc::clone(self);
        let (tx, rx) = async_channel::bounded(1);
        ticket.spawn(async move {
            let _operation = operation;
            let java_use = process::JavaUse::admitted(admission).map_err(storage);
            let result: Result<Arc<JavaRuntime>> = std::panic::AssertUnwindSafe(async {
                let java_use = java_use.as_ref().map_err(Clone::clone)?;
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
                        let mut error =
                            ManagedJavaError::caused_by(ManagedJavaErrorKind::Validation, e);
                        error.details = format!(
                            "Executable: {}\nArguments: -XshowSettings:properties -version\n{}",
                            extract::executable(&home).display(),
                            error.details
                        );
                        error
                    })?;
                validate_detected(&home, &snapshot.package, &detected)?;
                let runtime = registrar.register_managed(detected).await?;
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
                registrar.validation_committed(&runtime);
                Ok(runtime)
            })
            .catch_unwind()
            .await
            .unwrap_or_else(|_| {
                Err(ManagedJavaError::new(
                    "worker_panic",
                    "Managed Java validation panicked",
                ))
            });
            let result = match result {
                Ok(runtime) => Ok(runtime),
                Err(error) => Err(service.fail(id, error).await),
            };
            if let Ok(java_use) = java_use {
                java_use.finish(true);
            }
            job.finish(if result.is_ok() {
                crate::data::registry::operation::message::status::Outcome::Completed
            } else {
                crate::data::registry::operation::message::status::Outcome::Failed
            });
            let _ = tx.send(result).await;
        });
        rx.recv()
            .await
            .map_err(|e| ManagedJavaError::caused_by(ManagedJavaErrorKind::Validation, e))?
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
            drop(staging);
            self.update(id, |record| record.snapshot.downloaded_bytes = package.size);
            self.stage(id, ManagedJavaStage::VerifyingChecksum, cancel)
                .await?;
            return self
                .install_verified_archive(id, package, cancel, archive)
                .await;
        }
        let archive = staging.path().join("archive.part");
        let digest = transfer::download(package, &staging, cancel, |bytes| {
            self.update(id, |record| record.snapshot.downloaded_bytes = bytes);
        })
        .await?;
        drop(staging);
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
        use crate::utils::files::identity::FileIdentity;
        use std::ffi::OsStr;
        const CONTENTS: &str = "contents";

        self.stage(id, ManagedJavaStage::Extracting, cancel).await?;
        let worker_cancel = cancel.clone();
        let root = self.root.clone();
        let format = package.format;
        let (root, staging, contents, home) = task::spawn_blocking(move || {
            // Move the whole capability, not only its path (precise closure
            // capture must not leave admission in a cancellable caller future).
            let archive = archive;
            #[cfg(test)]
            tests::ownership_checkpoint(&root, "extracting");
            let root = ownership::open_root(&root)?;
            let staging = root
                .open_relative(&Path::new(".managed/staging").join(id.to_string()))
                .map_err(storage)?;
            let contents = staging
                .create_private_child(OsStr::new(CONTENTS))
                .map_err(storage)?;
            let home = extract::extract_in(&archive.path, &contents, format, &worker_cancel)?;
            Ok::<_, ManagedJavaError>((root, staging, contents, home))
        })
        .await?;
        cancel.check()?;
        let final_relative = PathBuf::from("managed")
            .join(&package.id)
            .join(id.to_string());
        let relative_home = home
            .strip_prefix(contents.path())
            .map_err(|e| ManagedJavaError::caused_by(ManagedJavaErrorKind::Extract, e))?;
        let database = match &self.database {
            Some(database) => database
                .retention_owner()
                .await
                .map_err(storage)?
                .map(|owner| owner.identity),
            None => None,
        };
        let owner = ownership::HomeOwner::new(id, package, relative_home.to_owned(), database)?;
        let portable_home =
            crate::java::paths::portable(&final_relative.join(relative_home)).map_err(storage)?;
        let witness = task::spawn_blocking(move || {
            owner.write_in(&contents)?;
            contents.move_witness().map_err(storage)
        })
        .await?;
        let identity = FileIdentity::from_handle(&witness).map_err(storage)?;
        #[cfg(test)]
        tests::ownership_checkpoint(&self.root, "home-owned");
        self.update(id, |record| {
            record.home = Some(PathBuf::from(portable_home))
        });
        self.persist(id).await?;
        let worker_cancel = cancel.clone();
        let published = task::spawn_blocking(move || {
            worker_cancel.check()?;
            let target = root.create_entry_parent(&final_relative).map_err(storage)?;
            staging
                .publish_directory(
                    OsStr::new(CONTENTS),
                    identity,
                    &target,
                    OsStr::new(&id.to_string()),
                )
                .map_err(|error| {
                    let already_exists = error.kind() == std::io::ErrorKind::AlreadyExists;
                    let mut error = storage(error);
                    if already_exists {
                        error.details =
                            format!("managed destination already exists\n{}", error.details);
                    }
                    error
                })?;
            Ok::<_, ManagedJavaError>(target)
        })
        .await?;
        #[cfg(test)]
        tests::ownership_checkpoint(&self.root, "published");
        let name = id.to_string();
        let current_publication = published.verify_file(OsStr::new(&name), identity);
        // The commit and cancellation request share the lock: exactly one wins.
        let cancelled = {
            let mut entries = self.entries.lock().expect("managed entries poisoned");
            let record = &mut entries.get_mut(&id).expect("installation exists").record;
            if cancel.check().is_err() {
                true
            } else {
                current_publication.map_err(storage)?;
                record.snapshot.stage = ManagedJavaStage::AwaitingValidation;
                false
            }
        };
        if cancelled {
            let cleanup = task::spawn_blocking(move || -> std::io::Result<()> {
                published.verify_file(OsStr::new(&name), FileIdentity::from_handle(&witness)?)?;
                std::fs::remove_dir_all(published.path().join(&name))
            })
            .await;
            if let Err(error) = cleanup {
                tracing::warn!(%id, %error, "left managed Java home after cancelled publication");
            }
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
            record.snapshot.stage = if error.code == ManagedJavaErrorKind::Cancelled {
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

    #[cfg(test)]
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
        let ticket = self
            .tasks
            .admit("recover managed Java records", None)
            .map_err(|error| ManagedJavaError::caused_by(ManagedJavaErrorKind::Shutdown, error))?;
        let root = self.root.clone();
        let db = self.database.clone();
        let report = ticket
            .spawn(async move {
                retention::attach(db.clone(), root.clone()).await?;
                task::spawn_blocking(move || {
                    let _db = db;
                    recovery::load(&root)
                })
                .await
            })
            .await
            .map_err(storage)??;
        for diagnostic in &report.diagnostics {
            tracing::warn!(path = %diagnostic.path.display(), error = %diagnostic.error.diagnostic(), "managed record recovery requires attention");
        }
        *self
            .recovery_diagnostics
            .lock()
            .expect("managed diagnostics poisoned") = report.diagnostics;
        let mut entries = self.entries.lock().expect("managed entries poisoned");
        for record in report.records {
            entries.insert(
                record.snapshot.id,
                Entry {
                    record,
                    cancel: Cancellation::new(),
                    owned: false,
                    job_id: None,
                },
            );
        }
        *initialized = true;
        Ok(())
    }
}

fn storage(error: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> ManagedJavaError {
    ManagedJavaError::caused_by_box(ManagedJavaErrorKind::Storage, error.into())
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
    let record = recovery::decode_bytes(root, &path, &bytes)?;
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
    let home = dunce::canonicalize(home)
        .map_err(|e| ManagedJavaError::caused_by(ManagedJavaErrorKind::Validation, e))?;
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
