pub mod download;
pub mod instance_manifest;
pub mod instances_list;
pub mod metacache;


pub fn extract_filename(path: &str) -> Option<&str> {
    let last_slash = path.rfind('/')?;
    let last_dot = path.rfind('.')?;

    if last_slash < last_dot {
        Some(&path[last_slash + 1..last_dot])
    } else {
        None
    }
}
