use std::collections::HashMap;

use crate::instance::download::libs::LibsData;

impl<'a> LibsData<'a> {
    pub async fn extract_manifest_libs(
        self
    ) -> Result<Vec<String>, String> {
        // Hashmap contains: hash, (name, path, url)
        let mut version_libs: HashMap<&str, (String, String, &str)> = HashMap::new();

        println!("Extraction libraries...");

        if let Some(libraries) = self.manifest["libraries"].as_array() {
            for lib in libraries {
                let lib_name = lib["name"].as_str();

                let allow_lib = if let Some(rules) = lib["rules"].as_array() {
                    rules.iter().any(|rule| {
                        if let Some(action) = rule["action"].as_str() {
                            if action == "allow" {
                                if let Some(os) = rule["os"].as_object() {
                                    if os.get("name").and_then(|name| name.as_str()) == Some(self.current_os)
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
                        version_libs.insert(
                            lib_hash,
                            (lib_name.to_string(), lib_path.to_string(), lib_url),
                        );
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
                                            if let Some(updated) = version_libs.insert(
                                                lib_hash,
                                                (lib_name.to_string(), lib_path.to_string(), lib_url),
                                            ) {
                                                println!("REWRITED: {:#?}", updated);
                                            }
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
            let hash = self.manifest["downloads"]["client"]["sha1"].as_str().unwrap();

            version_libs.insert(hash, (name, path, client_url));
        }

        println!("{:#?}", version_libs);
        match Self::download_missing_libs(version_libs, self.paths, self.ws_status).await {
            Ok(paths) => Ok(paths),
            Err(e) => Err(e),
        }
    }
}
