use crate::java::{model::JavaRuntime, policy::JavaPolicy, version::sort_key};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JavaRequirement {
    pub majors: Vec<u32>,
}

impl JavaRequirement {
    pub fn from_manifest(manifest: &serde_json::Value) -> Self {
        Self {
            majors: crate::version::provider::java_majors(manifest),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    Exact,
    Newer,
}

pub fn normalize_arch(arch: &str) -> &str {
    crate::utils::platform::Arch::parse(arch).name()
}

pub fn host_arch() -> &'static str {
    crate::utils::platform::HostPlatform::current().arch.name()
}

pub(crate) fn arch_ok(java: &JavaRuntime, host: &str) -> bool {
    java.arch
        .as_deref()
        .is_none_or(|arch| normalize_arch(arch) == host)
}

pub fn pick<'a, J: AsRef<JavaRuntime>>(
    runtimes: &'a [J],
    req: &JavaRequirement,
    host: &str,
) -> Option<(&'a JavaRuntime, Fit)> {
    if req.majors.is_empty() {
        return usable_runtimes(runtimes, host)
            .max_by_key(|java| (java.major, sort_key(&java.version)))
            .map(|java| (java, Fit::Newer));
    }
    let allow_newer = req.majors.iter().any(|&major| major > 8);
    pick_compatible(runtimes, &req.majors, allow_newer, host)
}

pub fn pick_with_policy<'a, J: AsRef<JavaRuntime>>(
    runtimes: &'a [J],
    policy: &JavaPolicy,
    host: &str,
) -> Option<(&'a JavaRuntime, Fit)> {
    pick_compatible(runtimes, &policy.majors, policy.allow_newer, host)
}

fn usable_runtimes<'a, 'host, J: AsRef<JavaRuntime>>(
    runtimes: &'a [J],
    host: &'host str,
) -> impl Iterator<Item = &'a JavaRuntime> + use<'a, 'host, J> {
    let host = normalize_arch(host);
    runtimes
        .iter()
        .map(AsRef::as_ref)
        .filter(move |java| java.major > 0 && arch_ok(java, host))
}

fn pick_compatible<'a, J: AsRef<JavaRuntime>>(
    runtimes: &'a [J],
    majors: &[u32],
    allow_newer: bool,
    host: &str,
) -> Option<(&'a JavaRuntime, Fit)> {
    let usable: Vec<_> = usable_runtimes(runtimes, host).collect();
    let newest_of = |major| {
        usable
            .iter()
            .copied()
            .filter(|java| java.major == major)
            .max_by_key(|java| sort_key(&java.version))
    };
    for &major in majors {
        if let Some(java) = newest_of(major) {
            return Some((java, Fit::Exact));
        }
    }
    if !allow_newer {
        return None;
    }
    let maximum = *majors.iter().max()?;
    let nearest = usable
        .iter()
        .map(|java| java.major)
        .filter(|major| *major > maximum)
        .min()?;
    newest_of(nearest).map(|java| (java, Fit::Newer))
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
    fn selector_matches_explicit_newer_policy_and_returns_the_usable_runtime() {
        let runtimes = [
            java(1, "17.0.1", Some("arm64")),
            amd64(1, "17.0.1"),
            amd64(2, "21.0.1"),
        ];
        let policy = JavaPolicy {
            majors: vec![8],
            allow_newer: true,
        };
        let (selected, fit) = pick_with_policy(&runtimes, &policy, HOST).unwrap();
        assert_eq!(fit, Fit::Newer);
        assert!(policy.accepts(selected.major));
        assert!(std::ptr::eq(selected, &runtimes[1]));
        assert!(pick_with_policy(
            &runtimes,
            &JavaPolicy {
                allow_newer: false,
                ..policy
            },
            HOST
        )
        .is_none());
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
