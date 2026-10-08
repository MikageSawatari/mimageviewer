//! Review-only fault-injection probe. Uses a disposable database exclusively.
fn main() {
    use mimageviewer::settings::Settings;
    use mimageviewer::settings_db::SettingsDb;
    let isolated = tempfile::tempdir().expect("temporary directory");
    let db = SettingsDb::create_new(isolated.path()).expect("create disposable database");
    db.save_full(&Settings::default()).expect("bootstrap");
    let connection = rusqlite::Connection::open(isolated.path().join("settings.db")).unwrap();
    connection.execute_batch(
        "INSERT INTO custom_open_with_apps(exe_path, display_name, sort_index)
         VALUES ('C:/ReviewOnly/viewer.exe', 'Preserved legacy app', 0);
         DELETE FROM schema_meta WHERE key = 'external_tools_migrated_from_custom_open_with';
         CREATE TRIGGER review_fail_external_migration BEFORE INSERT ON external_tools
         BEGIN SELECT RAISE(ABORT, 'review: transient migration failure'); END;"
    ).unwrap();
    let mut loaded = db.load_into_settings().expect("load continues despite migration failure");
    println!("after failed migration: legacy={} external={}", loaded.custom_open_with_apps.len(), loaded.external_tools.len());
    assert_eq!(loaded.custom_open_with_apps.len(), 1);
    assert!(loaded.external_tools.is_empty());
    connection.execute_batch("DROP TRIGGER review_fail_external_migration;").unwrap();
    loaded.show_hidden_files = !loaded.show_hidden_files;
    db.save_full(&loaded).expect("unrelated settings save after failure has cleared");
    let reloaded = db.load_into_settings().expect("reload");
    let marker: i64 = connection.query_row(
        "SELECT COUNT(*) FROM schema_meta WHERE key = 'external_tools_migrated_from_custom_open_with'",
        [], |row| row.get(0)
    ).unwrap();
    println!("after ordinary save and reload: legacy={} external={} migration_marker={}", reloaded.custom_open_with_apps.len(), reloaded.external_tools.len(), marker);
    assert_eq!(marker, 1);
    assert!(reloaded.external_tools.is_empty(), "probe expects the reviewed failure; fix changes this result");
    println!("CONFIRMED: the legacy registration is stranded by an unrelated successful save.");
}
