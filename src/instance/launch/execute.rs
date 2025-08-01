use async_std::process::Command;

use super::LaunchInfo;


pub async fn launch_instance<'a>(manifest: serde_json::Value, launch_info: LaunchInfo) {
    let args = define_launch_args(manifest, launch_info);
    println!("{:#?}", args);

    // Command execution
    let output = Command::new("/Users/quartix/Library/Application Support/tlauncher/mojang_jre/java-runtime-delta/mac-os-arm64/java-runtime-delta/jre.bundle/Contents/Home/bin/java")
        .args(args)
        .output()
        .await
        .unwrap();

    println!("{:#?}", output);
}

fn define_launch_args<'a>(manifest: serde_json::Value, info: LaunchInfo) -> Vec<String> {
    let mut tmp_args: Vec<String> = Vec::new();

    let mut jvm_args = vec![
        "-XX:+UnlockExperimentalVMOptions".to_string(),
        "-XX:+UseG1GC".to_string(),
        "-XX:G1NewSizePercent=20".to_string(),
        "-XX:G1ReservePercent=20".to_string(),
        "-XX:MaxGCPauseMillis=50".to_string(),
        "-XX:G1HeapRegionSize=32M".to_string(),
        "-XX:+DisableExplicitGC".to_string(),
        "-XX:+AlwaysPreTouch".to_string(),
        "-XX:+ParallelRefProcEnabled".to_string(),
        "-Xms512M".to_string(),
        "-Xmx1024M".to_string(),
        "-Dfile.encoding=UTF-8".to_string(),
        // "-Dlog4j.configurationFile=/home/quartix/.minecraft/assets/log_configs/patched-variant-2.0.xml".to_string(),
        "-Dfml.ignoreInvalidMinecraftCertificates=true".to_string(),
        "-Dfml.ignorePatchDiscrepancies=true".to_string(),
        "-Djava.net.useSystemProxies=true".to_string(),
    ];
    tmp_args.append(&mut jvm_args);

    tmp_args.push("-XstartOnFirstThread".to_string());
    // tmp_args.push("-Djava.library.path=".to_owned() + r"/Users/quartix/Library/Application Support/minecraft/versions/1.7.4/natives");
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

    for arg in info.game_args {
        tmp_args.push(arg.0);
        tmp_args.push(arg.1);
    }

    tmp_args.push("--accessToken".to_string());
    tmp_args.push("".to_string());

    // if let Some(arguments) = manifest["arguments"].as_object() {
    //     if let Some(game_args) = arguments["game"].as_array() {
    //         for arg in game_args {
    //             if let Some(simple_arg) = arg.as_str() {
    //                 if simple_arg[..2] == *"${" {
    //                     let default = " ".to_string();
    //                     let value = info.game_args.get(simple_arg).unwrap_or(&default);
    //                     tmp_args.push(value.to_owned());
    //                 } else {
    //                     tmp_args.push(simple_arg.to_string());
    //                 }
    //             } else if let Some(_complex_arg) = arg.as_object() {
    //                 // println!("Complex arg: {:#?}", complex_arg);
    //             }
    //         }
    //     }
    // } else if let Some(arguments) = manifest["minecraftArguments"].as_str() {
    //     println!("Using legacy manifest extraction pattern...");
    //     let arguments = arguments.split_whitespace();

    //     for arg in arguments {
    //         if &arg[..2] == "${" {
    //             let default = " ".to_string();
    //             let value = info.game_args.get(arg).unwrap_or(&default);
    //             tmp_args.push(value.to_owned());
    //         } else {
    //             tmp_args.push(arg.to_string());
    //         }
    //     }
    // }

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
