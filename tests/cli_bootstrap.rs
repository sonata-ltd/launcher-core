#[test]
fn help_and_invalid_arguments_do_not_create_a_database_or_load_dotenv() {
    for (args, code) in [
        (vec![], 0),
        (vec!["--help"], 0),
        (vec!["-h"], 0),
        (vec!["help"], 0),
        (vec!["unknown"], 2),
        (vec!["install", "invalid"], 2),
        (
            vec![
                "create",
                "name",
                "version",
                "loader",
                "https://example.invalid",
                "prism",
            ],
            2,
        ),
        (
            vec![
                "create-modded",
                "name",
                "1.20.1",
                "unknown",
                "1",
                "https://example.invalid",
                "prism",
            ],
            2,
        ),
        (
            vec![
                "register-cleanroom-url",
                "1",
                "https://example.invalid",
                "8,broken",
            ],
            2,
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("must-not-exist.db");
        // If run reaches config/DB initialization this would create the file.
        std::fs::write(
            directory.path().join(".env"),
            format!("DATABASE_URL=sqlite://{}\n", database.display()),
        )
        .unwrap();
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_sonata_launcher_core"))
            .args(&args)
            .current_dir(directory.path())
            .env_remove("DATABASE_URL")
            .env("HOME", directory.path())
            .env("USERPROFILE", directory.path())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "args={args:?}, stdout={}, stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(text.contains("cli usage:"));
        assert!(!text.contains("opening database"));
        assert!(!database.exists());
        assert_eq!(
            std::fs::read_dir(directory.path()).unwrap().count(),
            1,
            "bootstrap touched files for {args:?}"
        );
    }
}
