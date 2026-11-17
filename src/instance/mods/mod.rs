use std::path::PathBuf;

use async_std::stream::StreamExt;

use crate::instance::{
    error::Result,
    model::{InstanceId, InstanceMod},
};

pub async fn resolve_mods(instance_id: InstanceId, path: PathBuf) -> Result<Vec<InstanceMod>> {
    let mut entries = async_std::fs::read_dir(path)
        .await
        .expect("dir should be readable");

    println!("read");

    let mut mods: Vec<InstanceMod> = Vec::new();

    while let Some(entry) = entries.next().await {
        let entry = entry.expect("entry should be readable");

        println!("adding");

        mods.push(InstanceMod {
            instance_id,
            name: entry.file_name(),
        });
    }

    Ok(mods)
}
