pub enum ArgType {
    Username,
    Version,
    GameDir,
    AssetsDir,
    AssetIndex,
    AuthUuid,
    AccessToken,
    ClientId,
    XUid,
    UserType,
    VersionType,
}

impl<'a> ArgType {
    // Retrieve a key of the value in terms of manifest
    // e.g. "--username" - key | "${auth_player_name}" - placeholder value
    // so retrieves "${auth_player_name}" by ArgType::Username
    pub fn get_value_placeholder(self) -> String {
        Self::format_placeholder_value(match self {
            ArgType::Username => "auth_player_name",
            ArgType::Version => "version_name",
            ArgType::GameDir => "game_directory",
            ArgType::AssetsDir => "assets_root",
            ArgType::AssetIndex => "assets_index_name",
            ArgType::AuthUuid => "auth_uuid",
            ArgType::AccessToken => "auth_access_token",
            ArgType::ClientId => "clientid",
            ArgType::XUid => "auth_xuid",
            ArgType::UserType => "user_type",
            ArgType::VersionType => "version_type",
        })
    }

    fn format_placeholder_value(val: &'a str) -> String {
        String::from("${".to_owned() + val + "}")
    }

    // Retrieve a key of the value in terms of manifest
    // e.g. "--username" - key | "${auth_player_name}" - placeholder value
    // so retrieves "--username" by ArgType::Username
    pub fn get_manifest_key(self) -> String {
        Self::format_key(match self {
            ArgType::Username => "username",
            ArgType::Version => "version",
            ArgType::GameDir => "gameDir",
            ArgType::AssetsDir => "assetsDir",
            ArgType::AssetIndex => "assetIndex",
            ArgType::AuthUuid => "uuid",
            ArgType::AccessToken => "accessToken",
            ArgType::ClientId => "clientId",
            ArgType::XUid => "xuid",
            ArgType::UserType => "userType",
            ArgType::VersionType => "versionType",
        })
    }

    fn format_key(key: &'a str) -> String {
        String::from("--".to_owned() + key)
    }
}
