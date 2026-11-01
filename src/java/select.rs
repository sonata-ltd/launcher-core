//! Which registered runtime an instance should use. Pure: no I/O, no DB.

use crate::java::{model::JavaRuntime, version::sort_key};

/// What a Minecraft version asks of Java.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JavaRequirement {
    /// Acceptable majors, best first. Empty: the manifest did not say.
    pub majors: Vec<u32>,
}

impl JavaRequirement {
    /// Prism: `compatibleJavaMajors: [21]`; Mojang: `javaVersion.majorVersion: 21`.
    pub fn from_manifest(manifest: &serde_json::Value) -> Self {
        let prism = manifest
            .get("compatibleJavaMajors")
            .and_then(|v| v.as_array())
            .map(|majors| {
                majors
                    .iter()
                    .filter_map(|m| m.as_u64())
                    .filter_map(|m| u32::try_from(m).ok())
                    .collect::<Vec<_>>()
            })
            .filter(|majors| !majors.is_empty());

        let mojang = || {
            manifest
                .get("javaVersion")
                .and_then(|j| j.get("majorVersion"))
                .and_then(|m| m.as_u64())
                .and_then(|m| u32::try_from(m).ok())
                .map(|m| vec![m])
        };

        Self {
            majors: prism.or_else(mojang).unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// The major is one the version lists.
    Exact,
    /// Newer than every listed major: runs, but not what the version was built for.
    Newer,
}

/// `amd64`/`x86_64` → `x86_64`, `aarch64`/`arm64` → `aarch64`,
/// `x86`/`i386`/`i686` → `x86`, `arm`/`armv7l` → `arm`.
pub fn normalize_arch(arch: &str) -> &str {
    match arch {
        "amd64" | "x86_64" => "x86_64",
        "aarch64" | "arm64" => "aarch64",
        "x86" | "i386" | "i486" | "i586" | "i686" => "x86",
        "arm" | "armv7" | "armv7l" => "arm",
        other => other,
    }
}

/// The launcher's own architecture, spelled as `normalize_arch` spells it.
pub fn host_arch() -> &'static str {
    normalize_arch(std::env::consts::ARCH)
}

/// Unknown arch (rows added before it was recorded) counts as compatible.
fn arch_ok(java: &JavaRuntime, host: &str) -> bool {
    java.arch
        .as_deref()
        .is_none_or(|arch| normalize_arch(arch) == host)
}

/// The best runtime for `req`:
/// 1. a listed major: the newest build of the first listed major there is;
/// 2. otherwise the nearest newer major (its newest build), unless the version
///    needs Java 8: pre-1.17 versions and their modloaders break on 9+;
/// 3. otherwise nothing.
///
/// An unknown requirement gets the newest runtime there is, as `Newer`.
pub fn pick<'a, J: AsRef<JavaRuntime>>(
    runtimes: &'a [J],
    req: &JavaRequirement,
    host: &str,
) -> Option<(&'a JavaRuntime, Fit)> {
    let usable: Vec<&JavaRuntime> = runtimes
        .iter()
        .map(AsRef::as_ref)
        .filter(|j| j.major > 0 && arch_ok(j, host))
        .collect();

    let newest_of = |major: u32| {
        usable
            .iter()
            .copied()
            .filter(|j| j.major == major)
            .max_by(|a, b| sort_key(&a.version).cmp(&sort_key(&b.version)))
    };

    if req.majors.is_empty() {
        let top = usable.iter().map(|j| j.major).max()?;
        return newest_of(top).map(|j| (j, Fit::Newer));
    }

    for &major in &req.majors {
        if let Some(java) = newest_of(major) {
            return Some((java, Fit::Exact));
        }
    }

    let max_required = *req.majors.iter().max()?;
    if max_required <= 8 {
        return None;
    }

    let nearest = usable
        .iter()
        .map(|j| j.major)
        .filter(|&m| m > max_required)
        .min()?;

    newest_of(nearest).map(|j| (j, Fit::Newer))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::java::model::JavaSource;

    const HOST: &str = "x86_64";

    fn java(id: i64, version: &str, arch: Option<&str>) -> JavaRuntime {
        JavaRuntime {
            id,
            version: version.into(),
            major: crate::java::version::parse_major(version).unwrap(),
            arch: arch.map(Into::into),
            exec_path: format!("/j/{id}/bin/java"),
            home_path: format!("/j/{id}"),
            vendor: None,
            source: JavaSource::Discovered,
        }
    }

    fn amd64(id: i64, version: &str) -> JavaRuntime {
        java(id, version, Some("amd64"))
    }

    fn req(majors: &[u32]) -> JavaRequirement {
        JavaRequirement {
            majors: majors.to_vec(),
        }
    }

    fn picked(runtimes: &[JavaRuntime], r: &JavaRequirement) -> Option<(i64, Fit)> {
        pick(runtimes, r, HOST).map(|(j, fit)| (j.id, fit))
    }

    #[test]
    fn exact_wins() {
        let all = [amd64(1, "17.0.2"), amd64(2, "21.0.5")];
        assert_eq!(picked(&all, &req(&[21])), Some((2, Fit::Exact)));
    }

    #[test]
    fn newest_build_of_major() {
        let all = [amd64(1, "21.0.2+13"), amd64(2, "21.0.5+11")];
        assert_eq!(picked(&all, &req(&[21])), Some((2, Fit::Exact)));
    }

    #[test]
    fn nearest_newer() {
        let all = [amd64(1, "25"), amd64(2, "21.0.5")];
        assert_eq!(picked(&all, &req(&[17])), Some((2, Fit::Newer)));
    }

    #[test]
    fn java8_strict() {
        let all = [amd64(1, "17.0.2"), amd64(2, "21.0.5")];
        assert_eq!(picked(&all, &req(&[8])), None);
    }

    #[test]
    fn java8_found() {
        let all = [
            amd64(1, "1.8.0_66"),
            amd64(2, "1.8.0_402"),
            amd64(3, "21.0.5"),
        ];
        assert_eq!(picked(&all, &req(&[8])), Some((2, Fit::Exact)));
    }

    #[test]
    fn older_never() {
        let all = [amd64(1, "1.8.0_402"), amd64(2, "17.0.2")];
        assert_eq!(picked(&all, &req(&[21])), None);
    }

    #[test]
    fn foreign_arch_skipped() {
        let all = [java(1, "21.0.5", Some("x86"))];
        assert_eq!(picked(&all, &req(&[21])), None);
    }

    #[test]
    fn unknown_arch_allowed() {
        let all = [java(1, "21.0.5", None)];
        assert_eq!(picked(&all, &req(&[21])), Some((1, Fit::Exact)));
    }

    #[test]
    fn unknown_requirement_takes_newest() {
        let all = [
            amd64(1, "1.8.0_402"),
            amd64(2, "21.0.5"),
            amd64(3, "17.0.2"),
        ];
        assert_eq!(picked(&all, &req(&[])), Some((2, Fit::Newer)));
    }

    #[test]
    fn works_with_arcs() {
        let all = [std::sync::Arc::new(amd64(1, "21.0.5"))];
        assert!(pick(&all, &req(&[21]), HOST).is_some());
    }

    #[test]
    fn manifest_prism() {
        let m = json!({ "compatibleJavaMajors": [17] });
        assert_eq!(JavaRequirement::from_manifest(&m), req(&[17]));
    }

    #[test]
    fn manifest_mojang() {
        let m = json!({ "javaVersion": { "majorVersion": 21 } });
        assert_eq!(JavaRequirement::from_manifest(&m), req(&[21]));
    }

    #[test]
    fn manifest_without_java() {
        assert_eq!(JavaRequirement::from_manifest(&json!({})), req(&[]));
    }

    #[test]
    fn arch_spellings() {
        assert_eq!(normalize_arch("amd64"), "x86_64");
        assert_eq!(normalize_arch("arm64"), "aarch64");
        assert_eq!(normalize_arch("i386"), "x86");
    }
}
