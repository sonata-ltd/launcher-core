use std::collections::HashSet;

use async_std::stream::StreamExt;

use crate::websocket::messages::operation::process::{FileStatus, ProcessTarget};

use super::*;

impl<'a> AssetsData<'a> {
    pub async fn process_futures(
        futures: &mut FuturesUnordered<async_std::task::JoinHandle<std::option::Option<AssetInfo>>>,
        downloaded_assets: &mut HashSet<AssetInfo>,
        max: usize,
        ws_status: OperationWsMessageLocked<'a>,
    ) {
        let mut ws_status = ws_status;

        while let Some(result) = futures.next().await {
            if let Some(asset_info) = result {
                ws_status = ws_status
                    .update_determinable(
                        STAGE_TYPE,
                        Some(ProcessTarget::file(
                            asset_info.name.clone(),
                            FileStatus::Downloaded,
                        )),
                        downloaded_assets.len(),
                        max,
                    )
                    .await;

                downloaded_assets.insert(asset_info);
            }
        }
    }
}
