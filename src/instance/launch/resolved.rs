use super::{args::offline_uuid, arguments, traits::StartupTraits};
use crate::{
    data::layout::LauncherPaths,
    instance::{
        download::{artifacts, transfer},
        error::InstanceError,
        init::model::InstalledProfile,
        model::InstanceRecord,
        settings::EffectiveSettings,
        Result,
    },
    java::model::JavaRuntime,
    modloader::archive,
    utils::files,
};
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf};

pub struct LaunchCommand {
    pub java: PathBuf,
    pub game_dir: PathBuf,
    pub args: Vec<String>,
}

pub async fn prepare(
    rec: &InstanceRecord,
    settings: &EffectiveSettings,
    java: &JavaRuntime,
    paths: &LauncherPaths,
    profile: InstalledProfile,
) -> Result<LaunchCommand> {
    if profile.minecraft != rec.version
        || profile.loader.kind.as_str() != rec.loader
        || profile.loader.version != rec.loader_version
    {
        return Err(InstanceError::RunFailed(
            "instance and installed profile differ; reinstall".into(),
        ));
    }

    // if !profile.java.accepts(java.major)
    //     || java
    //         .arch
    //         .as_deref()
    //         .is_some_and(|arch| select::normalize_arch(arch) != select::host_arch())
    // {
    //     return Err(InstanceError::RunFailed(
    //         "selected Java is incompatible with installed profile".into(),
    //     ));
    // }

    let maven = files::child(paths.libraries(), &profile.libraries_root)?;
    let game = paths.instances().join(&rec.dir);

    async_std::fs::create_dir_all(&game)
        .await
        .map_err(|e| InstanceError::RunFailed(e.to_string()))?;

    let natives = game
        .join(".sonata-natives")
        .join(uuid::Uuid::new_v4().to_string());

    let mut classpath = Vec::new();
    for item in &profile.artifacts {
        let path = files::child(&maven, &item.path)?;

        transfer::check_file(path.clone(), item.sha1.clone()).await?;

        if item.classpath {
            classpath.push(path.clone());
        }

        if item.native {
            let key = item.path.to_string_lossy();
            let excludes = profile
                .manifest
                .pointer("/_sonata_native_excludes")
                .and_then(|map| map.get(key.as_ref()))
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_else(|| vec!["META-INF/".into()]);

            let destination = natives.clone();
            async_std::task::spawn_blocking(move || {
                archive::extract(&path, &destination, &excludes)
            })
            .await?;
        }
    }
    let classpath = std::env::join_paths(classpath)
        .map_err(|e| InstanceError::RunFailed(e.to_string()))?
        .into_string()
        .map_err(|_| InstanceError::RunFailed("non-Unicode classpath".into()))?;

    let mut variables = HashMap::from([
        ("auth_player_name".into(), "Player".into()),
        ("auth_uuid".into(), offline_uuid("Player")),
        ("auth_access_token".into(), String::new()),
        ("clientid".into(), String::new()),
        ("auth_xuid".into(), String::new()),
        ("user_properties".into(), "{}".into()),
        ("user_type".into(), "legacy".into()),
        ("version_name".into(), rec.version.clone()),
        ("version_type".into(), "release".into()),
        ("game_directory".into(), game.to_string_lossy().into_owned()),
        (
            "assets_root".into(),
            paths.assets().to_string_lossy().into_owned(),
        ),
        (
            "assets_index_name".into(),
            profile
                .manifest
                .pointer("/assetIndex/id")
                .and_then(Value::as_str)
                .ok_or_else(|| InstanceError::RunFailed("missing assets index".into()))?
                .into(),
        ),
        (
            "library_directory".into(),
            maven.to_string_lossy().into_owned(),
        ),
        (
            "natives_directory".into(),
            natives.to_string_lossy().into_owned(),
        ),
        ("classpath".into(), classpath.clone()),
        (
            "classpath_separator".into(),
            if cfg!(windows) { ";" } else { ":" }.into(),
        ),
        ("launcher_name".into(), "Sonata".into()),
        ("launcher_version".into(), env!("CARGO_PKG_VERSION").into()),
    ]);

    let mut context = artifacts::host();

    let os_version = os_version();
    context.os_version = &os_version;

    let jvm = profile
        .manifest
        .pointer("/arguments/jvm")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let game_args = profile
        .manifest
        .pointer("/arguments/game")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut args = vec![
        format!("-Xms{}M", settings.memory_min),
        format!("-Xmx{}M", settings.memory_max),
    ];

    args.extend(arguments::flatten(&jvm, &context, &variables)?);

    let has_cp = args.iter().any(|arg| {
        matches!(arg.as_str(), "-cp" | "-classpath" | "--class-path")
            || arg.starts_with("--class-path=")
    });

    if !has_cp {
        args.extend(["-cp".into(), classpath]);
    }
    args.push(format!("-Djava.library.path={}", natives.display()));
    args.extend(settings.jvm_args.clone());

    for startup in StartupTraits::extract(&profile.manifest) {
        match startup {
            StartupTraits::FirstThreadOnMacOS => args.push("-XstartOnFirstThread".into()),
        }
    }

    if let Some(logging) = profile.manifest.pointer("/logging/client") {
        let id = logging
            .pointer("/file/id")
            .and_then(Value::as_str)
            .ok_or_else(|| InstanceError::RunFailed("logging file id missing".into()))?;

        let path = files::child(&maven, &PathBuf::from("sonata-logging").join(id))?;
        variables.insert("path".into(), path.to_string_lossy().into_owned());

        if let Some(argument) = logging.get("argument").and_then(Value::as_str) {
            args.push(arguments::substitute(argument, &variables)?);
        }
    }

    adapt_forge_ignore_list(&profile, &mut args);

    args.push(
        profile
            .manifest
            .get("mainClass")
            .and_then(Value::as_str)
            .ok_or_else(|| InstanceError::RunFailed("mainClass missing".into()))?
            .into(),
    );

    args.extend(arguments::flatten(&game_args, &context, &variables)?);

    Ok(LaunchCommand {
        java: java.exec_path.clone().into(),
        game_dir: game,
        args,
    })
}

fn os_version() -> String {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim()
            .into()
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().into())
            .unwrap_or_default()
    }
    #[cfg(windows)]
    {
        windows_registry::LOCAL_MACHINE
            .open("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
            .ok()
            .and_then(|key| key.get_string("CurrentBuildNumber").ok())
            .map(|v| format!("10.0.{v}"))
            .unwrap_or_default()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        String::new()
    }
}

pub fn spawn(command: LaunchCommand) -> Result<async_std::process::Child> {
    async_std::process::Command::new(command.java)
        .current_dir(command.game_dir)
        .args(command.args)
        .spawn()
        .map_err(|e| InstanceError::RunFailed(e.to_string()))
}

// Official Forge metadata ignores <version>.jar, but our Maven layout uses
// minecraft-<version>-client.jar. Keep that input on the legacy classpath while
// excluding it from BootstrapLauncher's module discovery by its actual basename.
fn adapt_forge_ignore_list(profile: &InstalledProfile, args: &mut [String]) {
    use crate::modloader::model::LoaderKind;
    if !matches!(
        profile.loader.kind,
        LoaderKind::Forge | LoaderKind::NeoForge
    ) {
        return;
    }
    let hash = profile
        .manifest
        .pointer("/mainJar/downloads/artifact/sha1")
        .or_else(|| profile.manifest.pointer("/downloads/client/sha1"))
        .and_then(Value::as_str);
    let Some(hash) = hash else {
        return;
    };
    let names: Vec<_> = profile
        .artifacts
        .iter()
        .filter(|artifact| artifact.classpath && artifact.sha1.eq_ignore_ascii_case(hash))
        .filter_map(|artifact| artifact.path.file_name().and_then(|name| name.to_str()))
        .collect();
    for arg in args {
        let Some(value) = arg.strip_prefix("-DignoreList=") else {
            continue;
        };
        let mut filters: Vec<String> = value
            .split(',')
            .filter(|filter| !filter.is_empty())
            .map(str::to_owned)
            .collect();
        for name in &names {
            if !filters.iter().any(|prefix| name.starts_with(prefix)) {
                filters.push((*name).to_owned());
            }
        }
        *arg = format!("-DignoreList={}", filters.join(","));
    }
}

#[cfg(test)]
mod forge_ignore_tests {
    use super::*;
    use serde_json::json;
    fn profile() -> InstalledProfile {
        serde_json::from_value(json!({
            "format_version":1,"minecraft":"1.20","loader":{"kind":"forge","version":"46.0.14"},
            "manifest":{"mainJar":{"downloads":{"artifact":{"sha1":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}}},
            "java":{"majors":[17],"allow_newer":false},"libraries_root":"generation/maven",
            "artifacts":[{"path":"com/mojang/minecraft/1.20/minecraft-1.20-client.jar","sha1":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","classpath":true,"native":false}],"metadata":[]
        })).unwrap()
    }
    #[test]
    fn adapts_official_version_name_filter_to_actual_maven_filename() {
        let mut args = vec![
            "-DignoreList=asm,securejarhandler,client-extra,1.20.jar".into(),
            "--add-modules".into(),
            "ALL-MODULE-PATH".into(),
        ];
        adapt_forge_ignore_list(&profile(), &mut args);
        assert_eq!(
            args[0],
            "-DignoreList=asm,securejarhandler,client-extra,1.20.jar,minecraft-1.20-client.jar"
        );
        let unchanged = args.clone();
        adapt_forge_ignore_list(&profile(), &mut args);
        assert_eq!(args, unchanged);
        assert_eq!(args[1], "--add-modules");
    }
    #[test]
    fn respects_existing_prefix_and_applies_to_user_override_too() {
        let mut args = vec!["-DignoreList=minecraft-".into(), "-DignoreList=asm".into()];
        adapt_forge_ignore_list(&profile(), &mut args);
        assert_eq!(args[0], "-DignoreList=minecraft-");
        assert!(args[1].ends_with(",minecraft-1.20-client.jar"));
    }
    #[test]
    fn leaves_vanilla_and_legacy_launchers_unchanged() {
        let mut profile = profile();
        profile.loader.kind = crate::modloader::model::LoaderKind::Vanilla;
        let mut args = vec!["-DignoreList=asm".into()];
        adapt_forge_ignore_list(&profile, &mut args);
        assert_eq!(args[0], "-DignoreList=asm");
        profile.loader.kind = crate::modloader::model::LoaderKind::Forge;
        let mut args = vec!["-Xmx1G".into()];
        adapt_forge_ignore_list(&profile, &mut args);
        assert_eq!(args, ["-Xmx1G"]);
    }
    #[test]
    fn does_not_ignore_a_non_classpath_processor_input() {
        let mut profile = profile();
        profile.artifacts[0].classpath = false;
        let mut args = vec!["-DignoreList=asm".into()];
        adapt_forge_ignore_list(&profile, &mut args);
        assert_eq!(args[0], "-DignoreList=asm");
    }
}
