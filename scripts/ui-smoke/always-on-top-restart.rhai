// Second process in the same disposable profile, without the OFF setup override.
fn root_style(s) {
    for host in s.host_styles {
        if host.role == "root" { return host; }
    }
    fail("root style is missing after restart");
}

wait_until(|s| {
    if s.items_len != 3 || !s.always_on_top { return false; }
    for host in s.host_styles {
        if host.role == "root" && host.topmost { return true; }
    }
    false
}, 30000);
let restored = snapshot();
log("always-on-top restart persisted=" + restored.always_on_top
    + " root=" + root_style(restored).to_debug());
capture("aot-restart-root");
