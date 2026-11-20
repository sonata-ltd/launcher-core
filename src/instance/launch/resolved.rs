use super::{
    args::offline_uuid,
    arguments,
    natives::{NativeDirectory, Natives},
    traits::StartupTraits,
    LaunchError,
};
use crate::{
    data::layout::LauncherPaths,
    instance::{
        download::{assets::index, libs::LibInfo, transfer},
        error::InstanceError,
        init::model::{InstalledArtifact, InstalledProfile},
        model::InstanceRecord,
        settings::EffectiveSettings,
        Result,
    },
    java::{model::JavaRuntime, policy::JavaPolicy, select},
    modloader::model::LoaderSpec,
    utils::files,
    version::rules::RuleContext,
};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub struct LaunchCommand {
    pub java: PathBuf,
    pub game_dir: PathBuf,
    pub args: Vec<String>,
    natives: NativeDirectory,
    runtime_pins: Vec<crate::utils::process_pin::ProcessPin>,
}

impl LaunchCommand {
    pub(crate) fn retain_runtime(&mut self, pin: crate::utils::process_pin::ProcessPin) {
        self.runtime_pins.push(pin);
    }
}

fn validate_java(java: &JavaRuntime, policy: &JavaPolicy) -> Result<()> {
    if !policy.accepts(java.major) {
        return Err(LaunchError::IncompatibleJavaVersion {
            actual: java.major,
            supported: policy.majors.clone(),
            allow_newer: policy.allow_newer,
        }
        .into());
    }

    validate_java_architecture(java)
}

fn validate_java_architecture(java: &JavaRuntime) -> Result<()> {
    let host = select::host_arch();
    if let Some(actual) = java.arch.as_ref().filter(|_| !select::arch_ok(java, host)) {
        return Err(LaunchError::IncompatibleJavaArchitecture {
            actual: actual.clone(),
            host,
        }
        .into());
    }

    Ok(())
}

pub async fn prepare(
    rec: &InstanceRecord,
    settings: &EffectiveSettings,
    java: &JavaRuntime,
    paths: &LauncherPaths,
    profile: InstalledProfile,
) -> Result<LaunchCommand> {
    if profile.minecraft != rec.version
        || profile.loader.kind != rec.loader
        || profile.loader.version != rec.loader_version
    {
        return Err(InstanceError::RunFailed(
            "instance and installed profile differ; reinstall".into(),
        ));
    }

    validate_java(java, &profile.java)?;
    if profile.manifest[index::VERIFIED_INDEX].as_bool() == Some(true) {
        index::verify_published(&profile.manifest, paths.assets())
            .await
            .map_err(InstanceError::Manifest)?;
    }
    let maven = files::child(paths.libraries(), &profile.libraries_root)?;
    prepare_command(rec, settings, java, paths, &maven, profile, false).await
}

pub(super) async fn prepare_legacy(
    rec: &InstanceRecord,
    settings: &EffectiveSettings,
    java: &JavaRuntime,
    paths: &LauncherPaths,
    manifest: Value,
    libs: Vec<LibInfo>,
) -> Result<LaunchCommand> {
    let majors = crate::version::provider::java_majors(&manifest);
    let policy = JavaPolicy {
        allow_newer: majors.iter().any(|&major| major > 8),
        majors,
    };
    if policy.majors.is_empty() {
        validate_java_architecture(java)?;
    } else {
        validate_java(java, &policy)?;
    }

    let maven = paths.libraries();
    let mut artifacts = Vec::with_capacity(libs.len());
    for lib in libs {
        let path = Path::new(lib.path());
        let path = path.strip_prefix(maven).map_err(|_| {
            InstanceError::RunFailed(format!(
                "library is outside the configured directory: {}",
                path.display()
            ))
        })?;
        artifacts.push(InstalledArtifact {
            path: path.to_owned(),
            sha1: lib.hash().clone(),
            classpath: true,
            native: lib.is_native(),
        });
    }

    let profile = InstalledProfile {
        format_version: 1,
        minecraft: rec.version.clone(),
        loader: LoaderSpec::vanilla(),
        manifest,
        java: policy,
        libraries_root: PathBuf::new(),
        artifacts,
        metadata: Vec::new(),
    };
    prepare_command(rec, settings, java, paths, maven, profile, true).await
}

async fn prepare_command(
    rec: &InstanceRecord,
    settings: &EffectiveSettings,
    java: &JavaRuntime,
    paths: &LauncherPaths,
    maven: &Path,
    profile: InstalledProfile,
    legacy: bool,
) -> Result<LaunchCommand> {
    let game = files::child(paths.instances(), Path::new(&rec.dir))?;

    async_std::fs::create_dir_all(&game)
        .await
        .map_err(InstanceError::Io)?;

    let native_root = game.join(".sonata-natives");
    let natives = async_std::task::spawn_blocking(move || NativeDirectory::create(&native_root))
        .await
        .map_err(LaunchError::Natives)?;

    let mut classpath = Vec::new();
    for item in &profile.artifacts {
        let path = files::child(maven, &item.path)?;

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

            Natives::extract_archive(path, natives.clone(), excludes)
                .await
                .map_err(LaunchError::Natives)?;
        }
    }
    let classpath = std::env::join_paths(classpath)
        .map_err(|error| LaunchError::ClasspathSeparator(error.to_string()))?
        .into_string()
        .map_err(|_| LaunchError::ClasspathNotUnicode)?;

    let asset_index = super::super::download::manifest::asset_index_id(&profile.manifest)
        .map_err(InstanceError::Manifest)?;
    let mut variables = HashMap::from([
        ("auth_player_name".into(), "Player".into()),
        ("auth_uuid".into(), offline_uuid("Player")),
        ("auth_access_token".into(), String::new()),
        ("auth_session".into(), String::new()),
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
        ("assets_index_name".into(), asset_index.into()),
        (
            "game_assets".into(),
            paths
                .assets()
                .join("virtual")
                .join(asset_index)
                .to_string_lossy()
                .into_owned(),
        ),
        (
            "library_directory".into(),
            maven.to_string_lossy().into_owned(),
        ),
        (
            "natives_directory".into(),
            natives.as_ref().to_string_lossy().into_owned(),
        ),
        ("classpath".into(), classpath.clone()),
        (
            "classpath_separator".into(),
            if cfg!(windows) { ";" } else { ":" }.into(),
        ),
        ("launcher_name".into(), "Sonata".into()),
        ("launcher_version".into(), env!("CARGO_PKG_VERSION").into()),
    ]);

    let context = RuleContext::host();

    let jvm = profile
        .manifest
        .pointer("/arguments/jvm")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let game_args = match profile
        .manifest
        .pointer("/arguments/game")
        .and_then(Value::as_array)
    {
        Some(args) => args.clone(),
        None => match profile
            .manifest
            .get("minecraftArguments")
            .and_then(Value::as_str)
        {
            Some(raw) => shlex::split(raw)
                .ok_or(LaunchError::Arguments(arguments::ArgumentError::Invalid(
                    "invalid legacy argument quoting",
                )))?
                .into_iter()
                .map(Value::String)
                .collect(),
            None => Vec::new(),
        },
    };

    let mut args = vec![
        format!("-Xms{}M", settings.memory_min),
        format!("-Xmx{}M", settings.memory_max),
    ];

    args.extend(arguments::flatten(&jvm, &context, &variables).map_err(LaunchError::from)?);

    let has_cp = args.iter().any(|arg| {
        matches!(arg.as_str(), "-cp" | "-classpath" | "--class-path")
            || arg.starts_with("--class-path=")
    });

    if !has_cp {
        args.extend(["-cp".into(), classpath]);
    }
    args.push(format!(
        "-Djava.library.path={}",
        natives.as_ref().display()
    ));
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

        let path = files::child(maven, &PathBuf::from("sonata-logging").join(id))?;
        variables.insert("path".into(), path.to_string_lossy().into_owned());

        if let Some(argument) = logging.get("argument").and_then(Value::as_str) {
            if !legacy || path.is_file() {
                args.push(arguments::substitute(argument, &variables).map_err(LaunchError::from)?);
            }
        }
    }

    adapt_forge_ignore_list(&profile, &mut args);

    args.push(
        profile
            .manifest
            .get("mainClass")
            .and_then(Value::as_str)
            .or_else(|| legacy.then_some("net.minecraft.launchwrapper.Launch"))
            .ok_or_else(|| InstanceError::RunFailed("mainClass missing".into()))?
            .into(),
    );

    args.extend(arguments::flatten(&game_args, &context, &variables).map_err(LaunchError::from)?);

    Ok(LaunchCommand {
        java: java.exec_path.clone().into(),
        game_dir: game,
        args,
        natives,
        runtime_pins: Vec::new(),
    })
}

pub(crate) fn spawn(
    mut command: LaunchCommand,
) -> Result<(async_std::process::Child, NativeDirectory)> {
    for pin in &command.runtime_pins {
        pin.before_spawn().map_err(LaunchError::Spawn)?;
    }
    command
        .natives
        .before_spawn()
        .map_err(LaunchError::Natives)?;
    let child = async_std::process::Command::new(command.java)
        .current_dir(command.game_dir)
        .args(command.args)
        .spawn()
        .map_err(LaunchError::Spawn)?;
    command.natives.record_child(child.id());
    for pin in command.runtime_pins {
        pin.record_child(child.id());
        command.natives.retain_runtime(pin);
    }
    Ok((child, command.natives))
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

    #[cfg(unix)]
    #[async_std::test]
    async fn game_spawn_hands_all_resource_pins_to_child_and_completes_only_after_exit() {
        use crate::utils::process_pin::{allows_cleanup, ProcessPin};
        for confirmed in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let libraries = dir.path().join("libraries");
            let java = dir.path().join("java");
            let mut command = LaunchCommand {
                java: "sh".into(),
                game_dir: dir.path().into(),
                args: vec!["-c".into(), "exit 0".into()],
                natives: NativeDirectory::create(&dir.path().join("natives")).unwrap(),
                runtime_pins: Vec::new(),
            };
            for resource in [&libraries, &java] {
                command.retain_runtime(ProcessPin::create(resource).unwrap());
            }
            let (mut child, natives) = spawn(command).unwrap();
            for resource in [&libraries, &java] {
                assert!(!allows_cleanup(resource).unwrap());
                let pin = std::fs::read_dir(resource.join(".sonata-processes"))
                    .unwrap()
                    .next()
                    .unwrap()
                    .unwrap()
                    .path();
                assert_eq!(
                    std::fs::read_to_string(pin.join(".sonata-child")).unwrap(),
                    child.id().to_string()
                );
            }
            assert!(child.status().await.unwrap().success());
            natives.finish(confirmed);
            for resource in [&libraries, &java] {
                assert_eq!(allows_cleanup(resource).unwrap(), confirmed);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn failed_game_spawn_preserves_all_unknown_child_intents() {
        use crate::utils::process_pin::{allows_cleanup, ProcessPin};
        let dir = tempfile::tempdir().unwrap();
        let resources = [dir.path().join("java"), dir.path().join("libraries")];
        let mut command = LaunchCommand {
            java: dir.path().join("missing-java"),
            game_dir: dir.path().into(),
            args: Vec::new(),
            natives: NativeDirectory::create(&dir.path().join("natives")).unwrap(),
            runtime_pins: Vec::new(),
        };
        for resource in &resources {
            command.retain_runtime(ProcessPin::create(resource).unwrap());
        }
        assert!(spawn(command).is_err());
        for resource in &resources {
            assert!(!allows_cleanup(resource).unwrap());
        }
    }
    fn profile() -> InstalledProfile {
        serde_json::from_value(json!({
            "format_version":1,"minecraft":"1.20","loader":{"kind":"forge","version":"46.0.14"},
            "manifest":{"mainJar":{"downloads":{"artifact":{"sha1":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}}},
            "java":{"majors":[17],"allow_newer":false},"libraries_root":"generation/maven",
            "artifacts":[{"path":"com/mojang/minecraft/1.20/minecraft-1.20-client.jar","sha1":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","classpath":true,"native":false}],"metadata":[]
        })).unwrap()
    }
    #[async_std::test]
    async fn prepare_rejects_incompatible_java_before_creating_the_game_directory() {
        let temp = tempfile::tempdir().unwrap();
        let paths = LauncherPaths::resolve(&crate::Config {
            root_override: Some(temp.path().to_owned()),
            ..crate::Config::init()
        })
        .await
        .unwrap();
        let rec = InstanceRecord {
            id: 1.into(),
            name: "Example".into(),
            dir: "example".into(),
            version: "1.20".into(),
            loader: crate::modloader::model::LoaderKind::Forge,
            loader_version: Some("46.0.14".into()),
            manifest_url: String::new(),
            meta_provider: "prism".into(),
        };
        let settings = EffectiveSettings {
            java_runtime: Some(1),
            memory_min: 512,
            memory_max: 4096,
            jvm_args: vec![],
        };
        let mut java = JavaRuntime {
            id: 1,
            version: "21".into(),
            major: 21,
            arch: None,
            exec_path: "java".into(),
            home_path: String::new(),
            vendor: None,
            source: Default::default(),
        };
        assert!(matches!(
            prepare(&rec, &settings, &java, &paths, profile()).await,
            Err(InstanceError::Launch(
                LaunchError::IncompatibleJavaVersion { actual: 21, .. }
            ))
        ));
        java.major = 17;
        java.arch = Some(
            if select::host_arch() == "x86_64" {
                "aarch64"
            } else {
                "x86_64"
            }
            .into(),
        );
        assert!(matches!(
            prepare(&rec, &settings, &java, &paths, profile()).await,
            Err(InstanceError::Launch(
                LaunchError::IncompatibleJavaArchitecture { .. }
            ))
        ));
        assert!(!paths.instances().join(&rec.dir).exists());

        java.arch = None;
        assert!(validate_java(&java, &profile().java).is_ok());
    }

    #[async_std::test]
    async fn legacy_and_persisted_profiles_share_rules_arguments_and_working_directory() {
        let directory = tempfile::tempdir().unwrap();
        let paths = LauncherPaths::resolve(&crate::Config {
            root_override: Some(directory.path().to_owned()),
            ..crate::Config::init()
        })
        .await
        .unwrap();
        let rec = InstanceRecord {
            id: 1.into(),
            name: "Example".into(),
            dir: "Example Instance".into(),
            version: "1.20".into(),
            loader: crate::modloader::model::LoaderKind::Vanilla,
            loader_version: None,
            manifest_url: String::new(),
            meta_provider: "prism".into(),
        };
        let settings = EffectiveSettings {
            java_runtime: Some(1),
            memory_min: 512,
            memory_max: 4096,
            jvm_args: vec!["-Dtest=a b".into()],
        };
        let java = JavaRuntime {
            id: 1,
            major: 17,
            version: "17".into(),
            arch: None,
            exec_path: "java".into(),
            home_path: String::new(),
            vendor: None,
            source: Default::default(),
        };
        let client = paths.libraries().join("legacy/client.jar");
        std::fs::create_dir_all(client.parent().unwrap()).unwrap();
        std::fs::write(&client, b"fixture").unwrap();
        let sha1 = files::sha1_file(&client).unwrap();
        let lib = LibInfo::new(
            "client".into(),
            sha1.clone(),
            client.clone(),
            String::new(),
            false,
        )
        .unwrap();
        let manifest = json!({
            "assetIndex": {"id": "test"}, "mainClass": "Example",
            "javaVersion": {"majorVersion": 17},
            "arguments": {
                "jvm": [{"rules": [{"action": "allow"}], "value": "-Dname=${auth_player_name}"}],
                "game": [
                    {"rules": [{"action": "allow"}, {"action": "disallow"}], "value": "--skipped"},
                    {"rules": [{"action": "allow"}], "value": ["--dir", "${game_directory}"]}
                ]
            }
        });
        let legacy = prepare_legacy(
            &rec,
            &settings,
            &java,
            &paths,
            manifest.clone(),
            vec![lib.clone()],
        )
        .await
        .unwrap();
        let installed = prepare(
            &rec,
            &settings,
            &java,
            &paths,
            InstalledProfile {
                format_version: 1,
                minecraft: rec.version.clone(),
                loader: LoaderSpec::vanilla(),
                manifest: manifest.clone(),
                java: JavaPolicy {
                    majors: vec![17],
                    allow_newer: false,
                },
                libraries_root: "legacy".into(),
                artifacts: vec![InstalledArtifact {
                    path: "client.jar".into(),
                    sha1,
                    classpath: true,
                    native: false,
                }],
                metadata: Vec::new(),
            },
        )
        .await
        .unwrap();
        assert_eq!(legacy.game_dir, installed.game_dir);
        assert_eq!(legacy.game_dir, paths.instances().join(&rec.dir));
        let without_natives = |args: Vec<String>| {
            args.into_iter()
                .filter(|arg| !arg.starts_with("-Djava.library.path="))
                .collect::<Vec<_>>()
        };
        assert!(!legacy.args.iter().any(|arg| arg == "--skipped"));
        assert!(legacy.args.iter().any(|arg| arg == "-Dname=Player"));
        assert_eq!(
            without_natives(legacy.args),
            without_natives(installed.args)
        );

        let mut raw = manifest;
        raw.as_object_mut().unwrap().remove("arguments");
        raw["minecraftArguments"] = json!(r#"--note "one value" --player ${auth_player_name}"#);
        let command = prepare_legacy(&rec, &settings, &java, &paths, raw, vec![lib])
            .await
            .unwrap();
        assert!(command.args.ends_with(&[
            "--note".into(),
            "one value".into(),
            "--player".into(),
            "Player".into()
        ]));
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
