use std::sync::Arc;

use launch::{ClientOptions, LaunchInfoBuilder};

use crate::{
    instance::{
        download::{
            assets::AssetsData, libs::LibsData, manifest::{download_manifest, get_assets_manifest}
        },
        launch::args::ArgType,
        websocket::{OperationWsExt, OperationWsMessage},
    }, utils::download::download_in_json, websocket::messages::operation::stage::{OperationStage, StageStatus}
};

use super::*;

impl<'a> Instance {
    pub async fn init<'b>(
        client_data: InitData,
        _launch_options: Option<ClientOptions>,
        req: &'b EndpointRequest<'a>,
        ws: &WebSocketConnection,
    ) -> Result<(Self, LaunchInfo)> {
        // Init WebSocket sync task
        let ws_status = OperationWsMessage::create_init_task(&ws, &client_data.request_id).await;

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
            &client_data.name,
            &global_app_state.static_data.launcher_root_path,
        );

        // Init launch_info struct builder and set some data available
        let mut launch_builder = LaunchInfoBuilder::new();
        launch_builder.set_arg_value(ArgType::GameDir, &paths.instance());
        launch_builder.set_arg_value(ArgType::AssetsDir, &paths.assets());

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
        let version_manifest = match download_manifest(&client_data.url, Some(paths.meta())).await {
            Ok((data, path_to_manifest)) => {
                // Append path
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

                data
            }
            Err(e) => {
                return Err(InstanceError::CreationFailed(format!(
                    "Failed to download version manifest: {}",
                    e
                )))
            }
        };

        let version_id = match version_manifest["id"].as_str() {
            Some(ver) => {
                launch_builder.set_arg_value(ArgType::Version, ver);
                ver
            }
            None => return Err(InstanceError::VersionNotAvailable),
        };

        global_app_state
            .update_task(task_handle.id, |t| {
                t.stage = Some(OperationStage::DownloadLibs);
                t.progress = TaskProgress::Indeterminable;
            })
            .await
            .unwrap();

        // Sync & download all libs needed by this version - Stage 2
        let tmp_version_manifest = match download_in_json("https://meta.prismlauncher.org/v1/net.minecraft/1.7.4.json").await {
            Ok(manifest) => manifest,
            Err(_) => return Err(InstanceError::VersionNotAvailable)
        };
        match LibsData::sync_libs(&tmp_version_manifest, &paths, Arc::clone(&ws_status), download::libs::ManifestType::Prism).await {
            Ok(mut result) => {
                launch_builder.add_cps(LibsData::get_classpaths_mut(&mut result));
                launch_builder.add_natives(LibsData::take_natives_paths(result));
            },
            Err(e) => {
                return Err(InstanceError::CreationFailed(format!(
                    "Failed to download and register libs: {e}"
                )))
            }
        };

        // Get version assets manifest
        let assets_manifest_location = paths.assets().join("indexes");
        let assets_manifest = match get_assets_manifest(
            &version_manifest,
            &assets_manifest_location.to_str().unwrap(),
        )
        .await
        {
            Ok((asset_manifest, asset_index)) => {
                launch_builder.set_arg_value(ArgType::AssetIndex, asset_index);
                asset_manifest
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
        AssetsData::sync_assets(
            &assets_manifest,
            &assets_objects_dir,
            &paths.metacache_file(),
            Arc::clone(&ws_status),
        )
        .await;

        let instance = Instance {
            name: client_data.name,
            url: client_data.url,
            request_id: client_data.request_id,

            version_id: version_id.to_string(),
            version_manifest,
            paths,
        };

        // Initialize instance directory
        match Self::register_instance(&instance).await {
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

        return Ok((instance, launch_builder.fill_defauls().build()));
    }
}
