use std::path::Path;

#[deprecated(
    note = "use a checked relative path for file destinations, or MavenCoordinate for coordinates"
)]
pub fn build_file_path<S, P>(libs_dir: &P, maven_path: S) -> String
where
    S: Into<String>,
    P: AsRef<Path>,
{
    let maven_path = maven_path.into();

    if maven_path.chars().nth(0) == Some('/') {
        return format!("{}{}", libs_dir.as_ref().display(), maven_path);
    } else {
        return format!("{}/{}", libs_dir.as_ref().display(), maven_path);
    }
}

#[cfg(test)]
#[allow(deprecated)]
mod tests {
    use std::path::PathBuf;

    use crate::utils::maven::build_file_path;

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
}
