mod catalog;
mod extract;
pub mod model;
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
    utils::detect_properties,
};
pub use model::*;
use transfer::Cancellation;

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    snapshot: ManagedJavaSnapshot,
    home: Option<PathBuf>,
}

struct Entry {
    record: Record,
    cancel: Cancellation,
}

pub struct ManagedJavaService {
    root: PathBuf,
    entries: Mutex<HashMap<Uuid, Entry>>,
    initialized: AsyncMutex<bool>,
    catalog: AsyncMutex<Vec<ManagedJavaPackage>>,
}

impl ManagedJavaService {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            entries: Mutex::new(HashMap::new()),
            initialized: AsyncMutex::new(false),
            catalog: AsyncMutex::new(Vec::new()),
        }
    }

    pub async fn catalog(&self, refresh: bool) -> Result<Vec<ManagedJavaPackage>> {
        let mut cache = self.catalog.lock().await;
        if !refresh && !cache.is_empty() {
            return Ok(cache.clone());
        }
        let packages = catalog::fetch(JavaTarget::host()?).await?;
        *cache = packages;
        Ok(cache.clone())
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
            .iter()
            .find(|p| p.id == package_id)
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
        let id = Uuid::new_v4();
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
                    },
                    cancel: cancel.clone(),
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
        let service = Arc::clone(self);
        task::spawn(async move {
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
            home
        };
        // Validation is owned by the core even if the caller's future is dropped.
        let service = Arc::clone(self);
        let registry = java.get_registry();
        let bus = java.managed_bus();
        let (tx, rx) = async_channel::bounded(1);
        task::spawn(async move {
            let result: Result<Arc<JavaRuntime>> = async {
                service.persist(id).await?;
                let snapshot = service.installation(id).await?;
                let home = service.root.join(&home);
                let detected = detect_properties(extract::executable(&home)).await
                    .map_err(|e| ManagedJavaError::new("validation", format!("Executable: {}\nArguments: -XshowSettings:properties -version\n{e}", extract::executable(&home).display())))?;
                validate_detected(&home, &snapshot.package, &detected)?;
                let exec = detected.exec_path.clone();
                let existing = registry.list_all().await.map_err(|e| ManagedJavaError::new("registration", e))?
                    .into_iter().find(|j| Path::new(&j.exec_path) == exec);
                let runtime = match existing {
                    Some(runtime) => runtime,
                    None => {
                        let mut new = NewJavaRuntime::from(detected);
                        new.source = JavaSource::Managed;
                        registry.register(new).await.map_err(|e| ManagedJavaError::new("registration", e))?
                    }
                };
                let mut ready_record = service.entries.lock().expect("managed entries poisoned")
                    .get(&id).map(|entry| entry.record.clone())
                    .ok_or_else(|| ManagedJavaError::new("not_found", id))?;
                ready_record.snapshot.stage = ManagedJavaStage::Ready;
                ready_record.snapshot.runtime_id = Some(runtime.id);
                ready_record.snapshot.error = None;
                // Ready must also be recoverable from disk when polling callers
                // observe it, including after the validation caller is dropped.
                let directory = service.root.join(".managed/operations");
                let persisted_record = ready_record.clone();
                task::spawn_blocking(move || write_record(&directory, &persisted_record)).await?;
                service.update(id, |record| *record = ready_record);
                bus.publish(crate::java::model::JavaEvent::ListChanged);
                Ok(runtime)
            }.await;
            let result = match result {
                Ok(runtime) => Ok(runtime),
                Err(error) => Err(service.fail(id, error).await),
            };
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
        let staging = self.staging(id);
        fs::create_dir_all(&staging).await.map_err(storage)?;
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
        let staging = self.staging(id);
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
        self.stage(id, ManagedJavaStage::Extracting, cancel).await?;
        let destination = staging.join("contents");
        let worker_cancel = cancel.clone();
        let worker_destination = destination.clone();
        let format = package.format;
        let home = task::spawn_blocking(move || {
            extract::extract(&archive, &worker_destination, format, &worker_cancel)
        })
        .await?;
        cancel.check()?;
        let final_relative = PathBuf::from("managed")
            .join(&package.id)
            .join(id.to_string());
        let final_path = self.root.join(&final_relative);
        let relative_home = home
            .strip_prefix(&destination)
            .map_err(|e| ManagedJavaError::new("extract", e))?;
        self.update(id, |record| {
            record.home = Some(final_relative.join(relative_home))
        });
        self.persist(id).await?;
        fs::create_dir_all(final_path.parent().expect("installation has parent"))
            .await
            .map_err(storage)?;
        cancel.check()?;
        fs::rename(&destination, &final_path)
            .await
            .map_err(storage)?;
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
            return cancel.check();
        }
        self.persist(id).await?;
        if let Err(error) = fs::remove_dir_all(&staging).await {
            tracing::warn!(%id, %error, "failed to clean Java staging directory");
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
        if let Err(cleanup) = fs::remove_dir_all(self.staging(id)).await {
            if cleanup.kind() != std::io::ErrorKind::NotFound {
                error
                    .details
                    .push_str(&format!("\nStaging cleanup failed: {cleanup}"));
            }
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

    async fn persist(&self, id: Uuid) -> Result<()> {
        let record = self
            .entries
            .lock()
            .expect("managed entries poisoned")
            .get(&id)
            .map(|e| e.record.clone())
            .ok_or_else(|| ManagedJavaError::new("not_found", id))?;
        let directory = self.root.join(".managed/operations");
        task::spawn_blocking(move || write_record(&directory, &record)).await
    }

    async fn initialize(&self) -> Result<()> {
        let mut initialized = self.initialized.lock().await;
        if *initialized {
            return Ok(());
        }
        let root = self.root.clone();
        let records = task::spawn_blocking(move || -> Result<Vec<Record>> {
            let directory = root.join(".managed/operations");
            std::fs::create_dir_all(&directory).map_err(storage)?;
            let mut records = Vec::new();
            for entry in std::fs::read_dir(&directory).map_err(storage)? {
                let path = entry.map_err(storage)?.path();
                if path.extension().and_then(|s| s.to_str()) != Some("json") {
                    continue;
                }
                let mut record: Record =
                    serde_json::from_slice(&std::fs::read(&path).map_err(storage)?)
                        .map_err(storage)?;
                if record.home.as_ref().is_some_and(|p| {
                    p.is_absolute()
                        || p.components()
                            .any(|c| matches!(c, std::path::Component::ParentDir))
                }) {
                    return Err(ManagedJavaError::new(
                        "storage",
                        format!("Invalid Java home in {}", path.display()),
                    ));
                }
                if record.snapshot.stage.is_active() {
                    let installed = record
                        .home
                        .as_ref()
                        .is_some_and(|h| extract::executable(&root.join(h)).is_file());
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
                    let staging = root
                        .join(".managed/staging")
                        .join(record.snapshot.id.to_string());
                    match std::fs::remove_dir_all(&staging) {
                        Ok(()) => (),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                        Err(error) => return Err(storage(error)),
                    }
                    write_record(&directory, &record)?;
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

fn write_record(directory: &Path, record: &Record) -> Result<()> {
    std::fs::create_dir_all(directory).map_err(storage)?;
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(storage)?;
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
