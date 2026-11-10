ALTER TABLE instances ADD COLUMN loader_version TEXT;

CREATE TABLE instance_installations (
    instance_id INTEGER PRIMARY KEY REFERENCES instances(id) ON DELETE CASCADE,
    profile_json TEXT NOT NULL,
    installed_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TRIGGER instances_loader_insert
BEFORE INSERT ON instances
WHEN NEW.loader NOT IN ('vanilla', 'fabric', 'forge', 'neoforge', 'quilt', 'cleanroom')
  OR (NEW.loader = 'vanilla' AND NEW.loader_version IS NOT NULL)
  OR (NEW.loader <> 'vanilla' AND
      (NEW.loader_version IS NULL OR length(trim(NEW.loader_version)) = 0))
BEGIN
    SELECT RAISE(ABORT, 'invalid loader/version');
END;

CREATE TRIGGER instances_loader_update
BEFORE UPDATE OF loader, loader_version ON instances
WHEN NEW.loader NOT IN ('vanilla', 'fabric', 'forge', 'neoforge', 'quilt', 'cleanroom')
  OR (NEW.loader = 'vanilla' AND NEW.loader_version IS NOT NULL)
  OR (NEW.loader <> 'vanilla' AND
      (NEW.loader_version IS NULL OR length(trim(NEW.loader_version)) = 0))
BEGIN
    SELECT RAISE(ABORT, 'invalid loader/version');
END;
