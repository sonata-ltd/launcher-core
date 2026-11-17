use std::{
    collections::HashSet,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    java::{model::JavaRuntime, policy::JavaPolicy, select},
    modloader::error::{ModloaderError, Result},
    utils::{cancellation::Cancellation, files, maven::MavenCoordinate},
};

use super::model::Processor;

pub(super) async fn select_for_processors<'a>(
    processors: &[Processor],
    libraries: &Path,
    runtimes: &'a [Arc<JavaRuntime>],
    cancel: &Cancellation,
) -> Result<&'a JavaRuntime> {
    let archives = processors
        .iter()
        .filter(|step| step.applies_to_client())
        .flat_map(|step| std::iter::once(&step.jar).chain(&step.classpath))
        .map(|coordinate| {
            Ok::<_, ModloaderError>(files::child(
                libraries,
                &MavenCoordinate::parse(coordinate)?.path(),
            )?)
        })
        .collect::<Result<HashSet<PathBuf>>>()?;

    let token = cancel.clone();
    let minimum = async_std::task::spawn_blocking(move || {
        let mut minimum = 8;
        for jar in archives {
            minimum = minimum.max(minimum_jar_java(&jar, &token)?);
        }
        Ok::<_, ModloaderError>(minimum)
    })
    .await?;

    cancel.check()?;
    select::pick_with_policy(
        runtimes,
        &JavaPolicy {
            majors: vec![minimum],
            allow_newer: true,
        },
        select::host_arch(),
    )
    .map(|(runtime, _)| runtime)
    .ok_or(ModloaderError::ProcessorJavaUnavailable { minimum })
}

fn minimum_jar_java(path: &Path, cancel: &Cancellation) -> Result<u32> {
    cancel.check()?;
    let mut jar = zip::ZipArchive::new(File::open(path)?).map_err(|error| {
        ModloaderError::Invalid(format!(
            "cannot read processor classpath {}: {error}",
            path.display()
        ))
    })?;
    if jar.len() > 100_000 {
        return Err(ModloaderError::Invalid(format!(
            "processor classpath archive has too many entries: {}",
            path.display()
        )));
    }

    let mut minimum = 8;
    for index in 0..jar.len() {
        cancel.check()?;
        let mut entry = jar.by_index(index).map_err(|error| {
            ModloaderError::Invalid(format!(
                "cannot read processor classpath {}: {error}",
                path.display()
            ))
        })?;
        // Classpath launch ignores module descriptors. Multi-release overrides
        // are only selected by Java versions that can already load them.
        if !entry.name().ends_with(".class")
            || entry.name() == "module-info.class"
            || entry.name().starts_with("META-INF/versions/")
        {
            continue;
        }
        let mut header = [0u8; 8];
        entry.read_exact(&mut header)?;
        if header[..4] != [0xca, 0xfe, 0xba, 0xbe] {
            return Err(ModloaderError::Invalid(format!(
                "invalid class header in {}: {}",
                path.display(),
                entry.name()
            )));
        }
        let minor = u16::from_be_bytes([header[4], header[5]]);
        let major = u16::from_be_bytes([header[6], header[7]]);
        if major < 45 || minor == u16::MAX {
            return Err(ModloaderError::Unsupported(format!(
                "unsupported class version {major}.{minor} in {}: {}",
                path.display(),
                entry.name()
            )));
        }
        minimum = minimum.max(u32::from(major) - 44);
    }
    Ok(minimum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn jar(root: &Path, coordinate: &str, entries: &[(&str, u16)]) {
        let path = root.join(MavenCoordinate::parse(coordinate).unwrap().path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut jar = zip::ZipWriter::new(File::create(path).unwrap());
        for (name, major) in entries {
            jar.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            jar.write_all(&[0xca, 0xfe, 0xba, 0xbe, 0, 0]).unwrap();
            jar.write_all(&major.to_be_bytes()).unwrap();
        }
        jar.finish().unwrap();
    }

    fn runtime(major: u32) -> Arc<JavaRuntime> {
        Arc::new(JavaRuntime {
            id: i64::from(major),
            major,
            version: major.to_string(),
            arch: None,
            exec_path: "java".into(),
            home_path: String::new(),
            vendor: None,
            source: Default::default(),
        })
    }

    #[async_std::test]
    async fn chooses_nearest_runtime_supported_by_processor_and_dependencies() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        jar(
            root,
            "example:main:1",
            &[
                ("example/Main.class", 52),
                ("module-info.class", 65),
                ("META-INF/versions/21/example/Main.class", 65),
            ],
        );
        jar(root, "example:dep:1", &[("example/Dependency.class", 61)]);
        let steps: Vec<Processor> = serde_json::from_value(serde_json::json!([
            {"jar": "example:main:1", "classpath": ["example:dep:1"]},
            {"jar": "example:unavailable:1", "sides": ["server"]}
        ]))
        .unwrap();
        let cancel = Cancellation::new();
        let runtimes = [runtime(8), runtime(17), runtime(21)];
        let selected = select_for_processors(&steps, root, &runtimes, &cancel)
            .await
            .unwrap();
        assert_eq!(selected.major, 17);
        assert!(matches!(
            select_for_processors(&steps, root, &runtimes[..1], &cancel).await,
            Err(ModloaderError::ProcessorJavaUnavailable { minimum: 17 })
        ));
    }
}
