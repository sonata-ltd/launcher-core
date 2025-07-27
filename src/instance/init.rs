use std::sync::Arc;

use launch::{ClientOptions, LaunchInfoBuilder};

use crate::{
    instance::websocket::{OperationWsExt, OperationWsMessage},
    websocket::messages::operation::stage::{OperationStage, StageStatus},
};

use super::*;

impl<'a> Instance {
    pub async fn init<'b>(
        mut self,
        _launch_options: Option<ClientOptions>,
        req: &'b EndpointRequest<'a>,
        ws: &WebSocketConnection,
    ) -> Result<(Self, LaunchInfo)> {
        // Init WebSocket sync task
        let ws_status = OperationWsMessage::create_init_task(&ws, &self.request_id).await;

        // Init internal task
        let global_app_state = req.state();
        let task_handle = match global_app_state
            .add_task(Task::new_shared(
                "Initialize instance",
                TaskStatus::Pending,
                None,
                TaskProgress::Indeterminable,
                None,
            ))
            .await
        {
            Ok(handle) => handle,
            Err(e) => return Err(InstanceError::CreationFailed(e.to_string())),
        };

        // Get default paths
        let mut paths = paths::InstancePaths::get_required_paths(
            &self.name,
            &global_app_state.static_data.launcher_root_path,
        );

        // Init launch_info struct builder
        let mut launch_builder = LaunchInfoBuilder::new();

        // Sync WebSocket task and internal task
        ws_status
            .clone()
            .start_stage_indeterminable(OperationStage::FetchManifest)
            .await;
        global_app_state
            .update_task(task_handle.id, |t| {
                t.status = TaskStatus::Running;
                t.stage = Some(OperationStage::FetchManifest);
                t.progress = TaskProgress::Indeterminable;
            })
            .await
            .unwrap();

        // Get Minecraft version manifest - Stage 1
        // TODO: Find already downloaded manifest and redownload
        // it if outdated
        match manifest::download_manifest(&self.url, Some(paths.meta())).await {
            Ok((data, path_to_manifest)) => {
                if let Some(version) = data["id"].as_str() {
                    self.version = Some(version.to_string());
                } else {
                    return Err(InstanceError::VersionNotAvailable);
                }

                self.manifest = data;

                if let Some(path) = path_to_manifest {
                    paths.set_version_manifest_file(path);
                }

                ws_status
                    .clone()
                    .complete_stage(
                        StageStatus::Completed,
                        OperationStage::FetchManifest,
                        0.0,
                        None,
                    )
                    .await;
            }
            Err(e) => {
                return Err(InstanceError::CreationFailed(format!(
                    "Failed to download version manifest: {}",
                    e
                )))
            }
        };

        global_app_state
            .update_task(task_handle.id, |t| {
                t.stage = Some(OperationStage::DownloadLibs);
                t.progress = TaskProgress::Indeterminable;
            })
            .await
            .unwrap();

        // Sync & download all libs needed by this version - Stage 2
        match libs::sync_libs(&self.manifest, &paths, Arc::clone(&ws_status)).await {
            Ok(classpaths) => {
                launch_builder.add_cps(classpaths);
            }
            Err(e) => {
                return Err(InstanceError::CreationFailed(format!(
                    "Failed to download and register libs: {e}"
                )))
            }
        };

        // Get version assets manifest
        let assets_manifest_location = paths.assets().join("indexes");
        let assets_manifest = match manifest::get_assets_manifest(
            &self.manifest,
            &assets_manifest_location.to_str().unwrap(),
        )
        .await
        {
            Ok((data, id)) => {
                launch_builder.add_arg("${assets_index_name}", id);
                data
            }
            Err(e) => {
                return Err(InstanceError::CreationFailed(format!(
                    "Failed to download assets manifest: {}",
                    e
                )))
            }
        };

        global_app_state
            .update_task(task_handle.id, |t| {
                t.stage = Some(OperationStage::DownloadAssets);
                t.progress = TaskProgress::Indeterminable;
            })
            .await
            .unwrap();

        // Sync & download all assets needed by this version - Stage 3
        let assets_objects_dir = paths.assets().join("objects");
        assets::sync_assets(
            &assets_manifest,
            &assets_objects_dir,
            &paths.metacache_file(),
            Arc::clone(&ws_status),
        )
        .await;

        // Initialize instance directory
        self.paths = Some(paths);
        match Self::register_instance(&self).await {
            Ok(_) => {}
            Err(e) => {
                return Err(InstanceError::CreationFailed(format!(
                    "Failed to initialize instance directory: {}",
                    e
                )))
            }
        };

        global_app_state
            .update_task(task_handle.id, |t| {
                t.stage = None;
                t.status = TaskStatus::Completed;
            })
            .await
            .unwrap();

        return Ok((self, launch_builder.build()));
    }
}
