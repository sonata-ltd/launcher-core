CREATE TABLE IF NOT EXISTS java_runtimes(
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    version TEXT NOT NULL,
    exec_path TEXT NOT NULL UNIQUE,
    home_path TEXT NOT NULL,
    vendor TEXT
);
