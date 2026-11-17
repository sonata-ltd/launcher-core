use serde::Deserialize;

#[derive(Deserialize)]
pub struct Search {
    pub hits: Vec<Hit>,
    pub total_hits: u64,
}

#[derive(Deserialize)]
pub struct Hit {
    pub project_id: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub downloads: u64,
}

#[derive(Deserialize)]
pub struct Version {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub version_type: String,
    pub date_published: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    pub environment: Option<String>,
    pub files: Vec<File>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
pub struct File {
    pub filename: String,
    pub url: String,
    pub size: u64,
    pub primary: bool,
    pub file_type: Option<String>,
    pub hashes: Hashes,
}

#[derive(Deserialize)]
pub struct Hashes {
    pub sha1: Option<String>,
    pub sha512: Option<String>,
}

#[derive(Deserialize)]
pub struct Dependency {
    pub project_id: Option<String>,
    pub version_id: Option<String>,
    pub dependency_type: String,
}
