ALTER TABLE java_runtimes ADD COLUMN major INTEGER NOT NULL DEFAULT 0;
ALTER TABLE java_runtimes ADD COLUMN arch TEXT;
ALTER TABLE java_runtimes ADD COLUMN source TEXT NOT NULL DEFAULT 'manual';

CREATE INDEX IF NOT EXISTS java_runtimes_home ON java_runtimes(home_path);

CREATE TABLE IF NOT EXISTS launcher_state(
	key TEXT PRIMARY KEY,
	value TEXT NOT NULL
);
