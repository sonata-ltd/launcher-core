use std::{collections::HashMap, path::PathBuf};

use async_std::process::Command;

use crate::instance::launch::natives::Natives;

use super::LaunchInfo;

pub async fn launch_instance<'a>(manifest: serde_json::Value, launch_info: LaunchInfo) {
    let args = define_launch_args(manifest, launch_info).await;
    println!("{:#?}", args);

    // Command execution
    let output =
        Command::new("/Library/Internet Plug-Ins/JavaAppletPlugin.plugin/Contents/Home/bin/java")
            .args(args)
            .output()
            .await
            .unwrap();

    println!("{:#?}", output);
}

async fn define_launch_args<'a>(manifest: serde_json::Value, info: LaunchInfo) -> Vec<String> {
    let mut tmp_args: Vec<String> = Vec::new();

    let mut jvm_args = vec![
        // "-Xdock:icon=icon.png".to_string(),
        // r#"-Xdock:name="Sonata Launcher: 1.7.4""#.to_string(),
        "-Xms512M".to_string(),
        "-Xmx1024M".to_string(),
    ];
    tmp_args.append(&mut jvm_args);

    #[cfg(target_os = "macos")]
    // tmp_args.push("-XstartOnFirstThread".to_string());

    // TODO: Determine windows version and add that argument only on windows 10
    #[cfg(target_os = "windows")]
    tmp_args.push("-Dos.name=Windows 10 -Dos.version=10.0".to_string());

    // Handle natives
    if !info.native_libs.is_empty() {
        let native_dir = PathBuf::from("/Users/quartix/.sonata/instances/1.7.4/natives");

        match Natives::extract(info.native_libs, &native_dir).await {
            Ok(_) => {
                tmp_args.push("-Djava.library.path=".to_owned() + &native_dir.to_string_lossy());
            }
            Err(e) => {
                eprintln!("Error occured during natives extraction: {}", e);
            }
        }
    }

    // tmp_args.push("-Djna.tmpdir=".to_owned() + natives_dir);
    // tmp_args.push("-Dorg.lwjgl.system.SharedLibraryExtractPath=".to_owned() + natives_dir);
    // tmp_args.push("-Dio.netty.native.workdir=/".to_owned() + natives_dir);

    tmp_args.push("-cp".to_string());
    tmp_args.push(info.classpath);

    // Append main class that contains run point
    if let Some(main_class) = info.main_class {
        tmp_args.push(main_class);
    } else if let Some(main_class) = manifest["mainClass"].as_str() {
        tmp_args.push(main_class.to_string());
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
    println!("{:#?}", info.game_args);
    if let Some(arguments) = manifest["arguments"].as_object() {
        if let Some(game_args) = arguments["game"].as_array() {
            for arg in game_args {

                // First we have to handle simple args
                // Iterate other `keys`
                if let Some(simple_arg) = arg.as_str() {
                    handle_simple_arg(simple_arg, &info.game_args, &mut tmp_args);
                } else if let Some(complex_arg) = arg.as_object() {
                    println!("Complex arg: {:#?}", complex_arg);
                    println!("Complex args is not implemented yet");
                }
            }
        }
    } else if let Some(arguments) = manifest["minecraftArguments"].as_str() {
        println!("Using legacy manifest extraction pattern...");
        let arguments = arguments.split_whitespace();

        // Iterate other `keys`
        for arg in arguments {
            handle_simple_arg(arg, &info.game_args, &mut tmp_args);
        }
    }

    return tmp_args;
}

fn _extract_launch_args<'a>(manifest: serde_json::Value) -> Vec<(&'a str, &'a str)> {
    if let Some(arguments) = manifest["arguments"]["game"].as_array() {
        for argument in arguments {
            println!("{}", argument);
        }
    }

    vec![("asd", "asd")]
}

fn handle_simple_arg(
    arg: &str,
    defined_map: &HashMap<String, String>,
    output_array: &mut Vec<String>,
) {
    if &arg[..2] == "${" {
        let default = " ".to_string();

        // Extract the value from predefined game args or leave it empty
        println!("{}", arg);
        let value = defined_map.get(arg).unwrap_or(&default);
        output_array.push(value.to_owned());
    } else {
        // Push arg from manifest.
        // Do not use predefined args as we want to
        // build a string with only necessary args from the manifest
        output_array.push(arg.to_string());
    }
}
