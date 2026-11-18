-- The byte path is derived from the configured store root and SHA-256.
-- Never retain an absolute path as a second source of truth.
-- SQLite cannot drop a UNIQUE column without rebuilding all referring tables.
-- Retain a NULL-only compatibility column instead of weakening their FKs.
UPDATE content_objects SET store_path = NULL;

CREATE TRIGGER content_object_path_insert
BEFORE INSERT ON content_objects WHEN NEW.store_path IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'content store path is derived from object identity');
END;

CREATE TRIGGER content_object_path_update
BEFORE UPDATE OF store_path ON content_objects WHEN NEW.store_path IS NOT NULL
BEGIN
    SELECT RAISE(ABORT, 'content store path is derived from object identity');
END;
