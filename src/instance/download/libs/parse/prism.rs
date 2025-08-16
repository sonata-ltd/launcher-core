use std::path::PathBuf;

use serde_json::Value;

use crate::{
    instance::download::{libs::{LibInfo, SyncResult}, manifest::download_manifest},
    utils::str_nth_occurrence,
};

use super::*;

const META_BASE_URL: &'static str = "https://meta.prismlauncher.org/v1/";

impl<'a, 'b> LibsData<'a, 'b> {
    pub async fn parse_manifest_prism(self) -> Result<SyncResult, String> {
        let mut downloadable_libs: Vec<LibInfo> = Vec::new();

        if let Some(libs) = self.manifest.get("libraries").and_then(|v| v.as_array()) {
            for lib in libs {
                // Parse the name at the first
                if let Some(name) = lib.get("name").and_then(|v| v.as_str()) {
                    if let Some(downloads_val) = lib.get("downloads") {
                        // try artifact first (but don't index with ["artifact"] directly)
                        if let Some(artifact) = downloads_val.get("artifact") {
                            if let (Some(url), Some(sha1)) = (
                                artifact.get("url").and_then(|v| v.as_str()),
                                artifact.get("sha1").and_then(|v| v.as_str()),
                            ) {
                                if let Some(path) = get_path_from_url(url) {
                                    downloadable_libs.push(LibInfo {
                                        hash: sha1.to_string(),
                                        name: name.to_string(),
                                        path: path.to_string(),
                                        url: url.to_string(),
                                        native: false,
                                        save_path: None
                                    });
                                }
                            }
                        }


                        // If artifact not present or incomplete, check rules & downloads.classifiers under lib
                        let natives_key = format!("natives-{}", self.current_os);
                        if let Some(rules) = lib.get("rules").and_then(|v| v.as_array()) {
                            if rules_applies(rules, self.current_os) {
                                if let Some(native_obj) = downloads_val
                                    .get("classifiers")
                                    .and_then(|c| c.get(&natives_key))
                                {
                                    if let (Some(sha1), Some(url)) = (
                                        native_obj.get("sha1").and_then(|v| v.as_str()),
                                        native_obj.get("url").and_then(|v| v.as_str()),
                                    ) {
                                        if let Some(path) = get_path_from_url(url) {
                                            downloadable_libs.push(LibInfo {
                                                hash: sha1.to_string(),
                                                name: name.to_string(),
                                                path: path.to_string(),
                                                url: url.to_string(),
                                                native: true,
                                                save_path: None
                                            });
                                        }
                                    }
                                }
                            }
                        } else {
                            // No rules present, but we should check natives anyway
                            if let Some(classifiers) = downloads_val.get("classifiers") {
                                if let Some(native_val) = classifiers.get(&natives_key) {
                                    if let (Some(url), Some(sha1)) = (
                                        native_val.get("url").and_then(|v| v.as_str()),
                                        native_val.get("sha1").and_then(|v| v.as_str()),
                                    ) {
                                        if let Some(path) = get_path_from_url(url) {
                                            downloadable_libs.push(LibInfo {
                                                hash: sha1.to_string(),
                                                name: name.to_string(),
                                                path: path.to_string(),
                                                url: url.to_string(),
                                                native: true,
                                                save_path: None
                                            });
                                        }
                                    }
                                } // else: classifiers exist but not this native_key -> ignore
                            } // else: no classifiers -> nothing to do
                        }
                    } // else no downloads field -> ignore this lib
                } // else no name -> ignore this lib
            }
        }

        // Check for modern builded libs
        let mut additional_classpaths: Vec<String> = Vec::new();
        let mut additional_natives_paths: Vec<PathBuf> = Vec::new();
        if let Some(requires) = self.manifest.get("requires").and_then(|v| v.as_array()) {
            for req in requires {
                if let (Some(suggests), Some(uid)) = (
                    req.get("suggests").and_then(|v| v.as_str()),
                    req.get("uid").and_then(|v| v.as_str()),
                ) {
                    // Parse another page
                    let url = format!("{}{}/{}.json", META_BASE_URL, uid, suggests);
                    let manifest = match download_manifest::<String>(&url, None).await {
                        Ok(data) => data.0,
                        Err(e) => return Err(e.to_string()),
                    };

                    let libs_data = LibsData {
                        manifest: &manifest,
                        paths: self.paths,
                        ws_status: self.ws_status.clone(),
                        current_os: self.current_os,
                    };

                    match Box::pin(LibsData::parse_manifest_prism(libs_data)).await {
                        Ok(mut result) => {
                            // Insert delimiter
                            if !result.classpaths.is_empty() {
                                result.classpaths[0].insert(0, ':');
                                additional_classpaths.append(&mut result.classpaths);
                            }

                            if !result.natives.is_empty() {
                                additional_natives_paths.append(&mut result.natives);
                            }
                        }
                        Err(e) => return Err(e),
                    };
                }
            }
        }

        // Check for client jar
        if let Some(main_jar) = self.manifest.get("mainJar").and_then(|v| v.as_object()) {
            if let (Some(jar_name), Some(hash), Some(url), Some(version_name)) = (
                main_jar.get("name").and_then(|v| v.as_str()),
                main_jar
                    .get("downloads")
                    .and_then(|d| d.get("artifact"))
                    .and_then(|a| a.get("sha1"))
                    .and_then(|v| v.as_str()),
                main_jar
                    .get("downloads")
                    .and_then(|d| d.get("artifact"))
                    .and_then(|a| a.get("url"))
                    .and_then(|v| v.as_str()),
                self.manifest.get("version").and_then(|v| v.as_str()),
            ) {
                let file_name = version_name.to_owned() + "-client.jar";
                let path = "/com/mojang/minecraft/".to_owned() + &file_name;

                downloadable_libs.push(LibInfo {
                    hash: hash.to_string(),
                    name: jar_name.to_string(),
                    path,
                    url: url.to_string(),
                    native: false,
                    save_path: None
                });
            }
        }

        match Self::download_missing_libs(downloadable_libs, self.paths, self.ws_status).await {
            Ok(mut result) => {
                result.classpaths.append(&mut additional_classpaths);
                result.natives.append(&mut additional_natives_paths);
                Ok(result)
            }
            Err(e) => Err(e.to_string()),
        }
    }
}

fn rules_applies(rules: &Vec<Value>, current_os: &str) -> bool {
    let mut allow_lib = false;

    for rule in rules {
        let os_obj = match rule.get("os") {
            Some(os) => os,
            None => {
                if let Some(action) = rule.get("action").and_then(|v| v.as_str()) {
                    if action == "allow" {
                        allow_lib = true;
                    }
                }

                continue;
            }
        };

        if let Some(os_name) = os_obj.get("name").and_then(|v| v.as_str()) {
            if os_name == current_os {
                if let Some(action) = rule.get("action").and_then(|v| v.as_str()) {
                    if action == "allow" {
                        return true;
                    } else {
                        return false;
                    }
                }
            }
        }
    }

    allow_lib
}

fn get_path_from_url(url: &str) -> Option<&str> {
    let third_slash_pos = match str_nth_occurrence(url, '/', 3) {
        Some(pos) => pos,
        None => return None,
    };

    Some(&url[third_slash_pos..])
}
