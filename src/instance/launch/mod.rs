use serde::Deserialize;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
use strum::IntoEnumIterator;

use crate::instance::launch::args::ArgType;

pub mod args;
pub mod execute;
pub mod natives;

#[derive(Deserialize, Debug)]
pub struct ClientOptions {
    pub classpath: Vec<String>,
    pub main_class: String,
    pub game_args: HashMap<String, String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LaunchInfo {
    classpath: String,
    native_libs: Vec<PathBuf>,
    main_class: Option<String>,
    game_args: HashMap<String, String>,
}

#[derive(Debug, Default)]
pub struct LaunchInfoBuilder {
    classpath: Vec<String>,
    native_libs: Vec<PathBuf>,
    main_class: Option<String>,
    game_args: HashMap<String, String>,
}

impl LaunchInfoBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_main_class<S: Into<String>>(&mut self, class: S) -> &mut Self {
        self.main_class = Some(class.into());
        self
    }

    pub fn add_cp<P: AsRef<Path>>(&mut self, path: P) -> &mut Self {
        self.classpath.push(path.as_ref().display().to_string());
        self
    }

    pub fn add_cps<P, I>(&mut self, iter: I) -> &mut Self
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        for cp in iter {
            self.classpath.push(cp.as_ref().display().to_string());
        }
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

    /// Fills the `game_args` map
    /// with default values. Useful for old versions
    /// that requires some values to launch
    pub fn fill_defauls(mut self) -> Self {
        for arg_type in ArgType::iter() {
            self.game_args
                .entry(ArgType::get_value_placeholder(arg_type.clone()))
                .or_insert(ArgType::get_default_value(arg_type));
        }

        self
    }

    /// Build the LaunchInfo structure
    pub fn build(self) -> LaunchInfo {
        let mut classpath = String::new();
        for path in self.classpath {
            classpath.push_str(&path);
        }

        LaunchInfo {
            classpath,
            native_libs: self.native_libs,
            main_class: self.main_class,
            game_args: self.game_args,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_default_empty() {
        let lib = LaunchInfoBuilder::new().build();
        assert!(
            lib.classpath.is_empty(),
            "classpath should be empty by default"
        );
        assert!(
            lib.game_args.is_empty(),
            "game_args should be empty by default"
        );
        assert_eq!(lib.main_class, None, "classpath should be None by default");
    }
}
