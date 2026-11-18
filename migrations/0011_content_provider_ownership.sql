-- Older clients could associate an instance with another object's provenance.
-- Keep the installed bytes/path, but do not retain a false provider identity.
UPDATE instance_content
SET preferred_provider_file_id = NULL
WHERE preferred_provider_file_id IS NOT NULL
  AND NOT EXISTS (
      SELECT 1 FROM content_provider_files p
      WHERE p.id = instance_content.preferred_provider_file_id
        AND p.object_id = instance_content.object_id
  );

CREATE TRIGGER instance_content_provider_insert
BEFORE INSERT ON instance_content
WHEN NEW.preferred_provider_file_id IS NOT NULL
 AND NOT EXISTS (SELECT 1 FROM content_provider_files
                 WHERE id = NEW.preferred_provider_file_id AND object_id = NEW.object_id)
BEGIN
    SELECT RAISE(ABORT, 'preferred provider must belong to content object');
END;

CREATE TRIGGER instance_content_provider_update
BEFORE UPDATE OF object_id, preferred_provider_file_id ON instance_content
WHEN NEW.preferred_provider_file_id IS NOT NULL
 AND NOT EXISTS (SELECT 1 FROM content_provider_files
                 WHERE id = NEW.preferred_provider_file_id AND object_id = NEW.object_id)
BEGIN
    SELECT RAISE(ABORT, 'preferred provider must belong to content object');
END;

CREATE TRIGGER content_provider_object_update
BEFORE UPDATE OF object_id ON content_provider_files
WHEN EXISTS (SELECT 1 FROM instance_content
             WHERE preferred_provider_file_id = OLD.id
               AND (object_id IS NULL OR object_id != NEW.object_id))
BEGIN
    SELECT RAISE(ABORT, 'provider object is used by instance content');
END;
