use sha1_smol::Sha1;

#[derive(Default)]
pub struct UuidData {
    name: String,
    version: String
}

impl UuidData {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_name<S: Into<String>>(&mut self, name: S) -> &mut Self {
        self.name = name.into();
        self
    }

    pub fn add_version<S: Into<String>>(&mut self, version: S) -> &mut Self {
        self.version = version.into();
        self
    }

    pub fn gen(&mut self) -> String {
        let string = format!("{}_{}", self.name, self.version);

        let mut hasher = Sha1::new();
        hasher.update(string.as_bytes());
        return hasher.digest().to_string();
    }
}
