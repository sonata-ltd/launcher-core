use serde::Deserialize;
use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    path::{Path, PathBuf},
};
use strum::IntoEnumIterator;
use thiserror::Error;

use crate::{
    instance::{
        launch::args::ArgType,
        settings::{DEFAULT_MEMORY_MAX, DEFAULT_MEMORY_MIN},
    },
    java::model::JavaRuntime,
};

pub mod args;
pub mod execute;
pub mod natives;
pub mod prepare;
pub mod traits;

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("classpath contains the platform path separator: {0}")]
    ClasspathSeparator(String),

    #[error("classpath is not valid unicode")]
    ClasspathNotUnicode,

    #[error("no java runtime selected")]
    JavaRuntimeMissing,

    #[error("no version manifest provided")]
    ManifestMissing,

    #[error("failed to spawn java process: {0}")]
    Spawn(#[source] std::io::Error),

    #[error("failed to wait for java process: {0}")]
    Wait(#[source] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct LaunchInfo {
    manifest: serde_json::Value,
    classpath: String,
    native_libs: Vec<PathBuf>,
    natives_dir: PathBuf,
    main_class: Option<String>,
    game_args: HashMap<String, String>,
    memory_min: i64,
    memory_max: i64,
    jvm_args: Vec<String>,
    java_bin_path: JavaRuntime,
    version: String,
}

#[derive(Debug, Default)]
pub struct LaunchInfoBuilder {
    manifest: Option<serde_json::Value>,
    classpath: Vec<PathBuf>,
    native_libs: Vec<PathBuf>,
    natives_dir: Option<PathBuf>,
    main_class: Option<String>,
    game_args: HashMap<String, String>,
    memory_min: Option<i64>,
    memory_max: Option<i64>,
    jvm_args: Vec<String>,
    preferred_java: Option<JavaRuntime>,
    version: Option<String>,
}

#[derive(Deserialize, Debug)]
pub struct ClientOptions {
    pub classpath: Vec<String>,
    pub main_class: String,
    pub game_args: HashMap<String, String>,
}

impl LaunchInfoBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_manifest(&mut self, manifest: serde_json::Value) -> &mut Self {
        self.manifest = Some(manifest);
        self
    }

    pub fn set_main_class<S: Into<String>>(&mut self, class: S) -> &mut Self {
        self.main_class = Some(class.into());
        self
    }

    pub fn add_cp(&mut self, path: PathBuf) -> &mut Self {
        self.classpath.push(path);
        self
    }

    pub fn add_cps<I: IntoIterator<Item = PathBuf>>(&mut self, iter: I) -> &mut Self {
        self.classpath.extend(iter);
        self
    }

    pub fn set_args(&mut self, map: HashMap<String, String>) -> &mut Self {
        self.game_args = map;
        self
    }

    pub fn add_arg<K, V>(&mut self, key: K, val: V) -> &mut Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        self.game_args.insert(key.into(), val.into());
        self
    }

    pub fn rm_arg<K: AsRef<str>>(&mut self, key: K) -> Option<String> {
        self.game_args.remove(key.as_ref())
    }

    pub fn set_arg_value<P>(&mut self, key: ArgType, path: P) -> &mut Self
    where
        P: AsRef<Path>,
    {
        let placeholder = ArgType::get_value_placeholder(key);

        self.game_args
            .insert(placeholder, path.as_ref().display().to_string());
        self
    }

    pub fn add_natives(&mut self, natives_paths: Vec<PathBuf>) -> &mut Self {
        self.native_libs = natives_paths;
        self
    }

    pub fn set_natives_dir(&mut self, dir: PathBuf) -> &mut Self {
        self.natives_dir = Some(dir);
        self
    }

    pub fn set_memory(&mut self, min: i64, max: i64) -> &mut Self {
        self.memory_min = Some(min);
        self.memory_max = Some(max);
        self
    }

    pub fn set_jvm_args(&mut self, args: Vec<String>) -> &mut Self {
        self.jvm_args = args;
        self
    }

    /// Adds the version to `game_args` and assigns
    /// a value to an additional parameter for launching the game
    pub fn add_version<P>(&mut self, version: P) -> &mut Self
    where
        P: AsRef<Path>,
    {
        Self::set_arg_value(self, ArgType::Version, &version);
        self.version = Some(version.as_ref().display().to_string());
        self
    }

    /// Fills the `game_args` map
    /// with default values. Useful for old versions
    /// that requires some values to launch
    pub fn fill_defaults(mut self) -> Self {
        for arg_type in ArgType::iter() {
            self.game_args
                .entry(ArgType::get_value_placeholder(arg_type.clone()))
                .or_insert(ArgType::get_default_value(arg_type));
        }

        self
    }

    pub fn set_preferred_java_runtime(mut self, java_runtime: JavaRuntime) -> Self {
        self.preferred_java = Some(java_runtime);
        self
    }

    /// Build the LaunchInfo structure
    pub fn build(self) -> Result<LaunchInfo, LaunchError> {
        let classpath: OsString = env::join_paths(&self.classpath)
            .map_err(|e| LaunchError::ClasspathSeparator(e.to_string()))?;

        let classpath = classpath
            .into_string()
            .map_err(|_| LaunchError::ClasspathNotUnicode)?;

        let java_bin_path = self.preferred_java.ok_or(LaunchError::JavaRuntimeMissing)?;
        let manifest = self.manifest.ok_or(LaunchError::ManifestMissing)?;

        let version = match self.version {
            Some(v) => v,
            None => String::from("Version is not retrieved"),
        };

        let natives_dir = self.natives_dir.unwrap_or_default();

        Ok(LaunchInfo {
            manifest,
            classpath,
            native_libs: self.native_libs,
            natives_dir,
            main_class: self.main_class,
            game_args: self.game_args,
            memory_min: self.memory_min.unwrap_or(DEFAULT_MEMORY_MIN),
            memory_max: self.memory_max.unwrap_or(DEFAULT_MEMORY_MAX),
            jvm_args: self.jvm_args,
            java_bin_path,
            version,
        })
    }
}
