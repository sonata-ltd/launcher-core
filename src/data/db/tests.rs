use super::*;

#[async_std::test]
async fn retention_upgrade_adds_stable_immutable_database_identity_without_touching_content() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for migration in sqlx::migrate!("./migrations")
        .iter()
        .filter(|m| m.version <= 14)
    {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO content_objects(sha256,size_bytes,store_state) VALUES(?,3,'ready')")
        .bind(format!("{:064x}", 1))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/0015_retention_identity.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let identity: String = sqlx::query_scalar("SELECT identity FROM retention_identity")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(crate::utils::files::hash_valid(&identity, 32));
    assert!(
        sqlx::query("UPDATE retention_identity SET identity=lower(hex(randomblob(16)))")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM retention_identity")
        .execute(&pool)
        .await
        .is_err());
    let unchanged: String = sqlx::query_scalar("SELECT identity FROM retention_identity")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(identity, unchanged);
    let state: String = sqlx::query_scalar("SELECT store_state FROM content_objects")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "ready");
}

#[async_std::test]
async fn preparation_upgrade_preserves_pending_journals_and_restricts_new_states() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for migration in sqlx::migrate!("./migrations")
        .iter()
        .filter(|m| m.version <= 13)
    {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'old','old','1.21','vanilla','https://example.invalid','prism')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO content_objects(id,sha256,size_bytes) VALUES(1,?,3)")
        .bind(format!("{:064x}", 1))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO content_operations(id,instance_id,state,recovery_json) VALUES('pending',1,'publishing','[]')").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO content_object_leases(operation_id,object_id) VALUES('pending',1)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/0014_content_preparation_leases.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let row: (String, Option<Vec<u8>>, i64) = sqlx::query_as("SELECT o.recovery_json,o.store_root,l.object_id FROM content_operations o JOIN content_object_leases l ON l.operation_id=o.id").fetch_one(&pool).await.unwrap();
    assert_eq!(row, ("[]".into(), None, 1));
    for statement in [
        "INSERT INTO content_operations(id,instance_id,state,recovery_json) VALUES('bad',1,'preparing','[]')",
        "INSERT INTO content_operations(id,instance_id,state,recovery_json) VALUES('bad',1,'completed','[]')",
        "UPDATE content_operations SET state='preparing' WHERE id='pending'",
    ] {
        assert!(sqlx::query(statement).execute(&pool).await.is_err(), "{statement}");
    }
    sqlx::query("INSERT INTO content_operations(id,instance_id,state,recovery_json,store_root) VALUES('new',1,'preparing','[]',X'01')").execute(&pool).await.unwrap();
    assert!(
        sqlx::query("UPDATE content_operations SET state='completed' WHERE id='new'")
            .execute(&pool)
            .await
            .is_err()
    );
    sqlx::query("UPDATE content_operations SET state='publishing' WHERE id='new'")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE content_operations SET state='completed' WHERE id IN ('new','pending')")
        .execute(&pool)
        .await
        .unwrap();
    assert!(sqlx::query("DELETE FROM content_objects WHERE id=1")
        .execute(&pool)
        .await
        .is_err());
}

#[async_std::test]
async fn object_path_upgrade_keeps_identity_links_and_journal_leases() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for migration in sqlx::migrate!("./migrations")
        .iter()
        .filter(|m| m.version <= 12)
    {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'old','old','1.21','vanilla','https://example.invalid','prism')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO content_objects(id,sha256,size_bytes,store_path,store_state) VALUES(1,?,3,'/old/root/object','ready')")
        .bind(format!("{:064x}", 1)).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO content_operations(id,instance_id,state,recovery_json) VALUES('pending',1,'publishing','[]')")
        .execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO content_object_leases(operation_id,object_id) VALUES('pending',1)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/0013_content_object_paths.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let row: (Option<String>, String, i64) = sqlx::query_as("SELECT o.store_path,o.store_state,l.object_id FROM content_objects o JOIN content_object_leases l ON l.object_id=o.id")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row, (None, "ready".into(), 1));
    assert!(
        sqlx::query("UPDATE content_objects SET store_path='/new/absolute' WHERE id=1")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM content_objects WHERE id=1")
        .execute(&pool)
        .await
        .is_err());
}

#[async_std::test]
async fn library_placement_upgrade_preserves_legacy_order_paths_and_roles() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    for migration in sqlx::migrate!("./migrations")
        .iter()
        .filter(|m| m.version <= 11)
    {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'old','old','1.21','vanilla','https://example.invalid','prism')")
        .execute(&pool).await.unwrap();
    for (id, native, position) in [(1, false, 1), (2, true, 0)] {
        sqlx::query("INSERT INTO libraries(id,name,hash,path,native,url) VALUES(?,'library',?,?,?,'https://example.invalid')")
            .bind(id).bind(format!("{id:040x}")).bind(format!("/legacy/{id}.jar")).bind(native).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO instances_libraries(instance_id,library_id,position) VALUES(1,?,?)",
        )
        .bind(id)
        .bind(position)
        .execute(&pool)
        .await
        .unwrap();
    }
    sqlx::raw_sql(include_str!(
        "../../../migrations/0012_instance_library_placements.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let rows: Vec<(String, bool)> =
        sqlx::query_as("SELECT path,native FROM instance_library_placements ORDER BY position")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![
            ("/legacy/2.jar".into(), true),
            ("/legacy/1.jar".into(), false)
        ]
    );
    // The old tables are retained solely as a migration snapshot.
    let old: i64 = sqlx::query_scalar("SELECT count(*) FROM libraries")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(old, 2);
}

#[async_std::test]
async fn provider_ownership_upgrade_preserves_files_and_enforces_all_mutations() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let migrations = sqlx::migrate!("./migrations");
    for migration in migrations
        .iter()
        .filter(|migration| migration.version <= 10)
    {
        sqlx::raw_sql(&migration.sql).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO instances(id,name,dir,version,loader,manifest_url,meta_provider) VALUES(1,'old','old','1.21','vanilla','https://example.invalid','prism')")
        .execute(&pool).await.unwrap();
    for id in [1, 2] {
        sqlx::query("INSERT INTO content_objects(id,sha256,size_bytes) VALUES(?,?,3)")
            .bind(id)
            .bind(format!("{id:064x}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO content_provider_files(id,object_id,provider,project_id,release_id,file_key,match_evidence) VALUES(?,?,'modrinth','project','release','mod.jar','verified_hash')")
            .bind(id).bind(id).execute(&pool).await.unwrap();
    }
    // Both a valid and a historically mismatched association, plus a pending journal.
    for (path, provider) in [("mods/valid.jar", 1), ("mods/invalid.jar", 2)] {
        sqlx::query("INSERT INTO instance_content(instance_id,object_id,kind,relative_path,path_key,mod_enabled,placement,preferred_provider_file_id) VALUES(1,1,'mod',?,?,1,'copy',?)")
            .bind(path).bind(path.as_bytes()).bind(provider).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO content_operations(id,instance_id,state,recovery_json) VALUES('pending',1,'publishing','[]')")
        .execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../../../migrations/0011_content_provider_ownership.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let rows: Vec<(String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT relative_path,object_id,preferred_provider_file_id FROM instance_content ORDER BY id",
    ).fetch_all(&pool).await.unwrap();
    assert_eq!(
        rows,
        vec![
            ("mods/valid.jar".into(), 1, Some(1)),
            ("mods/invalid.jar".into(), 1, None)
        ]
    );
    let journal: String =
        sqlx::query_scalar("SELECT recovery_json FROM content_operations WHERE id='pending'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(journal, "[]");
    for statement in [
        "UPDATE instance_content SET preferred_provider_file_id=2 WHERE relative_path='mods/valid.jar'",
        "UPDATE instance_content SET object_id=2 WHERE relative_path='mods/valid.jar'",
        "UPDATE instance_content SET object_id=NULL WHERE relative_path='mods/valid.jar'",
        "UPDATE content_provider_files SET object_id=2 WHERE id=1",
        "INSERT INTO instance_content(instance_id,object_id,kind,relative_path,path_key,mod_enabled,placement,preferred_provider_file_id) VALUES(1,1,'mod','mods/new.jar',X'00',1,'copy',2)",
    ] {
        assert!(sqlx::query(statement).execute(&pool).await.is_err(), "{statement}");
    }
    // Existing FK actions must remain usable: deleting provenance clears the preference.
    sqlx::query("DELETE FROM content_provider_files WHERE id=1")
        .execute(&pool)
        .await
        .unwrap();
    let preferred: Option<i64> = sqlx::query_scalar("SELECT preferred_provider_file_id FROM instance_content WHERE relative_path='mods/valid.jar'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(preferred, None);
    assert!(sqlx::query("DELETE FROM instances WHERE id=1")
        .execute(&pool)
        .await
        .is_err());
    sqlx::query("DELETE FROM content_operations")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM instances WHERE id=1")
        .execute(&pool)
        .await
        .unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM instance_content")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
#[async_std::test]
async fn constraint_mapping_uses_database_metadata_and_keeps_sql_source() {
    use std::error::Error;
    let db = super::Database::init("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE q07_parent (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL)")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("CREATE TABLE q07_child (parent INTEGER REFERENCES q07_parent(id))")
        .execute(&db.pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO q07_parent VALUES (1, 'same')")
        .execute(&db.pool)
        .await
        .unwrap();
    let unique = super::DbError::from(
        sqlx::query("INSERT INTO q07_parent VALUES (2, 'same')")
            .execute(&db.pool)
            .await
            .unwrap_err(),
    );
    assert!(matches!(unique, super::DbError::Conflict(_)));
    assert!(unique
        .source()
        .unwrap()
        .downcast_ref::<sqlx::Error>()
        .is_some());
    let foreign = super::DbError::from(
        sqlx::query("INSERT INTO q07_child VALUES (999)")
            .execute(&db.pool)
            .await
            .unwrap_err(),
    );
    assert!(matches!(foreign, super::DbError::ForeignKey(_)));
    assert!(foreign
        .source()
        .unwrap()
        .downcast_ref::<sqlx::Error>()
        .is_some());
    sqlx::query("CREATE TRIGGER q07_misleading BEFORE INSERT ON q07_child BEGIN SELECT RAISE(ABORT, 'UNIQUE constraint failed'); END").execute(&db.pool).await.unwrap();
    let misleading = super::DbError::from(
        sqlx::query("INSERT INTO q07_child VALUES (1)")
            .execute(&db.pool)
            .await
            .unwrap_err(),
    );
    assert!(matches!(misleading, super::DbError::Db(_)));
}
