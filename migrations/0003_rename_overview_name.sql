-- Add migration script here
ALTER TABLE instances_overview
RENAME COLUMN "name" TO "changed_name"
