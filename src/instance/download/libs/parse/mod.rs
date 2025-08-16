use crate::instance::download::libs::{LibInfo, LibsData, SyncResult};

mod prism;

impl<'a, 'b> LibsData<'a, 'b> {
    pub async fn parse_manifest_official(self) -> Result<SyncResult, String> {
        // Hashmap contains: hash, (name, path, url)
        let mut downloadable_libs: Vec<LibInfo> = Vec::new();

        println!("Extraction libraries...");

        if let Some(libraries) = self.manifest["libraries"].as_array() {
            for lib in libraries {
                let lib_name = lib["name"].as_str();

                let allow_lib = if let Some(rules) = lib["rules"].as_array() {
                    rules.iter().any(|rule| {
                        if let Some(action) = rule["action"].as_str() {
                            if action == "allow" {
                                if let Some(os) = rule["os"].as_object() {
                                    if os.get("name").and_then(|name| name.as_str())
                                        == Some(self.current_os)
                                    {
                                        return true;
                                    }
                                } else {
                                    return true;
                                }
                            }
                        }

                        false
                    })
                } else {
                    true
                };

                if allow_lib {
                    let lib_path = lib["downloads"]["artifact"]["path"].as_str();
                    let lib_url = lib["downloads"]["artifact"]["url"].as_str();
                    let lib_hash = lib["downloads"]["artifact"]["sha1"].as_str();

                    if let (Some(lib_name), Some(lib_path), Some(lib_url), Some(lib_hash)) =
                        (lib_name, lib_path, lib_url, lib_hash)
                    {
                        downloadable_libs.push(LibInfo {
                            hash: lib_hash.to_string(),
                            name: lib_name.to_string(),
                            path: lib_path.to_string(),
                            url: lib_url.to_string(),
                            native: false,
                            save_path: None,
                        });
                    }
                }

                // Check for classifiers
                if let Some(natives) = lib["natives"].as_object() {
                    for (k, native_name) in natives {
                        if k == self.current_os {
                            if let Some(classifiers) = lib["downloads"]["classifiers"].as_object() {
                                for (name, v) in classifiers {
                                    if name == native_name {
                                        let lib_path = v["path"].as_str();
                                        let lib_url = v["url"].as_str();
                                        let lib_hash = v["sha1"].as_str();

                                        if let (
                                            Some(lib_name),
                                            Some(lib_path),
                                            Some(lib_url),
                                            Some(lib_hash),
                                        ) = (lib_name, lib_path, lib_url, lib_hash)
                                        {
                                            println!("Found: {}", lib_name);
                                            downloadable_libs.push(LibInfo {
                                                hash: lib_hash.to_string(),
                                                name: lib_name.to_string(),
                                                path: lib_path.to_string(),
                                                url: lib_url.to_string(),
                                                native: true,
                                                save_path: None,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                } else {
                }
            }
        }

        if let Some(client_url) = self.manifest["downloads"]["client"]["url"].as_str() {
            let name = self.manifest["id"].as_str().unwrap();
            let name = name.to_owned() + "-client.jar";
            let path = "com/mojang/minecraft/".to_owned() + &name;
            let hash = self.manifest["downloads"]["client"]["sha1"]
                .as_str()
                .unwrap();

            downloadable_libs.push(LibInfo {
                hash: hash.to_string(),
                name,
                path,
                url: client_url.to_string(),
                native: false,
                save_path: None,
            });
        }

        match Self::download_missing_libs(downloadable_libs, self.paths, self.ws_status).await {
            Ok(result) => Ok(result),
            Err(e) => Err(e),
        }
    }
}
