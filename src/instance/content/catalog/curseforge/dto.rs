use serde::Deserialize;

#[derive(Deserialize)]
pub struct Response<T> {
    pub data: T,
}

#[derive(Deserialize)]
pub struct Search {
    pub data: Vec<Project>,
    pub pagination: Pagination,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pagination {
    pub total_count: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: u64,
    pub name: String,
    pub summary: String,
    pub authors: Vec<Author>,
    pub download_count: f64,
}

#[derive(Deserialize)]
pub struct Author {
    pub name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    pub id: u64,
    pub mod_id: u64,
    pub display_name: String,
    pub file_name: String,
    pub release_type: u32,
    pub is_available: bool,
    pub file_date: String,
    pub file_length: u64,
    pub download_url: Option<String>,
    pub game_versions: Vec<String>,
    pub hashes: Vec<Hash>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
pub struct Hash {
    pub value: String,
    pub algo: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dependency {
    pub mod_id: u64,
    pub relation_type: u32,
}
