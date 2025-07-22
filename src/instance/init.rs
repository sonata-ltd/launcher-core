use std::process::exit;

use super::*;

impl<'a> Instance {
    pub async fn init<'b>(
        &mut self,
        req: &'b EndpointRequest<'a>,
        ws: &WebSocketConnection,
    ) -> Result<LaunchInfo<'b>> {
        // Init task
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
        let mut paths = match get_required_paths(&self.name, &global_app_state.static_data.launcher_root_path) {
            Ok(paths) => paths,
            Err(e) => return Err(e),
        };

        // Update launch arguments if info is not `None`
        self.update_info("${game_directory}", paths.instance.display().to_string());
        self.update_info("${assets_root}", paths.assets.display().to_string());
        self.update_info("${user_properties}", "{}".to_string());

        let msg: WsMessage = OperationMessage {
            base: BaseMessage {
                message_id: "todo",
                operation_id: Some("todo"),
                request_id: Some("todo"),
                timestamp: Utc::now(),
                correlation_id: None,
            },
            data: OperationStart {
                stages: vec![
                    OperationStage::FetchManifest,
                    OperationStage::DownloadLibs,
                    OperationStage::DownloadAssets,
                ],
            }
            .into(),
        }
        .into();

        msg.send(&ws).await.unwrap();

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
        let version_manifest = match manifest::download_manifest(&self.url, Some(&paths.meta)).await {
            Ok((data, path_to_manifest)) => {
                let msg: WsMessage = OperationMessage {
                    base: BaseMessage {
                        message_id: "todo",
                        operation_id: Some("todo"),
                        request_id: Some("todo"),
                        timestamp: Utc::now(),
                        correlation_id: None,
                    },
                    data: OperationUpdate::Completed(StageResult {
                        status: StageStatus::Completed,
                        stage: OperationStage::FetchManifest,
                        duration_secs: 0.0, // TODO
                        error: None,
                    })
                    .into(),
                }
                .into();

                msg.send(&ws).await.unwrap();

                // Update info in Paths structure for instance manifest generation
                if let Some(path_to_manifest) = path_to_manifest {
                    paths.version_manifest_file = Some(PathBuf::from(path_to_manifest));
                }

                data
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
        match libs::sync_libs(&version_manifest, &paths, &ws).await {
            Ok((dir, paths)) => {
                let mut paths_string = String::new();

                for path in paths.iter() {
                    paths_string.push_str(&path);
                }

                // Update launch args if info is not None
                self.update_info("${libs_directory}", dir.to_string());
                self.update_info("${classpath_libs_directories}", paths_string);
            }
            Err(e) => {
                return Err(InstanceError::CreationFailed(format!(
                    "Failed to download and register libs: {e}"
                )))
            }
        };


        // Get version assets manifest
        let assets_manifest_location = paths.assets.to_owned().join("indexes");
        println!("{}", version_manifest);
        let assets_manifest =
            match manifest::get_assets_manifest(&version_manifest, &assets_manifest_location.to_str().unwrap()).await
            {
                Ok((data, id)) => {
                    self.update_info("${assets_index_name}", id.to_string());
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
        let assets_objects_location = paths.assets.display().to_string().to_owned() + "/objects";
        assets::sync_assets(&assets_manifest, &assets_objects_location, ws, &paths).await;
        println!("pizda");

        // Initialize instance directory
        let instance_version = match version_manifest["id"].as_str() {
            Some(data) => InstanceInfo {
                version: data.to_string(),
            },
            None => {
                return Err(InstanceError::CreationFailed(
                    "Failed to determine version".to_string(),
                ))
            }
        };

        match Self::register_instance(&self, &paths, &instance_version).await {
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

        return Ok(LaunchInfo {
            version_manifest,
            paths,
        });
    }
}
