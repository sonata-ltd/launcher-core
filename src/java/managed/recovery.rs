//! Record-local recovery. Bad records are preserved in place: omitting one from
//! query results never removes its conservative GC root/unknown ownership gate.
use super::{
    ownership, storage, ManagedJavaError, ManagedJavaStage, ManagedRecoveryDiagnostic, Record,
    Result,
};
use crate::utils::{files, object_store::catalog::read_regular};
use std::{fs, path::Path};
use uuid::Uuid;

pub(super) struct Report {
    pub records: Vec<Record>,
    pub diagnostics: Vec<ManagedRecoveryDiagnostic>,
}

pub(super) fn diagnostic(path: &Path, error: ManagedJavaError) -> ManagedRecoveryDiagnostic {
    ManagedRecoveryDiagnostic {
        path: path.to_owned(),
        operation: path
            .file_stem()
            .and_then(|name| name.to_str())
            .and_then(|name| Uuid::parse_str(name).ok())
            .filter(|id| {
                path.file_name().and_then(|name| name.to_str()) == Some(&format!("{id}.json"))
            }),
        error: ManagedJavaError::new(
            "recovery_record",
            format!("{}: {}", path.display(), error.diagnostic()),
        ),
    }
}

pub(super) fn decode(root: &Path, path: &Path) -> Result<Record> {
    decode_bytes(
        root,
        path,
        &read_regular(path, 1024 * 1024).map_err(storage)?,
    )
}

pub(super) fn decode_bytes(root: &Path, path: &Path, bytes: &[u8]) -> Result<Record> {
    let json: serde_json::Value = serde_json::from_slice(bytes).map_err(storage)?;
    for (body, fields) in [
        (&json, &["snapshot", "home", "owner"][..]),
        (
            &json["snapshot"],
            &[
                "id",
                "created_at",
                "package",
                "stage",
                "downloaded_bytes",
                "total_bytes",
                "runtime_id",
                "error",
            ][..],
        ),
        (
            &json["snapshot"]["package"],
            &[
                "id",
                "version",
                "major",
                "target",
                "image_type",
                "url",
                "size",
                "sha256",
                "format",
            ][..],
        ),
        (&json["snapshot"]["package"]["target"], &["os", "arch"][..]),
    ] {
        if body
            .as_object()
            .is_none_or(|object| object.keys().any(|key| !fields.contains(&key.as_str())))
        {
            return Err(storage("unknown managed operation record schema"));
        }
    }
    let record: Record = serde_json::from_value(json).map_err(storage)?;
    if path.file_name().and_then(|name| name.to_str())
        != Some(&format!("{}.json", record.snapshot.id))
        || record.home.as_ref().is_some_and(|home| {
            home.to_string_lossy().contains('\\') || files::child(root, home).is_err()
        })
    {
        return Err(storage("invalid managed operation identity or home"));
    }
    Ok(record)
}

fn recover(root: &Path, record: &mut Record) -> Result<()> {
    let id = record.snapshot.id;
    // Terminal records may still own crash scratch. Ownership must be proven
    // before cleanup; an error preserves both record and its published home.
    if let Some(owner) = record.owner.as_ref().filter(|owner| owner.dead(id)) {
        ownership::cleanup_staging(root, id, owner, true)?;
    }
    if record.snapshot.stage.is_active()
        && record.owner.as_ref().is_some_and(|owner| owner.dead(id))
    {
        let installed = record
            .home
            .as_ref()
            .and_then(|home| {
                files::child(
                    root,
                    &home.join(if cfg!(windows) {
                        "bin/java.exe"
                    } else {
                        "bin/java"
                    }),
                )
                .ok()
            })
            .is_some_and(|executable| executable.is_file());
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
        super::write_record(root, record)?;
    }
    Ok(())
}

pub(super) fn load(root: &Path) -> Result<Report> {
    let _maintenance = ownership::maintenance(root)?;
    #[cfg(test)]
    super::tests::ownership_checkpoint(root, "recovery");
    let directory = files::child(root, Path::new(".managed/operations")).map_err(storage)?;
    fs::create_dir_all(&directory).map_err(storage)?;
    let archive_root = files::child(root, Path::new(".managed/bytes")).map_err(storage)?;
    crate::utils::object_store::ObjectStore::new(archive_root)
        .sweep_staging()
        .map_err(storage)?;
    let mut report = Report {
        records: Vec::new(),
        diagnostics: Vec::new(),
    };
    for entry in fs::read_dir(&directory).map_err(storage)? {
        let entry = entry.map_err(storage)?;
        let path = entry.path();
        if entry.file_name() == ".locks" && entry.file_type().map_err(storage)?.is_dir() {
            continue;
        }
        let mut record = match decode(root, &path) {
            Ok(record) => record,
            Err(error) => {
                report.diagnostics.push(diagnostic(&path, error));
                continue;
            }
        };
        // Recover a copy so a failed durable write never advertises an in-memory
        // transition that did not commit. Unknown scratch remains untouched.
        let mut recovered = record.clone();
        match recover(root, &mut recovered) {
            Ok(()) => record = recovered,
            Err(error) => report.diagnostics.push(diagnostic(&path, error)),
        }
        report.records.push(record);
    }
    report
        .diagnostics
        .sort_by(|left, right| left.path.cmp(&right.path));
    Ok(report)
}
