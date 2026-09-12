"""Current ring commit and Undo consumer, with context/DB/render boundaries.

Reuse round5's explicit type and boundary stubs. Extract the current production
owner wrapper, image apply, capture and restore bodies unchanged. Simulate the
documented mount/unmount contract; do not run the application or touch its profile.
"""
from pathlib import Path
here=Path(__file__).resolve().parent
old=(here.parent/'round5/flow_probe.py').read_text(encoding='utf-8')
prefix,_=old.split('for name in [',1)
prefix=prefix.replace('target/review-v350-round5','target/review-v350-round6')
exec(prefix)
parts[2]=parts[2].replace('struct RingPickerState {','struct RingPickerState { owner:String,anchor:String,')
parts[2]=parts[2].replace('struct App {',r'''
struct FixtureContext {folder:String,fullscreen_idx:Option<usize>,pages:HashMap<usize,adjustment::AdjustParams>}
struct App { contexts:HashMap<String,FixtureContext>,
''')
parts[2]=parts[2].replace('impl App {',r'''
impl App {
    // The registry mounts the named bundle for the closure, then restores the caller.
    // meta_undo is intentionally App-global, matching ViewerContextBundle in production.
    fn with_owner_viewer_context<R>(&mut self,owner:String,f:impl FnOnce(&mut Self)->R)->Option<R> {
        if owner==self.current_folder {return Some(f(self));}
        let mut bundle=self.contexts.remove(&owner)?;
        std::mem::swap(&mut self.current_folder,&mut bundle.folder);
        std::mem::swap(&mut self.fullscreen_idx,&mut bundle.fullscreen_idx);
        std::mem::swap(&mut self.adjustment_page_params,&mut bundle.pages);
        let result=f(self);
        std::mem::swap(&mut self.current_folder,&mut bundle.folder);
        std::mem::swap(&mut self.fullscreen_idx,&mut bundle.fullscreen_idx);
        std::mem::swap(&mut self.adjustment_page_params,&mut bundle.pages);
        self.contexts.insert(owner,bundle);
        Some(result)
    }
    fn current_ring_picker_anchor(&self,_:RingShortcutContext)->String {self.current_folder.clone()}
    fn set_page_params(&mut self,idx:usize,params:adjustment::AdjustParams) {
        self.write_params_for_scope(idx,ui_fullscreen::AdjustScope::PageOverride,params);
    }
    fn clear_page_params(&mut self,idx:usize){self.adjustment_page_params.remove(&idx);}
    fn restore_page_params_for_target(&mut self,_:&app::PageAdjustmentTarget,_:Option<adjustment::AdjustParams>){unreachable!()}
    fn set_favorite_default(&mut self,_:u64,_:adjustment::AdjustParams){unreachable!()}
    fn clear_favorite_default(&mut self,_:u64){unreachable!()}
    fn copy_params_to_global(&mut self,_:adjustment::AdjustParams){unreachable!()}
''')
# ViewerContextId is Copy in production. Use a Copy fixture identity rather than
# changing the source wrapper's `let owner = picker.owner` line.
parts[2]=parts[2].replace('owner:String','owner:Owner')
parts[2]=parts[2].replace('struct FixtureContext {',r'''
#[derive(Clone,Copy,Debug,PartialEq,Eq,Hash)] struct Owner(&'static str);
impl PartialEq<String> for Owner {fn eq(&self,s:&String)->bool{self.0==s}}
struct FixtureContext {
''')
parts[2]=parts[2].replace('contexts:HashMap<String,FixtureContext>','contexts:HashMap<Owner,FixtureContext>')
for name in ['dispatch_gamepad_batch','commit_ring_picker','preview_ring_container_rating',
             'finalize_live_picker_ratings','commit_live_picker_undo','apply_ring_picker_state',
             'apply_ring_picker_state_in_owner','picker_owner_fullscreen_idx','apply_image_picker_state',
             'picker_post_filter_params','preview_picker_post_filter_selection',
             'apply_picker_post_filter','apply_picker_upscale_model']:
    parts.append(method('src/app/gamepad_input.rs',name))
for name in ['capture_container_rating_undo_for_target','capture_adjust_full','capture_adjust_full_inner',
             'apply_adjustment_change_to_app']:
    parts.append(method('src/undo_ops.rs',name))
parts.append(r'''
}
fn fixture(projected:&str,row:RingPickerRowId)->App {
    let original=adjustment::AdjustParams::default();
    let mut preview=original.clone();
    if row==RingPickerRowId::PostFilter {preview.post_filter=PostFilter::BSelected;}
    let other=adjustment::AdjustParams {post_filter:PostFilter::COriginal,upscale_model:Some("C-original-model".into()),..Default::default()};
    let mut contexts=HashMap::new();
    if projected!="B" {contexts.insert(Owner("B"),FixtureContext{folder:"B".into(),fullscreen_idx:Some(0),pages:HashMap::from([(0,preview.clone())])});}
    App {
        contexts,
        ring_picker:Some(RingPickerState{owner:Owner("B"),anchor:"B".into(),
            context:RingShortcutContext::ImageFullscreen,dirty_rows:vec![row],
            original:RingPickerOriginalState {item_rating_records:vec![],container_rating:1,
                container_rating_target:None,post_filter:PostFilter::None,colorize:Default::default()},
            item_rating:0,container_rating:1,spread_mode:0,reading_flow:0,reading_direction:0,fit_mode:Fit,
            post_filter:PostFilter::BSelected,upscale_model_key:Some("B-selected-model".into())}),
        gamepad_state:Pad,rating_session_writes:HashMap::new(),global_search:Search{active:false},
        items_are_rating_view:false,items_are_global_search_view:false,checked:HashSet::new(),
        rating_db:Some(Db(HashMap::new())),rating_undo:vec![],current_folder:projected.into(),
        fullscreen_idx:if projected=="A" {None} else {Some(0)},
        settings:Settings{fullscreen_fit_mode:Fit,global_preset:Default::default()},
        spread_mode:0,reading_flow:0,reading_direction:0,
        adjustment_page_params:HashMap::from([(0,if projected=="B" {preview} else {other.clone()})]),
        adjustment_favorite_params:HashMap::new(),
        durable_params:HashMap::from([("B".into(),original),(projected.into(),if projected=="B" {Default::default()}else{other})]),
        adjustment_undo:vec![],
    }
}
fn main() {
    for row in [RingPickerRowId::PostFilter,RingPickerRowId::UpscaleModel] {
        for projected in ["B","A","C"] {
            let ctx=egui::Context::default();
            let mut app=fixture(projected,row);
            let other_before=app.durable_params.get(projected).cloned();
            app.dispatch_gamepad_batch(&ctx,GamepadFrameBatch {
                now:std::time::Instant::now(),actions:vec![],saw_input_event:false,session_ended:true,
            });
            let after_commit=app.durable_params["B"].clone();
            if row==RingPickerRowId::PostFilter {assert_eq!(after_commit.post_filter,PostFilter::BSelected);}
            else {assert_eq!(after_commit.upscale_model.as_deref(),Some("B-selected-model"));}
            if projected!="B" {assert_eq!(app.durable_params.get(projected).cloned(),other_before);}
            assert_eq!(app.current_folder,projected);
            assert_eq!(app.adjustment_undo.len(),1);
            let entry=app.adjustment_undo[0].clone();
            println!("row={row:?}, terminal dispatched in {projected}: commit B correct, undo scope={:?}",entry.scope);
            // apply_meta_undo/redo loop over this same App-global payload and call
            // this consumer directly; no owner remount occurs there.
            app.apply_adjustment_change_to_app(&entry,true);
            println!("  Undo in {projected}: B={:?}, foreground={:?}",app.durable_params["B"],app.durable_params[projected]);
            if projected=="B" {assert_eq!(app.durable_params["B"],adjustment::AdjustParams::default());}
            else {
                assert_eq!(app.durable_params["B"],after_commit,"original B was not undone");
                assert_eq!(app.durable_params[projected],adjustment::AdjustParams::default(),"unrelated foreground was overwritten");
            }
            app.apply_adjustment_change_to_app(&entry,false);
            assert_eq!(app.durable_params[projected],after_commit);
            println!("  Redo in {projected}: writes the B-selected params into {projected}");
        }
    }
}
''')
footer=old[old.index('probe=out/') :].replace('flow_probe.rs','undo_probe.rs')
exec(footer)
