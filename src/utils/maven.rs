use std::path::Path;

use regex::Regex;

/// Converts a coordinate like `group:artifact:...` into a path:
/// - dots in the `group` are replaced with `/`
/// - the segment recognized as a version (e.g. `1.2.3`, `2023.05.01`, `1.0.0-beta+build`)
///   is left intact (keeps its dots)
/// - other segments are appended as-is separated by `/`
///
/// Examples:
/// - "com.mojang:minecraft:1.20.1:client" -> "com/mojang/minecraft/1.20.1/client"
/// - "org.example:lib:jar:2.3.4" -> "org/example/lib/jar/2.3.4"
#[allow(dead_code)]
pub fn coord_to_path(coord: &str) -> String {
    let parts: Vec<&str> = coord.split(':').collect();
    if parts.is_empty() {
        return String::new();
    }

    // Regex to detect a version:
    // - starts with a digit
    // - may contain dot-separated numbers like 1.2.3
    // - allows suffixes via '-', '+', '_' or '.' and additional sections (RC, beta, SNAPSHOT, build, etc.)
    // Examples matched: "1", "1.2", "1.2.3", "1.0.0-SNAPSHOT", "1.0.0-beta+build.1"
    let ver_re =
        Regex::new(r"^\d+(?:\.\d+)*(?:[-+_\.][A-Za-z0-9]+(?:[.\-+_][A-Za-z0-9]+)*)?$").unwrap();

    // Find the first segment after group that looks like a version
    let mut version_idx: Option<usize> = None;
    for (i, p) in parts.iter().enumerate().skip(1) {
        if ver_re.is_match(p) {
            version_idx = Some(i);
            break;
        }
    }

    // Start output with transformed group (dots -> slashes)
    let mut out = parts[0].replace('.', "/");

    if let Some(vidx) = version_idx {
        // Append intermediate parts (artifact, packaging, etc.) before the version
        for p in &parts[1..vidx] {
            out.push('/');
            out.push_str(p);
        }
        // Append the version unchanged
        out.push('/');
        out.push_str(parts[vidx]);
        // Append anything after the version (classifier, etc.)
        for p in &parts[vidx + 1..] {
            out.push('/');
            out.push_str(p);
        }
    } else {
        // No version found — just join everything with '/', replacing dots only in group
        for p in parts.iter().skip(1) {
            out.push('/');
            out.push_str(p);
        }
    }

    out
}

pub fn build_file_path<S, P>(libs_dir: &P, maven_path: S) -> String
where
    S: Into<String>,
    P: AsRef<Path>
{
    let maven_path = maven_path.into();

    if maven_path.chars().nth(0) == Some('/') {
        return format!("{}{}", libs_dir.as_ref().display(), maven_path);
    } else {
        return format!("{}/{}", libs_dir.as_ref().display(), maven_path);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::utils::maven::build_file_path;

    use super::coord_to_path;

    #[test]
    fn no_first_slash() {
        let maven_path = "com/google/guava/guava/15.0/guava-15.0.jar";
        let libs_dir = PathBuf::from("/Users/quartix/.sonata/libraries");
        assert_eq!(
            build_file_path(&libs_dir, maven_path),
            String::from(
                "/Users/quartix/.sonata/libraries/com/google/guava/guava/15.0/guava-15.0.jar"
            )
        )
    }

    #[test]
    fn with_first_slash() {
        let maven_path = "/com/google/guava/guava/15.0/guava-15.0.jar";
        let libs_dir = PathBuf::from("/Users/quartix/.sonata/libraries");
        assert_eq!(
            build_file_path(&libs_dir, maven_path),
            String::from(
                "/Users/quartix/.sonata/libraries/com/google/guava/guava/15.0/guava-15.0.jar"
            )
        )
    }

    #[test]
    fn simple_three_parts() {
        let s = "com.mojang:minecraft:1.20.1:client";
        assert_eq!(coord_to_path(s), "com/mojang/minecraft/1.20.1/client");
    }

    #[test]
    fn artifact_and_version() {
        let s = "org.example:cool-lib:2.3.4";
        assert_eq!(coord_to_path(s), "org/example/cool-lib/2.3.4");
    }

    #[test]
    fn packaging_present() {
        // format: group:artifact:packaging:version
        let s = "org.example:cool-lib:jar:2.3.4";
        assert_eq!(coord_to_path(s), "org/example/cool-lib/jar/2.3.4");
    }

    #[test]
    fn classifier_present() {
        // format: group:artifact:version:classifier
        let s = "com.acme:tools:1.0.0:linux-x64";
        assert_eq!(coord_to_path(s), "com/acme/tools/1.0.0/linux-x64");
    }

    #[test]
    fn complex_version_with_plus_and_dots() {
        let s = "com.example:lib:1.0.0-beta+exp.sha.5114f85";
        assert_eq!(
            coord_to_path(s),
            "com/example/lib/1.0.0-beta+exp.sha.5114f85"
        );
    }

    #[test]
    fn no_version_found_fallback() {
        // if a clear version is not present (e.g. SNAPSHOT without digits), just concatenate
        let s = "some.group:artifact:SNAPSHOT";
        assert_eq!(coord_to_path(s), "some/group/artifact/SNAPSHOT");
    }

    #[test]
    fn only_group() {
        let s = "single.segment";
        assert_eq!(coord_to_path(s), "single/segment");
    }

    #[test]
    fn two_parts() {
        let s = "group:artifact";
        assert_eq!(coord_to_path(s), "group/artifact");
    }
}
