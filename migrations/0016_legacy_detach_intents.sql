CREATE TABLE legacy_detach_intents (
    binding_id INTEGER PRIMARY KEY REFERENCES instance_content(id) ON DELETE CASCADE,
    intent_json TEXT NOT NULL
);
