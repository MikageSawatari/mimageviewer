use super::*;

fn base() -> StatusPayload {
    StatusPayload {
        app_version: "fixture".into(),
        permission: PermissionSnapshot {
            level: AgentLevel::ReadOnly,
            thumbnails: false,
            gps: false,
            ai_captions: false,
            transcripts: false,
        },
        fts_ready: false,
        remote_active: false,
        search_scope: Vec::new(),
        search_scope_truncated: false,
    }
}

fn favorite(path: String, metadata: bool) -> crate::settings::FavoriteEntry {
    let mut favorite = crate::settings::FavoriteEntry::new("fixture".into(), path.into());
    favorite.auto_index_metadata = metadata;
    favorite.auto_index_structure = !metadata;
    favorite.auto_index_thumbs = !metadata;
    favorite
}

#[test]
fn scope_contains_only_metadata_favorites_in_saved_order() {
    let favorites = vec![
        favorite("C:/second".into(), true),
        favorite("C:/structure-and-thumbnails-only".into(), false),
        favorite("C:/first".into(), true),
    ];
    let status = with_search_scope(base(), &favorites).unwrap();
    assert_eq!(status.search_scope, ["C:/second", "C:/first"]);
    assert!(!status.search_scope_truncated);
    let empty = with_search_scope(base(), &[]).unwrap();
    assert!(empty.search_scope.is_empty());
    assert!(!empty.search_scope_truncated);
}

#[test]
fn scope_count_limit_only_reports_actual_omission() {
    let mut favorites: Vec<_> = (0..100)
        .map(|n| favorite(format!("C:/scope/{n}"), true))
        .collect();
    let exact = with_search_scope(base(), &favorites).unwrap();
    assert_eq!(exact.search_scope.len(), 100);
    assert!(!exact.search_scope_truncated);
    favorites.push(favorite("C:/disabled".into(), false));
    assert!(
        !with_search_scope(base(), &favorites)
            .unwrap()
            .search_scope_truncated
    );
    favorites.push(favorite("C:/extra".into(), true));
    let over = with_search_scope(base(), &favorites).unwrap();
    assert_eq!(over.search_scope, exact.search_scope);
    assert!(over.search_scope_truncated);
}

#[test]
fn scope_character_budget_removes_whole_paths_from_end() {
    let favorites: Vec<_> = (0..100)
        .map(|n| favorite(format!(r"C:\scope\{}\{n}", "日本語\\\"".repeat(100)), true))
        .collect();
    let status = with_search_scope(base(), &favorites).unwrap();
    assert!(status.search_scope_truncated);
    assert!(!status.search_scope.is_empty());
    assert!(status.search_scope.len() < 100);
    assert!(mcp_content_chars(&status).unwrap() <= MAX_PAGE_CHARS);
    for (actual, favorite) in status.search_scope.iter().zip(&favorites) {
        assert_eq!(actual, &favorite.path.to_string_lossy());
    }
    let mut next = status.clone();
    next.search_scope.push(
        favorites[status.search_scope.len()]
            .path
            .to_string_lossy()
            .into_owned(),
    );
    assert!(mcp_content_chars(&next).unwrap() > MAX_PAGE_CHARS);
    let huge = favorite(format!("C:/{}", "x".repeat(MAX_PAGE_CHARS)), true);
    let omitted = with_search_scope(base(), &[huge]).unwrap();
    assert!(omitted.search_scope.is_empty());
    assert!(omitted.search_scope_truncated);
}

#[test]
fn status_handler_reads_current_saved_scope_and_keeps_other_fields() {
    let fixture = super::super::tests::Fixture::new();
    let connection = fixture.connection();
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.favorites = vec![
            favorite("C:/indexed".into(), true),
            favorite("C:/other".into(), false),
        ];
        db.save_full(&settings)
    })
    .unwrap();
    let Payload::Status(status) = fixture
        .call(&connection, 1, Command::Status)
        .result
        .unwrap()
    else {
        panic!("expected status");
    };
    assert_eq!(status.search_scope, ["C:/indexed"]);
    assert!(!status.search_scope_truncated);
    assert_eq!(status.permission.level, AgentLevel::ReadOnly);
    assert!(!status.fts_ready);
    assert!(!status.remote_active);
    assert_eq!(status.app_version, env!("CARGO_PKG_VERSION"));
}
