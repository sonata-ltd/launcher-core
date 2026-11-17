//! Общая проверка создаваемых имён: используется не только модами.
pub fn validate_file_name(name: &str) -> std::io::Result<()> {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if name.is_empty()
        || name.len() > 200
        || name == "."
        || name == ".."
        || name.ends_with([' ', '.'])
        || reserved.contains(&stem.as_str())
        || name
            .chars()
            .any(|value| value.is_control() || "<>:\"/\\|?*".contains(value))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Небезопасное имя файла провайдера",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsafe_names_never_reach_the_instance() {
        for name in [
            "../mod.jar",
            "C:mod.jar",
            "CON.jar",
            "a.jar ",
            "a.jar.",
            "mod\\a.jar",
            "LPT1",
            "a\n.jar",
        ] {
            assert!(validate_file_name(name).is_err(), "{name}");
        }
        assert!(validate_file_name("Пример mod-1.0.jar").is_ok());
    }
}
