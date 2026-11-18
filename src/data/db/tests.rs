use super::*;

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
