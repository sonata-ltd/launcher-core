use thiserror::Error;

use std::path::PathBuf;

#[derive(Debug, Error)]
#[error("invalid Maven coordinate: {coordinate}")]
pub struct MavenCoordinateError {
    coordinate: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MavenCoordinate {
    pub group: String,
    pub artifact: String,
    pub version: String,
    pub classifier: Option<String>,
    pub extension: String,
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-+".contains(&c))
}

impl MavenCoordinate {
    pub fn parse(raw: &str) -> Result<Self, MavenCoordinateError> {
        let (coordinate, extension) = raw.split_once('@').unwrap_or((raw, "jar"));
        let parts: Vec<_> = coordinate.split(':').collect();

        if !(3..=4).contains(&parts.len())
            || !valid_segment(extension)
            || !parts.iter().all(|part| valid_segment(part))
            || !parts[0].split('.').all(valid_segment)
        {
            return Err(MavenCoordinateError {
                coordinate: raw.to_owned(),
            });
        }

        Ok(Self {
            group: parts[0].into(),
            artifact: parts[1].into(),
            version: parts[2].into(),
            classifier: parts.get(3).map(|s| (*s).to_owned()),
            extension: extension.into(),
        })
    }

    pub fn path(&self) -> PathBuf {
        let classifier = self
            .classifier
            .as_ref()
            .map(|s| format!("-{s}"))
            .unwrap_or_default();

        PathBuf::from(self.group.replace('.', "/"))
            .join(&self.artifact)
            .join(&self.version)
            .join(format!(
                "{}-{}{classifier}.{}",
                self.artifact, self.version, self.extension
            ))
    }

    pub fn conflict_key(&self) -> (String, String, Option<String>, String) {
        (
            self.group.clone(),
            self.artifact.clone(),
            self.classifier.clone(),
            self.extension.clone(),
        )
    }
}

#[cfg(test)]
mod tests2 {
    use super::*;

    #[test]
    fn classifier_and_extension_are_part_of_file_name() {
        let c = MavenCoordinate::parse("g.a:tool:1.0:client@zip").unwrap();
        assert_eq!(c.path(), PathBuf::from("g/a/tool/1.0/tool-1.0-client.zip"));
    }

    #[test]
    fn snapshot_is_not_guessed_from_digits() {
        let c = MavenCoordinate::parse("org.example:tool:SNAPSHOT").unwrap();
        assert_eq!(
            c.path(),
            PathBuf::from("org/example/tool/SNAPSHOT/tool-SNAPSHOT.jar")
        );
    }

    #[test]
    fn traversal_and_malformed_coordinates_are_rejected() {
        for raw in [
            "g:a:../x",
            "g:a:1:x:y",
            "g..x:a:1",
            "g:a:1@../zip",
            "g:a:1\\x",
        ] {
            assert!(MavenCoordinate::parse(raw).is_err(), "{raw}");
        }
    }
}
