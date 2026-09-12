"""Native overlay teardown routing; no HWND, player, app or profile is started.

Use the current commit/clear/setter bodies with a retained overlay boolean in
place of a native player. Mounting mirrors the registry's swap/restore contract.
"""
from pathlib import Path
here=Path(__file__).resolve().parent
undo=(here/'undo_probe.py').read_text(encoding='utf-8')
prefix,_=undo.split('\nfor name in [',1)
exec(prefix)
parts[2]=parts[2].replace('enum RingPickerRowId {ItemRating,',
    'enum RingPickerRowId {VideoVolume,VideoPlaybackSpeed,VideoContinuousMode,ItemRating,')
parts[2]=parts[2].replace('struct RingPickerState {',
    'struct RingPickerState { video_volume:f64,video_playback_speed:f64,video_continuous_mode:u8,')
parts[2]=parts[2].replace('struct Settings {', 'struct Settings { video_volume:f64,')
parts[2]=parts[2].replace('struct FixtureContext {',
    'struct FixtureContext { fs_cache:HashMap<usize,FsCacheEntry>,')
parts[2]=parts[2].replace('struct App {',
    'struct App { fs_cache:HashMap<usize,FsCacheEntry>, video_playback_speed:f64,video_continuous_mode:u8,')
parts[2]=parts[2].replace('std::mem::swap(&mut self.fullscreen_idx,&mut bundle.fullscreen_idx);',
    'std::mem::swap(&mut self.fullscreen_idx,&mut bundle.fullscreen_idx);\n'
    'std::mem::swap(&mut self.fs_cache,&mut bundle.fs_cache);')
parts[2]=parts[2].replace('fn clear_native_video_picker_overlay(&mut self,_:&egui::Context){}','')
parts[2]=parts[2].replace('fn apply_video_picker_state(&mut self,_:&egui::Context,_:usize,_:RingPickerState){unreachable!()}','')
parts[2]=parts[2].replace('impl App {',r'''
mod video {pub mod native_presenter {pub struct NativeOverlayRingPicker;}}
struct Player(std::sync::Arc<std::sync::atomic::AtomicBool>);
impl Player {
    fn set_native_ring_picker_overlay(&self,overlay:Option<video::native_presenter::NativeOverlayRingPicker>){
        self.0.store(overlay.is_some(),std::sync::atomic::Ordering::SeqCst);
    }
}
enum FsCacheEntry { Video {player:Player}, Image }
impl App {
    fn request_native_video_hud_repaint(&self,_:&egui::Context){}
    fn handle_native_video_set_volume_command(&mut self,_:&egui::Context,_:usize,_:f64,_:bool){}
    fn handle_video_playback_speed_command(&mut self,_:&egui::Context,_:usize,_:f64){}
    fn set_video_continuous_mode_common(&mut self,_:&egui::Context,_:usize,_:u8){}
''')
for name in ['dispatch_gamepad_batch','commit_ring_picker','preview_ring_container_rating',
             'finalize_live_picker_ratings','commit_live_picker_undo','apply_ring_picker_state',
             'apply_ring_picker_state_in_owner','picker_owner_fullscreen_idx','apply_image_picker_state',
             'picker_post_filter_params','preview_picker_post_filter_selection',
             'apply_picker_post_filter','apply_picker_upscale_model','apply_video_picker_state',
             'clear_native_video_picker_overlay']:
    parts.append(method('src/app/gamepad_input.rs',name))
parts.append(method('src/app/native_video.rs','set_native_video_ring_picker_overlay'))
for name in ['capture_container_rating_undo_for_target','capture_adjust_full','capture_adjust_full_inner']:
    parts.append(method('src/undo_ops.rs',name))
parts.append(r'''
}
fn main() {
    for projected in ["B","A"] {
        let visible=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let b_cache=HashMap::from([(0,FsCacheEntry::Video{player:Player(visible.clone())})]);
        let b_pages=HashMap::from([(0,adjustment::AdjustParams::default())]);
        let mut contexts=HashMap::new();
        let current_cache=if projected=="B" {b_cache} else {
            contexts.insert(Owner("B"),FixtureContext{fs_cache:b_cache,folder:"B".into(),fullscreen_idx:Some(0),pages:b_pages.clone()});
            HashMap::new()
        };
        let ctx=egui::Context::default();
        let mut app=App {
            fs_cache:current_cache,video_playback_speed:1.0,video_continuous_mode:0,contexts,
            ring_picker:Some(RingPickerState{owner:Owner("B"),anchor:"B".into(),
                video_volume:0.5,video_playback_speed:1.0,video_continuous_mode:0,
                context:RingShortcutContext::VideoFullscreen,dirty_rows:vec![],
                original:RingPickerOriginalState{item_rating_records:vec![],container_rating:0,container_rating_target:None,post_filter:PostFilter::None,colorize:Default::default()},
                item_rating:0,container_rating:0,spread_mode:0,reading_flow:0,reading_direction:0,fit_mode:Fit,post_filter:PostFilter::None,upscale_model_key:None}),
            gamepad_state:Pad,rating_session_writes:HashMap::new(),global_search:Search{active:false},
            items_are_rating_view:false,items_are_global_search_view:false,checked:HashSet::new(),
            rating_db:None,rating_undo:vec![],current_folder:projected.into(),fullscreen_idx:if projected=="B" {Some(0)} else {None},
            settings:Settings{video_volume:0.5,fullscreen_fit_mode:Fit,global_preset:Default::default()},
            spread_mode:0,reading_flow:0,reading_direction:0,adjustment_page_params:b_pages,
            adjustment_favorite_params:HashMap::new(),durable_params:HashMap::new(),adjustment_undo:vec![],
        };
        app.dispatch_gamepad_batch(&ctx,GamepadFrameBatch{now:std::time::Instant::now(),actions:vec![],saw_input_event:false,session_ended:true});
        let native_visible=visible.load(std::sync::atomic::Ordering::SeqCst);
        println!("video picker belongs to B; terminal in {projected}: App picker closed={}, B native overlay still visible={native_visible}",app.ring_picker.is_none());
        assert!(app.ring_picker.is_none());
        assert_eq!(native_visible,projected=="A");
    }
}
''')
footer=old[old.index('probe=out/'):].replace('flow_probe.rs','overlay_probe.rs')
exec(footer)
