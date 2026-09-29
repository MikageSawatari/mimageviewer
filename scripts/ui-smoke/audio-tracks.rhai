// Real app audio-track scenario. The runner supplies only the disposable multi.mkv.
// Native clicks use committed Response rectangles in the presenter; egui clicks
// use the registered music HUD widgets. The Hz value comes from the audio pump.

fn detached_video(s) {
    for window in s.windows {
        if window.role == "detached" && window.host_ready && window.media_kind == "video" {
            return window;
        }
    }
    ()
}

fn root_video(s) {
    for window in s.windows {
        if window.role == "root" && window.host_ready && window.media_kind == "video" {
            return window;
        }
    }
    ()
}

fn track_is(window, stream, tone) {
    if window == () { return false; }
    let track = window.audio_track;
    track.audio_tracks_len == 3
        && track.desired_stream_index == stream
        && track.applied_stream_index == stream
        && track.desired_generation == track.applied_generation
        && track.state == "Applied"
        && track.engine_state == "Playing"
        && (track.processed_frequency_hz - tone).abs() < 100.0
}

fn select_detached() {
    let window = detached_video(snapshot());
    if window == () { fail("detached video window is absent"); }
    let selected = select_window(window.window_id, window.context_serial);
    if selected.target_mode != "targeted" || !selected.current {
        fail("detached video owner was not selected");
    }
}

fn click_native_row(ordinal) {
    let button = click_native_audio_control("native_audio_button", 30000);
    if button.name != "native_audio_button" || button.token <= 0 {
        fail("native audio button did not resolve to a committed control");
    }
    let row_name = "native_audio_row_" + ordinal.to_string();
    let row = click_native_audio_control(row_name, 30000);
    if row.name != row_name || row.token <= 0 {
        fail("native audio row did not resolve to a committed control");
    }
}

log("audio-tracks: open disposable multi.mkv");
wait_until(|s| !s.is_fullscreen && s.items_len == 1 && s.pending_thumbs == 0, 30000);
select_root();
run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_until(|s| track_is(detached_video(s), 2, 880.0), 30000);
select_detached();
log("audio-tracks: default 880 Hz " + detached_video(snapshot()).audio_track.to_debug());

click_native_row(1);
wait_until(|s| track_is(detached_video(s), 1, 440.0), 30000);
log("audio-tracks: playing switch to 440 Hz " + detached_video(snapshot()).audio_track.to_debug());

run_action("VideoSeekStart");
wait_until(|s| track_is(detached_video(s), 1, 440.0), 30000);
run_action("VideoPlayPause");
wait_until(|s| {
    let window = detached_video(s);
    window != () && window.audio_track.engine_state == "Paused"
}, 30000);
click_native_row(3);
wait_until(|s| {
    let window = detached_video(s);
    if window == () { return false; }
    let track = window.audio_track;
    track.audio_tracks_len == 3 && track.state == "Applied"
        && track.desired_stream_index == 3 && track.applied_stream_index == 3
        && track.desired_generation == track.applied_generation
        && track.engine_state == "Paused"
}, 30000);
log("audio-tracks: paused switch applied " + detached_video(snapshot()).audio_track.to_debug());
run_action("VideoPlayPause");
wait_until(|s| track_is(detached_video(s), 3, 1320.0), 30000);
log("audio-tracks: resumed at 1320 Hz");

run_action("VideoCloseFullscreen");
wait_until(|s| detached_video(s) == () && !s.is_fullscreen, 30000);
select_root();
run_action("GridMoveFirst");
run_action("GridOpenSelected");
wait_until(|s| track_is(detached_video(s), 3, 1320.0), 30000);
select_detached();
log("audio-tracks: reopen remembered row 3");

// F12 off -> main viewer; F12 on -> a new detached presentation of this video.
run_action("ToggleDetachedViewerMode");
wait_until(|s| root_video(s) != () && detached_video(s) == (), 30000);
select_root();
run_action("ToggleDetachedViewerMode");
wait_until(|s| detached_video(s) != (), 30000);
select_detached();
run_action("VideoSeekStart");
wait_until(|s| track_is(detached_video(s), 3, 1320.0), 30000);
click_native_row(2);
wait_until(|s| track_is(detached_video(s), 2, 880.0), 30000);
log("audio-tracks: F12 detached HUD switch to 880 Hz");
click_native_row(1);
click_native_row(3);
wait_until(|s| track_is(detached_video(s), 3, 1320.0), 30000);
log("audio-tracks: consecutive HUD selections settled on row 3");
click_native_row(2);
wait_until(|s| track_is(detached_video(s), 2, 880.0), 30000);

run_action("ToggleDetachedViewerMode");
wait_until(|s| root_video(s) != () && detached_video(s) == (), 30000);
select_root();
run_action("VideoSeekStart");
wait_until(|s| track_is(root_video(s), 2, 880.0), 30000);
run_action("VideoToggleAudioMode");
wait_until(|s| s.music_view_active && s.audio_track.audio_mode
    && s.audio_track.audio_tracks_len == 3
    && !s.music_audio_track_popup_open, 30000);
click_widget("music_audio_track_button");
wait_until(|s| s.music_view_active && s.music_audio_track_popup_open, 30000);
click_widget("music_audio_track_row_3");
wait_until(|s| s.music_view_active && s.audio_track.audio_mode
    && s.audio_track.state == "Applied"
    && s.audio_track.desired_stream_index == 3
    && s.audio_track.applied_stream_index == 3
    && (s.audio_track.processed_frequency_hz - 1320.0).abs() < 100.0, 30000);
capture("audio-mode-hud");
log("audio-tracks: audio-mode HUD selected 1320 Hz " + snapshot().audio_track.to_debug());
