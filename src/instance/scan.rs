use crate::instance::model::InstanceId;

#[derive(Debug, Clone)]
pub struct ScanMessage {
    pub data: ScanData,
}

#[derive(Debug, Clone)]
pub struct ScanData {
    pub integrity: ScanIntegrity,
    pub info: Option<ScanInfo>,
}

#[derive(Debug, Clone)]
pub struct ScanIntegrity {
    pub instance_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScanInfo {
    pub id: InstanceId,
    pub name: String,
    pub version: String,
    pub loader: String,
}

/// Projects the record and a checked directory into the legacy scan message.
pub(super) async fn inspect(
    record: &super::model::InstanceRecord,
    paths: &crate::data::layout::LauncherPaths,
) -> ScanMessage {
    let root =
        crate::utils::files::child(paths.instances(), std::path::Path::new(&record.dir)).ok();
    let instance_path = match root {
        Some(root)
            if async_std::fs::metadata(&root)
                .await
                .is_ok_and(|metadata| metadata.is_dir()) =>
        {
            root.to_str().map(str::to_owned)
        }
        _ => None,
    };
    project(record, instance_path)
}

pub(super) fn project(
    record: &super::model::InstanceRecord,
    instance_path: Option<String>,
) -> ScanMessage {
    ScanMessage {
        data: ScanData {
            integrity: ScanIntegrity { instance_path },
            info: Some(ScanInfo {
                id: record.id,
                name: record.name.clone(),
                version: record.version.clone(),
                loader: record.loader.as_str().to_owned(),
            }),
        },
    }
}
