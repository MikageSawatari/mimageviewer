use super::*;

fn keys(keys: &[&str]) -> HashSet<String> {
    keys.iter().map(|key| (*key).to_owned()).collect()
}

fn available(requested: &HashSet<String>) -> FontLoadResult {
    assemble_fontset(requested, Some(comic_core::FontSet::new()), |_| None)
}

fn pending(owner: &mut ComicFontOwner, requested: &[&str]) -> mpsc::Sender<FontLoadResult> {
    assert!(owner.load.is_none());
    let (tx, rx) = mpsc::channel();
    owner.load = Some(Load {
        rx,
        keys: keys(requested),
    });
    tx
}

#[test]
fn font_request_adoption_is_nonblocking_and_accumulates_attempted() {
    let ctx = egui::Context::default();
    let mut owner = ComicFontOwner::default();
    owner.loader_for_test(Arc::new(available));
    let tx = pending(&mut owner, &["missing-a"]);
    owner.poll();
    assert!(matches!(owner.current, Current::Unloaded));
    // Requests while busy do not replace the receiver or add a second job.
    owner.request(&ctx, &["missing-b".to_owned()]);
    assert_eq!(owner.load.as_ref().unwrap().keys, keys(&["missing-a"]));
    tx.send(available(&keys(&["missing-a"]))).ok().unwrap();
    owner.poll();
    assert!(owner.fonts().is_some());
    assert!(owner.needed(&["missing-a".to_owned()]).is_none());
    owner.request(&ctx, &["missing-b".to_owned()]);
    owner.wait_for_test();
    owner.poll();
    assert!(owner.load.is_none());
    let Current::Ready { attempted, .. } = &owner.current else {
        panic!()
    };
    assert_eq!(attempted, &keys(&["missing-a", "missing-b"]));
    assert!(owner.needed(&["missing-a".to_owned()]).is_none());
}

#[test]
fn font_sync_takeover_discards_inflight_and_accumulates_attempts() {
    let mut owner = ComicFontOwner::default();
    owner.loader_for_test(Arc::new(available));
    owner.ensure(&["missing-a".to_owned()], &HashMap::new());
    let tx = pending(&mut owner, &["stale"]);
    owner.ensure(&["missing-b".to_owned()], &HashMap::new());
    let current = owner.fonts().clone().unwrap();
    assert!(tx.send(available(&keys(&["stale"]))).is_err());
    owner.poll();
    assert!(Arc::ptr_eq(&current, owner.fonts().as_ref().unwrap()));
    let Current::Ready { attempted, .. } = &owner.current else {
        panic!()
    };
    assert_eq!(attempted, &keys(&["missing-a", "missing-b"]));
}

#[test]
fn font_sync_consumes_ready_result_without_reading_again() {
    let mut owner = ComicFontOwner::default();
    owner.loader_for_test(Arc::new(|_| panic!("ready fonts must be reused")));
    let tx = pending(&mut owner, &["missing"]);
    tx.send(available(&keys(&["missing"]))).ok().unwrap();
    owner.ensure(&["missing".to_owned()], &HashMap::new());
    assert!(owner.fonts().is_some());
    assert!(owner.load.is_none());
}

#[test]
fn font_sync_unknown_paths_keep_existing_fallback_without_io() {
    let mut owner = ComicFontOwner::default();
    owner.adopt(available(&HashSet::new()));
    let current = owner.fonts().clone().unwrap();
    owner.loader_for_test(Arc::new(|_| {
        panic!("unknown paths must not reread the default font")
    }));
    let tx = pending(&mut owner, &["stale"]);
    owner.ensure(&["no-path".to_owned()], &HashMap::new());
    assert!(tx.send(available(&keys(&["stale"]))).is_err());
    assert!(Arc::ptr_eq(&current, owner.fonts().as_ref().unwrap()));
    assert!(owner.needed(&["no-path".to_owned()]).is_none());
}

#[test]
fn font_invalidation_discards_inflight_and_addition_keeps_set_and_loaded_keys() {
    for reason in [
        FontInvalidation::SourcesChanged,
        FontInvalidation::FontAdded,
    ] {
        let mut owner = ComicFontOwner::default();
        owner.adopt(available(&keys(&["missing"])));
        // Seed a loaded key too, to verify addition preserves both snapshot parts.
        if let Current::Ready { loaded, .. } = &mut owner.current {
            loaded.insert("loaded".to_owned());
        }
        let current = owner.fonts().clone().unwrap();
        let tx = pending(&mut owner, &["stale"]);
        owner.invalidate(reason);
        assert!(tx.send(available(&keys(&["stale"]))).is_err());
        owner.poll();
        assert!(owner.needed(&["missing".to_owned()]).is_some());
        match reason {
            FontInvalidation::SourcesChanged => assert!(matches!(owner.current, Current::Unloaded)),
            FontInvalidation::FontAdded => {
                assert!(Arc::ptr_eq(&current, owner.fonts().as_ref().unwrap()));
                assert_eq!(owner.loaded_keys(), &keys(&["loaded"]));
                assert!(owner.needed(&["loaded".to_owned()]).is_none());
            }
        }
    }
}

#[test]
fn font_missing_default_is_terminal_until_sources_invalidation() {
    let mut owner = ComicFontOwner::default();
    owner.loader_for_test(Arc::new(|requested| {
        assemble_fontset(requested, None, |_| panic!())
    }));
    owner.ensure(&[], &HashMap::new());
    owner.request(&egui::Context::default(), &["new".to_owned()]);
    assert!(owner.load.is_none());
    owner.ensure(&["new".to_owned()], &HashMap::new());
    assert!(owner.needed(&["new".to_owned()]).is_none());
    owner.invalidate(FontInvalidation::FontAdded);
    assert!(
        owner.needed(&[]).is_none(),
        "addition does not replace a terminal default"
    );
    owner.invalidate(FontInvalidation::SourcesChanged);
    assert!(owner.needed(&[]).is_some());
}

#[test]
fn font_assembly_parses_requested_bytes_and_preserves_default_fallback() {
    let definitions = egui::FontDefinitions::default();
    let bytes = definitions
        .font_data
        .values()
        .next()
        .unwrap()
        .font
        .as_ref()
        .as_ref()
        .to_vec();
    let default = comic_core::LoadedFont::from_bytes("ui", bytes.clone()).unwrap();
    let mut set = comic_core::FontSet::new();
    set.insert(default);
    let result = assemble_fontset(
        &keys(&["good", "bad", "absent"]),
        Some(set),
        |key| match key {
            "good" => Some(bytes.clone()),
            "bad" => Some(vec![0, 1, 2]),
            _ => None,
        },
    );
    assert_eq!(result.loaded, keys(&["good"]));
    assert_eq!(result.attempted, keys(&["good", "bad", "absent"]));
    let set = result.set.unwrap();
    assert_eq!(set.get("unknown").unwrap().key, "ui");
}
