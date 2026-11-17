use crate::utils::files;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UrlBuildError {
    #[error("invalid base URL: {0}")]
    InvalidBase(String),

    #[error("URL cannot contain path segments")]
    CannotBeBase,

    #[error("invalid URL path segment: {0}")]
    InvalidSegment(String),

    #[error("invalid relative path: {0}")]
    InvalidPath(String),
}

type Result<T> = std::result::Result<T, UrlBuildError>;

pub fn append_segments(base: &str, segments: &[&str]) -> Result<String> {
    let mut url =
        http_types::Url::parse(base).map_err(|_| UrlBuildError::InvalidBase(base.to_string()))?;

    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| UrlBuildError::CannotBeBase)?;
        path.pop_if_empty();

        for segment in segments {
            if matches!(*segment, "." | "..") || segment.contains(['/', '\\']) {
                return Err(UrlBuildError::InvalidSegment(segment.to_string()));
            }

            path.push(segment);
        }
    }

    Ok(url.to_string())
}

pub fn relative_url(base: &str, path: &Path) -> Result<String> {
    files::relative(path).map_err(|e| UrlBuildError::InvalidPath(e.to_string()))?;

    let parts: Vec<String> = path
        .components()
        .map(|p| p.as_os_str().to_string_lossy().into_owned())
        .collect();
    append_segments(base, &parts.iter().map(String::as_str).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_segments_without_replacing_base_path() {
        for base in ["https://example.com/v1", "https://example.com/v1/"] {
            assert_eq!(
                append_segments(base, &["net.minecraft", "1.20.1.json"]).unwrap(),
                "https://example.com/v1/net.minecraft/1.20.1.json"
            );
        }
    }

    #[test]
    fn appends_segments_to_host_root() {
        assert_eq!(
            append_segments("https://example.com", &["libraries", "client.jar"]).unwrap(),
            "https://example.com/libraries/client.jar"
        );
    }

    #[test]
    fn encodes_reserved_characters_as_path_data() {
        assert_eq!(
            append_segments("https://example.com/v1/", &["a b?#.json"]).unwrap(),
            "https://example.com/v1/a%20b%3F%23.json"
        );
    }

    #[test]
    fn preserves_query_and_fragment_when_appending_path() {
        assert_eq!(
            append_segments(
                "https://example.com/v1/?source=test#section",
                &["file.json"]
            )
            .unwrap(),
            "https://example.com/v1/file.json?source=test#section"
        );
    }

    #[test]
    fn rejects_dot_segments_and_path_separators() {
        for segment in [".", "..", "a/b", "a\\b", "/absolute"] {
            assert!(matches!(
                append_segments("https://example.com/v1/", &[segment]),
                Err(UrlBuildError::InvalidSegment(value)) if value == segment
            ));
        }
    }

    #[test]
    fn distinguishes_invalid_base_from_url_without_path_segments() {
        assert!(matches!(
            append_segments("not a URL", &["file"]),
            Err(UrlBuildError::InvalidBase(value)) if value == "not a URL"
        ));
        assert!(matches!(
            append_segments("mailto:user@example.com", &["file"]),
            Err(UrlBuildError::CannotBeBase)
        ));
    }

    #[test]
    fn builds_artifact_url_from_relative_filesystem_path() {
        let path = Path::new("org").join("example").join("a b.jar");
        assert_eq!(
            relative_url("https://example.com/maven/", &path).unwrap(),
            "https://example.com/maven/org/example/a%20b.jar"
        );
    }

    #[test]
    fn rejects_unsafe_relative_paths_before_building_url() {
        for path in ["", ".", "..", "../client.jar", "a/../client.jar", "a:b"] {
            assert!(
                matches!(
                    relative_url("https://example.com/maven/", Path::new(path)),
                    Err(UrlBuildError::InvalidPath(_))
                ),
                "accepted invalid path: {path:?}"
            );
        }
        let temp = tempfile::tempdir().unwrap();
        assert!(matches!(
            relative_url("https://example.com/maven/", temp.path()),
            Err(UrlBuildError::InvalidPath(_))
        ));
    }

    #[test]
    fn relative_url_propagates_invalid_base() {
        assert!(matches!(
            relative_url("not a URL", Path::new("client.jar")),
            Err(UrlBuildError::InvalidBase(_))
        ));
    }
}
