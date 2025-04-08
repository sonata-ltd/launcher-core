use std::ffi::OsString;
use home::home_dir;


pub struct Config {
    pub launcher_root_dir: Option<OsString>,
}

impl Config {
    pub fn get_default_values() -> Result<Config, ()> {
        if let Some(path) = home_dir() {
            let mut path = path.into_os_string();
            path.push("/.sonata");

            return Ok(Config {
                launcher_root_dir: Some(path),
            })
        }

        return Err(());
    }
}
