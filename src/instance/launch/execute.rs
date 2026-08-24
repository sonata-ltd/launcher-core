use std::collections::HashMap;

use async_std::process::Command;

use crate::instance::launch::{natives::Natives, traits::StartupTraits, LaunchError};

use super::LaunchInfo;

pub async fn launch_instance(info: LaunchInfo) -> Result<(), LaunchError> {
    let java_exec_path = info.java_bin_path.exec_path.clone();
    let args = define_launch_args(info).await;

    tracing::debug!("executed with args:\n{:#?}", args);

    let mut child = Command::new(java_exec_path)
        .args(args)
        .spawn()
        .map_err(LaunchError::Spawn)?;

    let status = child.status().await.map_err(LaunchError::Wait)?;

    tracing::info!(%status, "game was stopped");

    Ok(())
}

async fn define_launch_args(info: LaunchInfo) -> Vec<String> {
    let LaunchInfo {
        manifest,
        classpath,
        native_libs,
        natives_dir,
        main_class,
        game_args,
        memory_min,
        memory_max,
        jvm_args,
        ..
    } = info;

    let mut tmp_args: Vec<String> = Vec::new();

    let mut launch_args = vec![
        // "-Xdock:icon=icon.png".to_string(),
        format!("-Xms{memory_min}M"),
        format!("-Xmx{memory_max}M"),
    ];
    launch_args.extend(jvm_args);

    tmp_args.append(&mut launch_args);

    for startup_trait in StartupTraits::extract(&manifest) {
        match startup_trait {
            StartupTraits::FirstThreadOnMacOS => {
                tmp_args.push("-XstartOnFirstThread".to_string());
            }
        }
    }

    // TODO: Determine windows version and add that argument only on windows 10
    #[cfg(target_os = "windows")]
    tmp_args.push("-Dos.name=Windows 10 -Dos.version=10.0".to_string());

    // Handle natives
    if !native_libs.is_empty() {
        match Natives::extract(native_libs, &natives_dir).await {
            Ok(_) => {
                tmp_args.push("-Djava.library.path=".to_owned() + &natives_dir.to_string_lossy());
            }
            Err(e) => {
                tracing::error!("error occured during natives extraction {}", e);
            }
        }
    }

    // tmp_args.push("-Djna.tmpdir=".to_owned() + natives_dir);
    // tmp_args.push("-Dorg.lwjgl.system.SharedLibraryExtractPath=".to_owned() + natives_dir);
    // tmp_args.push("-Dio.netty.native.workdir=/".to_owned() + natives_dir);

    tmp_args.push("-cp".to_string());
    tmp_args.push(classpath);

    // Append main class that contains run point
    if let Some(main_class) = main_class {
        tmp_args.push(main_class);
    } else if let Some(main_class) = manifest["mainClass"].as_str() {
        tmp_args.push(main_class.to_string());
    } else {
        tmp_args.push(String::from("net.minecraft.launchwrapper.Launch"));
    }

    // for arg in info.game_args {
    //     tmp_args.push(arg.0);
    //     tmp_args.push(arg.1);
    // }

    // tmp_args.push("--accessToken".to_string());
    // tmp_args.push("".to_string());

    // tmp_args.push("--userProperties".to_string());
    // tmp_args.push("{}".to_string());

    // tmp_args.push("--username".to_string());
    // tmp_args.push("Melicta".to_string());

    // tmp_args.push("--userType".to_string());
    // tmp_args.push("legacy".to_string());

    // Check for modern manifest pattern
    if let Some(arguments) = manifest["arguments"].as_object() {
        if let Some(manifest_game_args) = arguments["game"].as_array() {
            for arg in manifest_game_args {
                // First we have to handle simple args
                // Iterate other `keys`
                if let Some(simple_arg) = arg.as_str() {
                    handle_simple_arg(simple_arg, &game_args, &mut tmp_args);
                } else if let Some(_complex_arg) = arg.as_object() {
                    tracing::warn!(
                        "found complex argument, but complex arguments is not supported yet"
                    );
                }
            }
        }
    } else if let Some(arguments) = manifest["minecraftArguments"].as_str() {
        tracing::debug!("using legacy manifest extraction pattern");
        let arguments = arguments.split_whitespace();

        // Iterate other `keys`
        for arg in arguments {
            handle_simple_arg(arg, &game_args, &mut tmp_args);
        }
    }

    return tmp_args;
}

fn handle_simple_arg(
    arg: &str,
    defined_map: &HashMap<String, String>,
    output_array: &mut Vec<String>,
) {
    if arg.starts_with("${") {
        let default = " ".to_string();

        // Extract the value from predefined game args or leave it empty
        let value = defined_map.get(arg).unwrap_or(&default);
        output_array.push(value.to_owned());
    } else {
        // Push arg from manifest.
        // Do not use predefined args as we want to
        // build a string with only necessary args from the manifest
        output_array.push(arg.to_string());
    }
}
