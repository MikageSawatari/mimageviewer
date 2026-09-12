"""Extract current dispatch/commit/prewarm methods into round3's boundary stubs.

No app/device/Shell/real DB is started. Current production method bodies are
unchanged. The mock dispatch gate is closed, as it is for a modal or no foreground.
"""
from pathlib import Path
here=Path(__file__).resolve().parent
old=(here.parent/'round3/routing_probe.py').read_text(encoding='utf-8')
prefix,_=old.split('for name in [',1)
prefix=prefix.replace('target/review-v350-round3','target/review-v350-round4')
exec(prefix)
parts[0]=parts[0].replace('items:Vec<PathBuf>,cached_handlers:',
                        'association_prewarm_queue:std::collections::VecDeque<String>, items:Vec<PathBuf>,cached_handlers:')
parts[2]=parts[2].replace('fn is_idle(&self)->bool{true}',
                        'fn suppress_pending_actions(&mut self){} fn is_idle(&self)->bool{true}')
parts[2]=parts[2].replace('struct RingApp {',r'''
struct PadAction;
struct GamepadFrameBatch {now:std::time::Instant,actions:Vec<PadAction>,saw_input_event:bool,session_ended:bool}
struct GamepadDispatchOutcome {nav:Option<String>,dispatched:bool,dispatch_allowed:bool,saw_input_event:bool}
struct RingApp {
''')
parts[2]=parts[2].replace('impl RingApp {',r'''
impl RingApp {
    fn gamepad_dispatch_allowed(&self,_:&egui::Context)->bool {false}
    fn current_input_surface(&self)->u8 {0}
    fn note_input_surface(&mut self,_:u8){}
    fn consume_gamepad_directional_neutral_gate(&mut self,_:std::time::Instant)->bool {false}
    fn dispatch_gamepad_button(&mut self,_:&egui::Context,_:PadAction)->Option<String>{None}
    fn dispatch_gamepad_analog(&mut self,_:&egui::Context,_:std::time::Instant)->bool{false}
    fn reset_gamepad_continuous_steps(&mut self,_:std::time::Instant){}
''')
for name in ['dispatch_gamepad_batch','commit_ring_picker','finalize_live_picker_ratings','commit_live_picker_undo']:
    parts.append(method('src/app/gamepad_input.rs',name))
parts.append(r'''
}
fn main(){
    let exts=[".jpg",".png",".gif",".bmp",".tif",".avif",".heic",".jxl",".webp",".jpeg"];
    let mut app=Associations {
        association_prewarm:None,association_prewarm_generation:None,association_prewarm_queue:Default::default(),items_generation:1,
        items:exts.iter().enumerate().map(|(i,x)|PathBuf::from(format!("{i}{x}"))).collect(),
        cached_handlers:exts.iter().map(|x|(x.to_string(),vec!["old-handler".to_owned()])).collect(),
    };
    let ctx=egui::Context::default();
    for _ in 0..8 {let _=ctx.run(Default::default(), |_|{});}
    assert!(!ctx.has_requested_repaint());
    app.poll_association_handler_prewarm();
    let result=app.association_prewarm.take().unwrap().recv().unwrap();
    println!("first worker complete: delivered={} queued={} UI_repaint_requested={} ninth={:?}",
        result.len(),app.association_prewarm_queue.len(),ctx.has_requested_repaint(),app.cached_handlers[".webp"]);
    assert_eq!(result.len(),8);
    assert_eq!(app.association_prewarm_queue.len(),2);
    assert!(!ctx.has_requested_repaint());
    let(tx,rx)=mpsc::channel();tx.send(result).unwrap();app.association_prewarm=Some(rx);
    app.poll_association_handler_prewarm();
    println!("one manual frame to drain result: queued={} next_worker_started={} UI_repaint_requested={}",
        app.association_prewarm_queue.len(),app.association_prewarm.is_some(),ctx.has_requested_repaint());
    assert!(app.association_prewarm.is_none());
    app.poll_association_handler_prewarm();
    let result=app.association_prewarm.take().unwrap().recv().unwrap();
    let(tx,rx)=mpsc::channel();tx.send(result).unwrap();app.association_prewarm=Some(rx);
    app.poll_association_handler_prewarm();
    println!("control with repeated manual polls: ninth={:?} remaining={}",app.cached_handlers[".webp"],app.association_prewarm_queue.len());
    assert_eq!(app.cached_handlers[".webp"],vec!["new-handler"]);

    for projected in ["B","A"] {
        let mut app=RingApp {
            ring_picker:Some(RingPickerState {context:RingShortcutContext::ImageFullscreen,
                dirty_rows:vec![RingPickerRowId::ContainerRating],item_rating:0,container_rating:4,
                original:RingPickerOriginalState {item_rating_records:vec![],container_rating:1}}),
            gamepad_state:Pad,gamepad_favorite_picker:None,gamepad_location_picker:None,gamepad_video_marker_picker:None,
            rating_session_writes:HashMap::new(),global_search:Search{active:false},
            items_are_rating_view:false,items_are_global_search_view:false,checked:HashSet::new(),
            current_folder_rating_cache:Some(if projected=="A" {2} else {4}),current_folder:projected.into(),
            ratings:HashMap::from([("A".into(),2),("B".into(),4)]),undo:vec![],
        };
        let outcome=app.dispatch_gamepad_batch(&ctx,GamepadFrameBatch {
            now:std::time::Instant::now(),actions:vec![],saw_input_event:false,session_ended:true,
        });
        println!("picker belongs to B; terminal batch dispatched in {projected}; dispatch_allowed={}; A={} B={} undo={:?}",
            outcome.dispatch_allowed,app.ratings["A"],app.ratings["B"],app.undo);
        assert_eq!(app.ratings["A"],if projected=="A"{4}else{2});
        assert_eq!(app.undo,vec![(projected.into(),1,4)]);
    }
}
''')
footer=old[old.index('probe=out/'):].replace('routing_probe.rs','flow_probe.rs')
exec(footer)
