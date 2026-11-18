use serde::{Deserialize, Serialize};

pub mod service;
pub(crate) mod storage;

pub const DEFAULT_MEMORY_MIN: i64 = 512;
pub const DEFAULT_MEMORY_MAX: i64 = 4096;

#[derive(Debug, Clone, Default, Serialize)]
pub struct InstanceSettings {
    pub java_runtime: Option<i64>,
    pub memory_min: Option<i64>,
    pub memory_max: Option<i64>,
    pub jvm_args: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GlobalSettings {
    pub java_runtime: Option<i64>,
    pub memory_min: i64,
    pub memory_max: i64,
    pub jvm_args: String,
}

impl Default for GlobalSettings {
    fn default() -> Self {
        Self {
            java_runtime: None,
            memory_min: DEFAULT_MEMORY_MIN,
            memory_max: DEFAULT_MEMORY_MAX,
            jvm_args: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EffectiveSettings {
    pub java_runtime: Option<i64>,
    pub memory_min: i64,
    pub memory_max: i64,
    pub jvm_args: Vec<String>,
}

impl EffectiveSettings {
    /// Resolves a display snapshot, including legacy settings that need correction.
    pub fn resolve(global: &GlobalSettings, instance: &InstanceSettings) -> Self {
        let args = Self::args_source(global, instance);
        let jvm_args = shlex::split(args)
            .unwrap_or_else(|| args.split_whitespace().map(str::to_owned).collect());
        Self::from_parts(global, instance, jvm_args)
    }

    pub fn try_resolve(
        global: &GlobalSettings,
        instance: &InstanceSettings,
    ) -> super::Result<Self> {
        Self::validate_memory(global, instance)?;
        let jvm_args = Self::parse_jvm_args(global, instance)?;
        Ok(Self::from_parts(global, instance, jvm_args))
    }

    fn args_source<'a>(global: &'a GlobalSettings, instance: &'a InstanceSettings) -> &'a str {
        instance.jvm_args.as_deref().unwrap_or(&global.jvm_args)
    }

    fn from_parts(
        global: &GlobalSettings,
        instance: &InstanceSettings,
        jvm_args: Vec<String>,
    ) -> Self {
        Self {
            java_runtime: instance.java_runtime.or(global.java_runtime),
            memory_min: instance.memory_min.unwrap_or(global.memory_min),
            memory_max: instance.memory_max.unwrap_or(global.memory_max),
            jvm_args,
        }
    }

    fn validate_memory(global: &GlobalSettings, instance: &InstanceSettings) -> super::Result<()> {
        let min_mib = instance.memory_min.unwrap_or(global.memory_min);
        let max_mib = instance.memory_max.unwrap_or(global.memory_max);
        if min_mib <= 0 || max_mib < min_mib {
            return Err(super::error::InstanceError::InvalidMemoryRange { min_mib, max_mib });
        }
        Ok(())
    }

    fn parse_jvm_args(
        global: &GlobalSettings,
        instance: &InstanceSettings,
    ) -> super::Result<Vec<String>> {
        shlex::split(Self::args_source(global, instance))
            .ok_or(super::error::InstanceError::InvalidJvmArguments)
    }
}

#[serde_with::skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub java_runtime: Option<Option<i64>>,

    #[serde(default, with = "::serde_with::rust::double_option")]
    pub memory_min: Option<Option<i64>>,

    #[serde(default, with = "::serde_with::rust::double_option")]
    pub memory_max: Option<Option<i64>>,

    #[serde(default, with = "::serde_with::rust::double_option")]
    pub jvm_args: Option<Option<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn global() -> GlobalSettings {
        GlobalSettings {
            java_runtime: Some(7),
            memory_min: 1024,
            memory_max: 8192,
            jvm_args: "-XX:+UseG1GC -Dfoo=bar".to_string(),
        }
    }

    #[test]
    fn empty_overrides_inherit_everything() {
        let effective = EffectiveSettings::resolve(&global(), &InstanceSettings::default());

        assert_eq!(effective.java_runtime, Some(7));
        assert_eq!(effective.memory_min, 1024);
        assert_eq!(effective.memory_max, 8192);
        assert_eq!(effective.jvm_args, vec!["-XX:+UseG1GC", "-Dfoo=bar"]);
    }

    #[test]
    fn overrides_win_field_by_field() {
        let overrides = InstanceSettings {
            java_runtime: Some(9),
            memory_max: Some(2048),
            ..Default::default()
        };

        let effective = EffectiveSettings::resolve(&global(), &overrides);

        assert_eq!(effective.java_runtime, Some(9));
        assert_eq!(effective.memory_max, 2048);
        assert_eq!(effective.memory_min, 1024);
    }

    #[test]
    fn empty_string_override_is_not_inheritance() {
        let overrides = InstanceSettings {
            jvm_args: Some(String::new()),
            ..Default::default()
        };

        let effective = EffectiveSettings::resolve(&global(), &overrides);

        assert!(effective.jvm_args.is_empty());
    }

    #[test]
    fn global_without_runtime_leaves_none() {
        let global = GlobalSettings {
            java_runtime: None,
            ..global()
        };

        let effective = EffectiveSettings::resolve(&global, &InstanceSettings::default());

        assert_eq!(effective.java_runtime, None);
    }
}
