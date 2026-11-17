-- Older registrations may lack provider metadata about their dependencies.
ALTER TABLE content_provider_files ADD COLUMN dependencies_json TEXT;
