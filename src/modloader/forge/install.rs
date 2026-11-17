use crate::data::registry::operation::message::output::ProcessorOutput;
use crate::{
    data::registry::operation::message::stage::OperationStage,
    modloader::{
        archive, components,
        error::{ModloaderError, Result},
        forge::{
            model::ModernInstallProfile,
            processor::{self, ProcessorCommand},
            tokens,
        },
        repository::Repository,
    },
    utils::download::transfer::{self, Cancel},
    utils::{files, maven::MavenCoordinate},
};
use serde_json::Value;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

pub async fn installer(
    repo: &Repository,
    component: &Value,
    destination: &Path,
    cancel: &Cancel,
) -> Result<PathBuf> {
    let mut installers = Vec::new();

    for value in component
        .get("mavenFiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(name) = value.get("name").and_then(Value::as_str) {
            if MavenCoordinate::parse(name)?.classifier.as_deref() == Some("installer") {
                installers.push(value);
            }
        }
    }

    if installers.len() != 1 {
        return Err(ModloaderError::Unsupported(
            "component has no unique installer artefact".into(),
        ));
    }

    let descriptor = installers[0]
        .pointer("/downloads/artifact")
        .ok_or_else(|| ModloaderError::Invalid("installer artifact missing".into()))?;
    let url = descriptor
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| ModloaderError::Invalid("installer URL missing".into()))?;
    let hash = descriptor
        .get("sha1")
        .and_then(Value::as_str)
        .ok_or_else(|| ModloaderError::Invalid("installer hash missing".into()))?;

    let _ = repo;

    transfer::download(url, destination, Some(hash), cancel).await?;
    Ok(destination.into())
}

pub struct Prepared {
    pub profile: ModernInstallProfile,
    pub installer: PathBuf,
    pub extracted: PathBuf,
}

impl Prepared {
    pub fn has_client_processors(&self) -> bool {
        self.profile
            .processors
            .iter()
            .any(|processor| processor.applies_to_client())
    }
}

pub async fn prepare(
    installer: PathBuf,
    extracted: PathBuf,
    maven: PathBuf,
    game_version: &str,
    base: &mut components::Components,
) -> Result<Prepared> {
    let input = installer.clone();
    let dir = extracted.clone();

    let value: Value = async_std::task::spawn_blocking(move || {
        archive::extract(&input, &dir, &[])?;
        Ok::<Value, ModloaderError>(serde_json::from_slice(&archive::read(
            &input,
            "install_profile.json",
            4 * 1024 * 1024,
        )?)?)
    })
    .await?;

    if value.get("install").is_some() || value.get("versionInfo").is_some() {
        return Err(ModloaderError::Unsupported(
            "legacy installer; use legacy Prism component".into(),
        ));
    }

    let profile: ModernInstallProfile = serde_json::from_value(value)?;
    if profile.spec.is_some_and(|s| s > 1) || profile.minecraft != game_version {
        return Err(ModloaderError::Unsupported(
            "installer spec or Minecraft version mismatch".into(),
        ));
    }

    let input = installer.clone();
    let json = profile.json.clone();
    let mut patch: Value = async_std::task::spawn_blocking(move || {
        Ok::<Value, ModloaderError>(serde_json::from_slice(&archive::read(
            &input,
            &json,
            4 * 1024 * 1024,
        )?)?)
    })
    .await?;

    if patch
        .get("inheritsFrom")
        .and_then(Value::as_str)
        .is_some_and(|v| v != game_version)
    {
        return Err(ModloaderError::Invalid(
            "installer version targets another Minecraft version".into(),
        ));
    }

    // Modern game arguments extend the inherited Minecraft command. Legacy
    // minecraftArguments remains a complete template handled by components::apply.
    patch
        .as_object_mut()
        .ok_or_else(|| ModloaderError::Invalid("version profile is not object".into()))?
        .remove("inheritsFrom");
    components::apply(base, &patch)?;
    base.maven_files.extend(profile.libraries.clone());

    let embedded = extracted.join("maven");
    if embedded.is_dir() {
        copy_tree(embedded, maven).await?;
    }

    Ok(Prepared {
        profile,
        installer,
        extracted,
    })
}

async fn copy_tree(source: PathBuf, destination: PathBuf) -> Result<()> {
    async_std::task::spawn_blocking(move || {
        let mut stack = vec![(source, destination)];

        while let Some((source, target)) = stack.pop() {
            std::fs::create_dir_all(&target)?;

            for item in std::fs::read_dir(source)? {
                let item = item?;
                let dest = target.join(item.file_name());
                let kind = item.file_type()?;

                if kind.is_symlink() {
                    return Err(ModloaderError::Invalid("embedded Maven symlink".into()));
                }

                if kind.is_dir() {
                    stack.push((item.path(), dest));
                } else {
                    std::fs::copy(item.path(), dest)?;
                }
            }
        }
        Ok(())
    })
    .await
}

pub async fn run(
    prepared: Prepared,
    libraries: PathBuf,
    minecraft: PathBuf,
    game_root: PathBuf,
    java: PathBuf,
    cancel: &Cancel,
    op: &crate::data::registry::operation::handle::OperationHandle,
) -> Result<()> {
    let mut tokens = HashMap::from([
        ("SIDE".into(), "client".into()),
        (
            "MINECRAFT_VERSION".into(),
            prepared.profile.minecraft.clone(),
        ),
        (
            "MINECRAFT_JAR".into(),
            minecraft.to_string_lossy().into_owned(),
        ),
        ("ROOT".into(), game_root.to_string_lossy().into_owned()),
        (
            "INSTALLER".into(),
            prepared.installer.to_string_lossy().into_owned(),
        ),
        (
            "LIBRARY_DIR".into(),
            libraries.to_string_lossy().into_owned(),
        ),
    ]);

    for (key, side) in &prepared.profile.data {
        let Some(raw) = &side.client else {
            continue;
        };

        let value = if raw.starts_with('[') {
            tokens::resolve_argument(raw, &tokens, &libraries)?
        } else if raw.starts_with('\'') {
            tokens::expand_tokens(raw, &tokens)?
        } else {
            let path = PathBuf::from(raw.trim_start_matches('/'));

            files::relative(&path)?;

            let path = files::child(&prepared.extracted, &path)?;

            if !path.is_file() {
                return Err(ModloaderError::Invalid(
                    "installer data file missing".into(),
                ));
            }

            path.to_string_lossy().into_owned()
        };

        tokens.insert(key.clone(), value);
    }

    // let total = prepared
    //     .profile
    //     .processors
    //     .iter()
    //     .filter(|step| step.applies_to_client())
    //     .count();

    // let mut completed = 0;
    let mut output_revision = 0_u64;
    // op.update_stage(OperationStage::RunModloaderProcessors, 0, total, None);

    for (index, step) in prepared.profile.processors.into_iter().enumerate() {
        if !step.applies_to_client() {
            continue;
        }

        cancel.check()?;

        let jar = libraries.join(MavenCoordinate::parse(&step.jar)?.path());
        let input = jar.clone();
        let main_class =
            async_std::task::spawn_blocking(move || archive::main_class(&input)).await?;
        let mut classpath = vec![jar];

        for item in step.classpath {
            classpath.push(libraries.join(MavenCoordinate::parse(&item)?.path()));
        }

        let args = step
            .args
            .iter()
            .map(|arg| tokens::resolve_argument(&arg, &tokens, &libraries))
            .collect::<Result<Vec<_>>>()?;

        let mut outputs = Vec::new();
        for (path, hash) in step.outputs {
            let path = PathBuf::from(tokens::resolve_argument(&path, &tokens, &libraries)?);

            let relative = path.strip_prefix(&libraries).map_err(|_| {
                ModloaderError::Invalid("processor output escapes installation Maven root".into())
            })?;
            let path = files::child(&libraries, relative)?;

            if let Some(parent) = path.parent() {
                async_std::fs::create_dir_all(parent).await?;
            }

            outputs.push((path, tokens::expand_tokens(&hash, &tokens)?));
        }

        let mut publish_output = |lines| {
            output_revision += 1;
            op.output(ProcessorOutput {
                stage: OperationStage::RunModloaderProcessors,
                processor_index: index,
                revision: output_revision,
                lines,
            });
        };
        publish_output(Vec::new());

        processor::run(
            ProcessorCommand {
                java: java.clone(),
                main_class,
                classpath,
                args,
                cwd: prepared.extracted.clone(),
                log: prepared.extracted.join(format!("processor-{index}.log")),
                outputs,
            },
            cancel,
            &mut publish_output,
        )
        .await?;

        // completed += 1;
        // op.update_stage(
        //     OperationStage::RunModloaderProcessors,
        //     completed,
        //     total,
        //     None,
        // );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[cfg(unix)]
    #[async_std::test]
    async fn processor_events_keep_revision_across_client_steps_and_publish_boundaries() {
        use crate::{
            bus::{event::CoreEvent, EventBus},
            data::registry::operation::{
                message::event::{OperationEvent, OperationUpdate},
                OperationRegistry,
            },
        };
        use std::{io::Write, os::unix::fs::PermissionsExt};

        let temp = tempfile::tempdir().unwrap();
        let libraries = temp.path().join("libraries");
        let jar = libraries.join(MavenCoordinate::parse("g:processor:1").unwrap().path());
        std::fs::create_dir_all(jar.parent().unwrap()).unwrap();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(jar).unwrap());
        zip.start_file(
            "META-INF/MANIFEST.MF",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"Manifest-Version: 1.0\nMain-Class: example.Processor\n\n")
            .unwrap();
        zip.finish().unwrap();

        let java = temp.path().join("fake-java");
        std::fs::write(&java, "#!/bin/sh\nprintf 'processor output\\n'\n").unwrap();
        std::fs::set_permissions(&java, std::fs::Permissions::from_mode(0o700)).unwrap();

        let prepared = Prepared {
            profile: serde_json::from_value(json!({
                "minecraft": "1.20.1", "json": "/version.json",
                "processors": [
                    {"jar": "g:processor:1", "sides": ["client"]},
                    {"jar": "g:processor:1", "sides": ["server"]},
                    {"jar": "g:processor:1", "sides": ["client"]},
                ]
            }))
            .unwrap(),
            installer: temp.path().join("installer.jar"),
            extracted: temp.path().join("work"),
        };
        let bus = EventBus::new(32);
        let mut receiver = bus.subscribe();
        let registry = OperationRegistry::new(bus);
        let op = registry.begin(
            "Install".into(),
            vec![OperationStage::RunModloaderProcessors],
        );
        run(
            prepared,
            libraries,
            temp.path().join("minecraft.jar"),
            temp.path().into(),
            java,
            &Cancel::default(),
            &op,
        )
        .await
        .unwrap();

        let mut snapshots = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if let CoreEvent::Operation(message) = event {
                if let OperationEvent::Update(OperationUpdate::Output(output)) = message.data {
                    snapshots.push(output);
                }
            }
        }
        assert_eq!(snapshots.len(), 4);
        assert_eq!(
            snapshots
                .iter()
                .map(|output| output.revision)
                .collect::<Vec<_>>(),
            [1, 2, 3, 4]
        );
        assert_eq!(
            snapshots
                .iter()
                .map(|output| output.processor_index)
                .collect::<Vec<_>>(),
            [0, 0, 2, 2]
        );
        assert!(snapshots[0].lines.is_empty());
        assert!(snapshots[2].lines.is_empty());
        assert_eq!(snapshots[1].lines[0].text, "processor output");
        assert_eq!(snapshots[3].lines[0].text, "processor output");
    }

    #[async_std::test]
    async fn modern_installer_preserves_inherited_game_arguments() {
        use std::io::Write;
        let temp = tempfile::tempdir().unwrap();
        let jar = temp.path().join("installer.jar");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&jar).unwrap());
        for (name, value) in [
            (
                "install_profile.json",
                json!({"spec":1,"minecraft":"1.20.1","json":"/version.json"}),
            ),
            (
                "version.json",
                json!({"inheritsFrom":"1.20.1","mainClass":"example.Bootstrap","arguments":{"game":["--launchTarget","forgeclient"]}}),
            ),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(serde_json::to_string(&value).unwrap().as_bytes())
                .unwrap();
        }
        zip.finish().unwrap();
        let mut base = components::Components {
            manifest: serde_json::json_internal!({
                "libraries": [],"arguments": {
                    "game": ["--username","${auth_player_name}","--gameDir","${game_directory}"],"jvm": []
                }
            }),
            ..Default::default()
        };
        prepare(
            jar,
            temp.path().join("extract"),
            temp.path().join("maven"),
            "1.20.1",
            &mut base,
        )
        .await
        .unwrap();
        assert_eq!(
            base.manifest["arguments"]["game"],
            json!([
                "--username",
                "${auth_player_name}",
                "--gameDir",
                "${game_directory}",
                "--launchTarget",
                "forgeclient"
            ])
        );
    }
}
