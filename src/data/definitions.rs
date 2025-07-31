pub enum EnvVars {
    HomeDirOverride
}

impl EnvVars {
    pub fn as_str(&self) -> &'static str {
        match self {
            EnvVars::HomeDirOverride => "HOME"
        }
    }
}
