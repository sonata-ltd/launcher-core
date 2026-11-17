//! Регрессии CAS, графа и inventory из раннего reference перенесены на реальные модули ядра.
use std::{collections::HashMap, fs, io, sync::Arc};

use crate::{
    instance::content::scan_policy::{
        identity_decision, DirtyGeneration, IdentityDecision, VerificationMode,
    },
    instance::content::{
        model::{Candidate, Dependency, DependencyKind, ProjectKey, Provider, ReleaseKey},
        resolve::graph::{solve, solve_with_installed, Requirement, ResolveError},
    },
    utils::{
        cancellation::Cancellation,
        files::{
            digest::digest_file,
            placement::{copy_new, place_new, CopyFallback, PlacementKind},
        },
    },
};

#[test]
fn known_hashes_are_computed_in_one_pass() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("example.jar");
    fs::write(&file, b"abc").unwrap();
    let (digests, _) = digest_file(&file, &Cancellation::default(), |_| {}).unwrap();

    assert_eq!(digests.size_bytes, 3);
    assert_eq!(digests.sha1, "a9993e364706816aba3e25717850c26c9cd0d89d");
    assert_eq!(
        digests.sha256,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(digests.sha512, "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f");
}

#[test]
fn mutation_during_hashing_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("example.jar");
    fs::write(&file, vec![1_u8; 128 * 1024]).unwrap();
    let mut mutated = false;

    let result = digest_file(&file, &Cancellation::default(), |_| {
        if !mutated {
            fs::write(&file, b"different length").unwrap();
            mutated = true;
        }
    });

    assert!(result.is_err());
}

#[test]
fn copying_never_overwrites_an_existing_link() {
    let directory = tempfile::tempdir().unwrap();
    let object = directory.path().join("object");
    let placement = directory.path().join("mod.jar");
    let replacement = directory.path().join("replacement");
    fs::write(&object, b"original").unwrap();
    fs::write(&replacement, b"new").unwrap();
    fs::hard_link(&object, &placement).unwrap();

    assert!(copy_new(&replacement, &placement, &Cancellation::default()).is_err());
    assert_eq!(fs::read(&object).unwrap(), b"original");
}

#[test]
fn replacing_one_directory_entry_preserves_another_instance() {
    let directory = tempfile::tempdir().unwrap();
    let object = directory.path().join("object");
    let instance_a = directory.path().join("a.jar");
    let instance_b = directory.path().join("b.jar");
    let replacement = directory.path().join("replacement.jar");
    fs::write(&object, b"version one").unwrap();

    let result = place_new(
        &object,
        &instance_a,
        CopyFallback::Never,
        &Cancellation::default(),
    )
    .unwrap();
    assert_eq!(result.kind, PlacementKind::Hardlink);
    fs::hard_link(&object, &instance_b).unwrap();
    fs::write(&replacement, b"version two").unwrap();

    // remove+rename здесь только демонстрируют семантику записей каталога.
    // Production-сценарий обязан иметь журнал и backup, описанные в плане.
    fs::remove_file(&instance_a).unwrap();
    fs::rename(&replacement, &instance_a).unwrap();

    assert_eq!(fs::read(&instance_a).unwrap(), b"version two");
    assert_eq!(fs::read(&instance_b).unwrap(), b"version one");
}

#[test]
fn writing_through_hardlink_changes_shared_object() {
    let directory = tempfile::tempdir().unwrap();
    let object = directory.path().join("object");
    let placement = directory.path().join("mod.jar");
    fs::write(&object, b"original").unwrap();
    fs::hard_link(&object, &placement).unwrap();
    fs::write(&placement, b"external edit").unwrap();

    assert_eq!(fs::read(&object).unwrap(), b"external edit");
}

#[test]
fn cancellation_removes_partial_copy_and_preserves_source() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let destination = directory.path().join("destination");
    fs::write(&source, b"original").unwrap();
    let cancellation = Cancellation::default();
    cancellation.cancel();

    assert!(copy_new(&source, &destination, &cancellation).is_err());
    assert!(!destination.exists());
    assert_eq!(fs::read(source).unwrap(), b"original");
}

#[test]
fn fast_mode_defers_new_content_and_full_mode_reads_even_unchanged_content() {
    assert_eq!(
        identity_decision(VerificationMode::Fast, false, false, false),
        IdentityDecision::QueueBackgroundIndex,
    );
    assert_eq!(
        identity_decision(VerificationMode::Fast, true, true, false),
        IdentityDecision::UseCachedIdentity,
    );
    assert_eq!(
        identity_decision(VerificationMode::Full, true, true, false),
        IdentityDecision::HashBeforeLaunch,
    );
}

#[test]
fn event_arriving_during_scan_is_not_cleared() {
    let dirty = Arc::new(DirtyGeneration::default());
    let started = dirty.current();
    let callback_dirty = dirty.clone();
    std::thread::spawn(move || callback_dirty.mark_changed())
        .join()
        .unwrap();

    assert!(dirty.changed_since(started));
}

fn project(name: &str) -> ProjectKey {
    ProjectKey {
        provider: Provider::Modrinth,
        project_id: name.to_owned(),
    }
}

fn requirement(name: &str, version: Option<&str>) -> Requirement {
    Requirement {
        project: project(name),
        exact_release: version.map(str::to_owned),
    }
}

fn candidate(name: &str, version: &str, dependencies: Vec<Dependency>) -> Candidate {
    Candidate {
        key: ReleaseKey {
            project: project(name),
            release_id: version.to_owned(),
        },
        dependencies,
    }
}

fn required(name: &str, version: Option<&str>) -> Dependency {
    Dependency {
        project: project(name),
        exact_release: version.map(str::to_owned),
        kind: DependencyKind::Required,
    }
}

#[test]
fn resolver_backtracks_when_a_later_dependency_adds_an_exact_constraint() {
    let catalog = HashMap::from([
        (
            project("a"),
            vec![candidate(
                "a",
                "1",
                vec![required("b", None), required("c", None)],
            )],
        ),
        (
            project("b"),
            vec![candidate("b", "2", vec![]), candidate("b", "1", vec![])],
        ),
        (
            project("c"),
            vec![candidate("c", "1", vec![required("b", Some("1"))])],
        ),
    ]);

    // B=2 выбран до раскрытия C. Решение требует возврата к B=1:
    // A, B2, C(конфликт), B1, C — ровно пять выборов кандидата.
    assert_eq!(
        solve(&[requirement("a", None)], &catalog, 4).unwrap_err(),
        ResolveError::SearchLimit
    );
    let solution = solve(&[requirement("a", None)], &catalog, 5).unwrap();
    assert_eq!(solution[&project("b")].key.release_id, "1");
}

#[test]
fn resolver_handles_cycles_without_repeated_selection() {
    let catalog = HashMap::from([
        (
            project("a"),
            vec![candidate("a", "1", vec![required("b", None)])],
        ),
        (
            project("b"),
            vec![candidate("b", "1", vec![required("a", None)])],
        ),
    ]);

    assert_eq!(
        solve(&[requirement("a", None)], &catalog, 10)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn incompatible_roots_are_rejected() {
    let catalog = HashMap::from([
        (
            project("a"),
            vec![candidate(
                "a",
                "1",
                vec![Dependency {
                    project: project("b"),
                    exact_release: None,
                    kind: DependencyKind::Incompatible,
                }],
            )],
        ),
        (project("b"), vec![candidate("b", "1", vec![])]),
    ]);

    assert_eq!(
        solve(
            &[requirement("a", None), requirement("b", None)],
            &catalog,
            10
        )
        .unwrap_err(),
        ResolveError::Conflict,
    );
}

#[test]
fn decision_limit_is_not_reported_as_conflict() {
    let catalog = HashMap::from([(project("a"), vec![candidate("a", "1", vec![])])]);

    assert_eq!(
        solve(&[requirement("a", None)], &catalog, 0).unwrap_err(),
        ResolveError::SearchLimit
    );
}

#[test]
fn repeated_imports_share_one_cas_path_and_preserve_originals() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("local.jar");
    fs::write(&source, b"same bytes").unwrap();
    let store = crate::instance::content::store::ObjectStore::new(directory.path().join("store"));
    let cancellation = Cancellation::default();

    let first = store.import_file(&source, None, &cancellation).unwrap();
    let second = store
        .import_file(&source, Some(&first.digests.sha256), &cancellation)
        .unwrap();

    assert_eq!(first.path, second.path);
    assert!(source.exists());
    assert_eq!(fs::read(first.path).unwrap(), b"same bytes");
}

#[test]
fn import_rejects_changed_preview_and_corrupt_existing_object() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("local.jar");
    fs::write(&source, b"original").unwrap();
    let store = crate::instance::content::store::ObjectStore::new(directory.path().join("store"));
    let cancellation = Cancellation::default();
    let object = store.import_file(&source, None, &cancellation).unwrap();

    assert!(store
        .import_file(&source, Some(&"0".repeat(64)), &cancellation)
        .is_err());
    fs::write(&object.path, b"corrupted").unwrap();
    assert!(store.import_file(&source, None, &cancellation).is_err());
}

#[test]
fn import_validates_digests_of_the_staged_bytes_before_publishing() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.jar");
    let data = vec![3; 128 * 1024 + 1];
    fs::write(&source, &data).unwrap();
    let root = directory.path().join("store");
    let store = crate::instance::content::store::ObjectStore::new(root.clone());
    let cancel = Cancellation::default();
    let (expected, _) = digest_file(&source, &cancel, |_| {}).unwrap();

    let error = store
        .import_checked(&source, &cancel, |digests| {
            assert_eq!(digests, &expected);
            Err(io::Error::other("provider digest rejected"))
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "provider digest rejected");
    assert!(!store.path_for(&expected.sha256).unwrap().exists());
    assert_eq!(fs::read_dir(root.join("staging")).unwrap().count(), 0);

    let object = store
        .import_checked(&source, &cancel, |digests| {
            assert_eq!(digests, &expected);
            Ok::<_, io::Error>(())
        })
        .unwrap();
    assert_eq!(fs::read(object.path).unwrap(), data);
}

#[test]
fn inventory_reads_archive_metadata_without_descending_into_packs_or_worlds() {
    use crate::instance::content::inventory::{inventory_scope, ContentKind, ObservedType};

    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("mods")).unwrap();
    fs::write(directory.path().join("mods/a.jar"), b"not even a ZIP").unwrap();
    fs::write(directory.path().join("mods/b.jar.disabled"), b"disabled").unwrap();
    fs::write(directory.path().join("mods/config.txt"), b"not a mod").unwrap();
    fs::create_dir_all(directory.path().join("resourcepacks/local/deep")).unwrap();

    let mods = inventory_scope(directory.path(), ContentKind::Mod).unwrap();
    assert_eq!(mods.entries.len(), 2);
    assert!(mods
        .entries
        .iter()
        .all(|entry| matches!(entry.entry_type, ObservedType::Archive(_))));
    assert!(mods
        .entries
        .iter()
        .any(|entry| entry.mod_enabled == Some(false)));

    let packs = inventory_scope(directory.path(), ContentKind::ResourcePack).unwrap();
    assert_eq!(packs.entries.len(), 1);
    assert!(matches!(
        packs.entries[0].entry_type,
        ObservedType::LocalDirectory
    ));
}

#[test]
fn missing_content_directory_differs_from_missing_instance() {
    use crate::instance::content::inventory::{inventory_scope, ContentKind};

    let directory = tempfile::tempdir().unwrap();
    assert!(
        !inventory_scope(directory.path(), ContentKind::Mod)
            .unwrap()
            .directory_exists
    );
    assert!(inventory_scope(&directory.path().join("missing-instance"), ContentKind::Mod).is_err());
}

#[test]
fn native_watcher_wakes_consumer_without_polling_archive_contents() {
    use std::{sync::mpsc, time::Duration};

    let directory = tempfile::tempdir().unwrap();
    let dirty = Arc::new(DirtyGeneration::default());
    let (sender, receiver) = mpsc::sync_channel(1);
    let mut watcher =
        crate::instance::content::watch::ContentWatcher::new(dirty.clone(), sender).unwrap();
    watcher.watch_directory(directory.path()).unwrap();
    fs::write(directory.path().join("new.jar"), b"content").unwrap();

    receiver.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(dirty.current() > 0);
}

#[test]
fn cancelled_import_preserves_source_and_leaves_no_staging_files() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("manual.jar");
    fs::write(&source, vec![7; 128 * 1024]).unwrap();
    let root = directory.path().join("store");
    let store = crate::instance::content::store::ObjectStore::new(root.clone());
    let cancellation = Cancellation::default();
    cancellation.cancel();

    assert!(store.import_file(&source, None, &cancellation).is_err());
    assert_eq!(fs::metadata(source).unwrap().len(), 128 * 1024);
    assert_eq!(fs::read_dir(root.join("staging")).unwrap().count(), 0);
}

#[test]
fn concurrent_imports_publish_identical_content_without_clobbering() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("manual.zip");
    let bytes = vec![23; 256 * 1024];
    fs::write(&source, &bytes).unwrap();
    let root = directory.path().join("store");
    let barrier = Arc::new(std::sync::Barrier::new(4));

    let workers: Vec<_> = (0..4)
        .map(|_| {
            let source = source.clone();
            let root = root.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                crate::instance::content::store::ObjectStore::new(root)
                    .import_file(&source, None, &Cancellation::default())
                    .unwrap()
                    .path
            })
        })
        .collect();
    let paths: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect();

    assert!(paths.iter().all(|path| path == &paths[0]));
    assert_eq!(fs::read(&paths[0]).unwrap(), bytes);
    assert_eq!(fs::read_dir(root.join("staging")).unwrap().count(), 0);
}

#[test]
fn cas_rejects_path_traversal_and_normalizes_hex_case() {
    let store =
        crate::instance::content::store::ObjectStore::new(std::path::PathBuf::from("store"));
    for invalid in [
        "../object",
        "",
        &"g".repeat(64),
        &"a".repeat(63),
        &"a".repeat(65),
    ] {
        assert!(store.path_for(invalid).is_err(), "accepted {invalid}");
    }
    assert_eq!(
        store.path_for(&"AB".repeat(32)).unwrap(),
        store.path_for(&"ab".repeat(32)).unwrap()
    );
}

#[test]
fn deleting_one_hardlink_does_not_delete_the_object_or_other_instance() {
    let directory = tempfile::tempdir().unwrap();
    let object = directory.path().join("object");
    let first = directory.path().join("first.jar");
    let second = directory.path().join("second.jar");
    fs::write(&object, b"shared").unwrap();
    fs::hard_link(&object, &first).unwrap();
    fs::hard_link(&object, &second).unwrap();

    fs::rename(&first, directory.path().join("first.jar.disabled")).unwrap();
    fs::remove_file(directory.path().join("first.jar.disabled")).unwrap();
    assert_eq!(fs::read(object).unwrap(), b"shared");
    assert_eq!(fs::read(second).unwrap(), b"shared");
}

#[test]
fn fast_mode_invalidates_identity_on_event_even_when_metadata_matches() {
    assert_eq!(
        identity_decision(VerificationMode::Fast, true, true, true),
        IdentityDecision::QueueBackgroundIndex,
    );
    assert_eq!(
        identity_decision(VerificationMode::Fast, true, false, false),
        IdentityDecision::QueueBackgroundIndex,
    );
}

#[test]
fn shader_inventory_has_no_fake_active_state_and_ignores_settings_files() {
    use crate::instance::content::inventory::{inventory_scope, ContentKind};
    let directory = tempfile::tempdir().unwrap();
    let packs = directory.path().join("shaderpacks");
    fs::create_dir(&packs).unwrap();
    fs::write(packs.join("pack.ZIP"), b"zip").unwrap();
    fs::write(packs.join("pack.zip.txt"), b"settings").unwrap();
    let snapshot = inventory_scope(directory.path(), ContentKind::ShaderPack).unwrap();

    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.entries[0].mod_enabled, None);
}

#[cfg(unix)]
#[test]
fn symbolic_links_are_observed_but_not_imported_or_hashed() {
    use crate::instance::content::inventory::{inventory_scope, ContentKind, ObservedType};
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let external = directory.path().join("external.jar");
    fs::write(&external, b"external").unwrap();
    fs::create_dir(directory.path().join("mods")).unwrap();
    let link = directory.path().join("mods/link.jar");
    symlink(&external, &link).unwrap();
    let snapshot = inventory_scope(directory.path(), ContentKind::Mod).unwrap();
    assert!(matches!(
        snapshot.entries[0].entry_type,
        ObservedType::SymbolicLink
    ));
    assert!(digest_file(&link, &Cancellation::default(), |_| {}).is_err());
    let store = crate::instance::content::store::ObjectStore::new(directory.path().join("store"));
    assert!(store
        .import_file(&link, None, &Cancellation::default())
        .is_err());
    assert_eq!(fs::read(external).unwrap(), b"external");
}

#[cfg(unix)]
#[test]
fn non_utf8_names_are_preserved_without_lossy_identity() {
    use crate::instance::content::inventory::{inventory_scope, ContentKind};
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let directory = tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join("mods")).unwrap();
    let name = OsString::from_vec(b"mod-\xff.jar".to_vec());
    fs::write(directory.path().join("mods").join(&name), b"content").unwrap();
    let snapshot = inventory_scope(directory.path(), ContentKind::Mod).unwrap();
    assert_eq!(snapshot.entries[0].name, name);
}

#[test]
fn optional_and_embedded_dependencies_do_not_require_external_candidates() {
    for kind in [DependencyKind::Optional, DependencyKind::Embedded] {
        let catalog = HashMap::from([(
            project("root"),
            vec![candidate(
                "root",
                "1",
                vec![Dependency {
                    project: project("not-in-catalog"),
                    exact_release: None,
                    kind,
                }],
            )],
        )]);
        assert_eq!(
            solve(&[requirement("root", None)], &catalog, 10)
                .unwrap()
                .len(),
            1
        );
    }
}

#[test]
fn contradictory_exact_pins_never_silently_choose_latest() {
    let catalog = HashMap::from([(
        project("mod"),
        vec![candidate("mod", "2", vec![]), candidate("mod", "1", vec![])],
    )]);
    let result = solve(
        &[requirement("mod", Some("1")), requirement("mod", Some("2"))],
        &catalog,
        100,
    );
    assert_eq!(result.unwrap_err(), ResolveError::Conflict);
}

#[test]
fn incompatible_exact_release_allows_another_version() {
    let catalog = HashMap::from([
        (
            project("a"),
            vec![candidate(
                "a",
                "1",
                vec![Dependency {
                    project: project("b"),
                    exact_release: Some("2".into()),
                    kind: DependencyKind::Incompatible,
                }],
            )],
        ),
        (
            project("b"),
            vec![candidate("b", "2", vec![]), candidate("b", "1", vec![])],
        ),
    ]);
    let solution = solve(
        &[requirement("a", None), requirement("b", None)],
        &catalog,
        20,
    )
    .unwrap();
    assert_eq!(solution[&project("b")].key.release_id, "1");
}

#[test]
fn unknown_relation_is_explicit_and_can_backtrack_to_a_supported_release() {
    let unknown = candidate(
        "mod",
        "2",
        vec![Dependency {
            project: project("other"),
            exact_release: None,
            kind: DependencyKind::Unknown("future".into()),
        }],
    );
    let only_unknown = HashMap::from([(project("mod"), vec![unknown.clone()])]);
    assert_eq!(
        solve(&[requirement("mod", None)], &only_unknown, 10).unwrap_err(),
        ResolveError::UnsupportedRelation
    );
    let catalog = HashMap::from([(project("mod"), vec![unknown, candidate("mod", "1", vec![])])]);
    assert_eq!(
        solve(&[requirement("mod", None)], &catalog, 10).unwrap()[&project("mod")]
            .key
            .release_id,
        "1"
    );
}

#[test]
fn identical_provider_local_ids_do_not_merge_projects_across_providers() {
    let modrinth = project("same-id");
    let mut curseforge = modrinth.clone();
    curseforge.provider = Provider::CurseForge;
    let catalog = HashMap::from([
        (modrinth.clone(), vec![candidate("same-id", "1", vec![])]),
        (
            curseforge.clone(),
            vec![Candidate {
                key: ReleaseKey {
                    project: curseforge.clone(),
                    release_id: "2".into(),
                },
                dependencies: vec![],
            }],
        ),
    ]);
    let roots = [
        Requirement {
            project: modrinth,
            exact_release: None,
        },
        Requirement {
            project: curseforge,
            exact_release: None,
        },
    ];
    assert_eq!(solve(&roots, &catalog, 10).unwrap().len(), 2);
}

#[test]
fn installed_projects_satisfy_requirements_and_prevent_incompatible_candidates() {
    let installed = HashMap::from([(
        project("existing"),
        candidate(
            "existing",
            "1",
            vec![Dependency {
                project: project("root"),
                exact_release: Some("blocked-by-installed".into()),
                kind: DependencyKind::Incompatible,
            }],
        ),
    )]);
    let candidates = HashMap::from([(
        project("root"),
        vec![
            candidate("root", "blocked-by-installed", vec![]),
            candidate(
                "root",
                "bad",
                vec![Dependency {
                    project: project("existing"),
                    exact_release: None,
                    kind: DependencyKind::Incompatible,
                }],
            ),
            candidate("root", "also-bad", vec![required("existing", Some("2"))]),
            candidate("root", "good", vec![required("existing", Some("1"))]),
        ],
    )]);
    let selected =
        solve_with_installed(&[requirement("root", None)], &candidates, installed, 16).unwrap();
    assert_eq!(selected[&project("root")].key.release_id, "good");
    assert_eq!(selected[&project("existing")].key.release_id, "1");
}
