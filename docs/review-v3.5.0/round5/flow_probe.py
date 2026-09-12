"""Review-only extraction of current production control flow.

Keep dispatch/commit/prewarm/image-apply method bodies unchanged. Substitute
HWND routing, Shell enumeration, rendering and durable stores with explicit
in-memory boundaries. No application/device or normal profile is started.
"""
from pathlib import Path
import re
import subprocess

root = Path(__file__).resolve().parents[3]
out = root / "target/review-v350-round5"
out.mkdir(exist_ok=True)

def method(path, name):
    source = (root / path).read_text(encoding="utf-8")
    match = re.search(r"^    (?:pub\(crate\) )?fn " + name + r"(?:<[^\n]+>)?\(", source, re.M)
    assert match, name
    return source[match.start():source.index("\n    }", match.start()) + 6]

parts = [r'''
#![allow(dead_code)]
use std::{collections::{HashMap,HashSet},path::{Path,PathBuf},sync::mpsc};
mod external_tool {
    pub struct LaunchTarget<'a>(&'a PathBuf);
    use super::*;
    impl<'a> LaunchTarget<'a> {
        pub fn from_grid_item(item:Option<&'a PathBuf>)->Self {Self(item.unwrap())}
        pub fn real_file(self)->Result<&'a Path,()> {Ok(self.0)}
    }
}
mod open_with {pub fn enumerate_handlers(_: &str)->Vec<String>{vec!["new-handler".into()]}}
struct Associations {
    association_prewarm:Option<mpsc::Receiver<Vec<(String,Vec<String>)>>>,
    association_prewarm_generation:Option<u64>, items_generation:u64,
    association_prewarm_queue:std::collections::VecDeque<String>,
    items:Vec<PathBuf>,cached_handlers:HashMap<String,Vec<String>>,
}
impl Associations {
''', method("src/app.rs", "poll_association_handler_prewarm"), r'''
}
mod colorize {
    #[derive(Clone,Debug,Default,PartialEq)] pub struct ColorizeParams {pub enabled:bool}
    pub enum ColorizePalette {Legacy4Color,LegacySkin}
    impl ColorizeParams {pub fn enable_with_palette(&mut self,_:ColorizePalette){self.enabled=true;}}
}
#[derive(Clone,Copy,Debug,Default,PartialEq)] enum PostFilter {#[default] None, BSelected, COriginal, PseudoColor4,PseudoColorSkin}
impl PostFilter {fn display_label(self)->&'static str {"fixture filter"}}
mod adjustment {
    use super::*;
    #[derive(Clone,Debug,Default,PartialEq)] pub struct AdjustParams {
        pub post_filter:PostFilter,pub colorize:colorize::ColorizeParams,pub upscale_model:Option<String>,
    }
    pub fn upscale_model_label(_:Option<&str>)->String {"fixture AI".into()}
}
mod ui_fullscreen {
    #[derive(Clone,Copy)] pub enum AdjustScope {PageOverride,FavoriteDefault(u64),Global}
    impl AdjustScope {pub fn label(self)->&'static str {"page"}}
}
mod rating_db {pub type RatingMeta = ();}
mod ring_shortcut {
    use super::*;
    #[derive(Clone,Debug)] pub struct RingPickerContainerTarget {pub path_key:String,pub source_path:PathBuf,pub meta:rating_db::RatingMeta}
}
mod app {#[derive(Clone,Debug)] pub struct PageAdjustmentTarget {pub page_key:String}}
#[derive(Clone,Copy,Debug,PartialEq)] enum RingShortcutContext {Grid,ImageFullscreen,VideoFullscreen}
#[derive(Clone,Copy,Debug,PartialEq)] enum RingPickerRowId {ItemRating,ContainerRating,SpreadMode,ReadingFlow,ReadingDirection,FitMode,PostFilter,UpscaleModel}
#[derive(Clone,Copy,Default,PartialEq)] struct Fit;
impl Fit {fn effective_for_flow(self,_:u8)->Self {self}}
#[derive(Clone)] struct RatingChange {path_key:String, source_path:PathBuf,meta:Option<()>,xmp_target:Option<PathBuf>,before:u8,after:u8}
struct RatingTarget {path_key:String,source_path:PathBuf,meta:Option<()>,xmp_target:Option<PathBuf>,before:u8,session_generation:u64}
struct RingPickerOriginalState {
    item_rating_records:Vec<RatingTarget>,container_rating:u8,
    container_rating_target:Option<ring_shortcut::RingPickerContainerTarget>,
    post_filter:PostFilter,colorize:colorize::ColorizeParams,
}
struct RingPickerState {
    context:RingShortcutContext,dirty_rows:Vec<RingPickerRowId>,original:RingPickerOriginalState,
    item_rating:u8,container_rating:u8,spread_mode:u8,reading_flow:u8,reading_direction:u8,
    fit_mode:Fit,post_filter:PostFilter,upscale_model_key:Option<String>,
}
struct RatingWrite {generation:u64,stars:u8}
struct Search {active:bool}
struct Pad;
impl Pad {fn suppress_pending_actions(&mut self){} fn cancel_west_ring(&mut self){} fn require_directional_neutral(&mut self){}}
struct PadAction;
struct GamepadFrameBatch {now:std::time::Instant,actions:Vec<PadAction>,saw_input_event:bool,session_ended:bool}
struct GamepadDispatchOutcome {nav:Option<String>,dispatched:bool,dispatch_allowed:bool,saw_input_event:bool}
struct Db(HashMap<String,u8>);
impl Db {fn get(&self,key:&str)->u8 {*self.0.get(key).unwrap_or(&0)}}
#[derive(Clone,Debug)] enum AdjustUndoScope {Page(usize),Favorite(u64),Global,PageKey(app::PageAdjustmentTarget)}
#[derive(Clone,Debug)] struct AdjustmentChange {scope:AdjustUndoScope,before:Option<adjustment::AdjustParams>,after:Option<adjustment::AdjustParams>}
struct Settings {fullscreen_fit_mode:Fit,global_preset:adjustment::AdjustParams}
struct App {
    ring_picker:Option<RingPickerState>,gamepad_state:Pad,
    rating_session_writes:HashMap<String,RatingWrite>,global_search:Search,
    items_are_rating_view:bool,items_are_global_search_view:bool,checked:HashSet<usize>,
    rating_db:Option<Db>,rating_undo:Vec<(String,u8,u8)>,current_folder:String,
    fullscreen_idx:Option<usize>,settings:Settings,spread_mode:u8,reading_flow:u8,reading_direction:u8,
    adjustment_page_params:HashMap<usize,adjustment::AdjustParams>,
    adjustment_favorite_params:HashMap<u64,adjustment::AdjustParams>,
    durable_params:HashMap<String,adjustment::AdjustParams>,adjustment_undo:Vec<AdjustmentChange>,
}
impl App {
    fn gamepad_dispatch_allowed(&self,_:&egui::Context)->bool {false} // modal/no foreground fixture
    fn current_input_surface(&self)->u8 {0}
    fn note_input_surface(&mut self,_:u8){}
    fn consume_gamepad_directional_neutral_gate(&mut self,_:std::time::Instant)->bool {false}
    fn dispatch_gamepad_button(&mut self,_:&egui::Context,_:PadAction)->Option<String>{None}
    fn dispatch_gamepad_analog(&mut self,_:&egui::Context,_:std::time::Instant)->bool{false}
    fn reset_gamepad_continuous_steps(&mut self,_:std::time::Instant){}
    fn clear_native_video_picker_overlay(&mut self,_:&egui::Context){}
    fn apply_picker_item_rating_targets(&mut self,_:&RingPickerState,_:u8)->(Vec<usize>,Option<String>){unreachable!("no item rating in fixture")}
    fn report_rating_write_error(&mut self,error:&str){panic!("{error}")}
    fn refresh_global_search_hit_stars(&mut self,_:&[usize]){}
    fn refresh_rating_view_after_rating_changes(&mut self,_:&[(String,u8)]){}
    fn rebuild_items_from_global_search(&mut self){}
    fn rebuild_visible_indices(&mut self){}
    fn write_container_rating_for_target(&mut self,key:&str,_:&Path,_:&(),stars:u8,_:bool)->Result<bool,String>{
        self.rating_db.as_mut().unwrap().0.insert(key.into(),stars);Ok(true)
    }
    fn push_rating_undo_entry(&mut self,changes:Vec<RatingChange>,_:String){
        self.rating_undo.extend(changes.into_iter().map(|c|(c.path_key,c.before,c.after)));
    }
    fn show_container_rating_toast(&mut self,_:u8){}
    fn apply_grid_picker_state(&mut self,_:RingPickerState){}
    fn apply_video_picker_state(&mut self,_:&egui::Context,_:usize,_:RingPickerState){unreachable!()}
    fn vertical_reading_supported_idx(&self,_:usize)->bool {false}
    fn apply_fullscreen_spread_mode(&mut self,_:&egui::Context,_:usize,_:u8){}
    fn set_reading_flow_for_fullscreen(&mut self,_:&egui::Context,_:usize,_:u8){}
    fn set_reading_direction_for_fullscreen(&mut self,_:&egui::Context,_:usize,_:u8){}
    fn set_fullscreen_fit_mode_for_current(&mut self,_:&egui::Context,_:usize,_:Fit){}
    fn effective_params(&self,idx:usize)->&adjustment::AdjustParams {&self.adjustment_page_params[&idx]}
    fn resolve_adjust_scope(&self,_:usize)->ui_fullscreen::AdjustScope {ui_fullscreen::AdjustScope::PageOverride}
    fn clear_caches_for_param_change(&mut self,_:usize,_:&adjustment::AdjustParams,_:&adjustment::AdjustParams){}
    fn clear_all_adjustment_and_ai_caches(&mut self,_:usize){}
    fn show_feedback_toast(&mut self,_:String){}
    fn write_params_for_scope(&mut self,idx:usize,_:ui_fullscreen::AdjustScope,params:adjustment::AdjustParams){
        self.durable_params.insert(self.current_folder.clone(),params.clone());
        self.adjustment_page_params.insert(idx,params);
    }
    fn stored_page_params_for_target(&self,_:&app::PageAdjustmentTarget)->Option<adjustment::AdjustParams> {None}
    fn page_path_key(&self,_:usize)->Option<String> {Some(self.current_folder.clone())}
    fn push_adjustment_undo_entry(&mut self,changes:Vec<AdjustmentChange>,_:String){self.adjustment_undo.extend(changes);}
''']
for name in ["dispatch_gamepad_batch", "commit_ring_picker", "preview_ring_container_rating",
             "finalize_live_picker_ratings", "commit_live_picker_undo", "apply_ring_picker_state",
             "apply_image_picker_state", "picker_post_filter_params",
             "preview_picker_post_filter_selection", "apply_picker_post_filter", "apply_picker_upscale_model"]:
    parts.append(method("src/app/gamepad_input.rs", name))
for name in ["capture_container_rating_undo_for_target", "capture_adjust_full", "capture_adjust_full_inner"]:
    parts.append(method("src/undo_ops.rs", name))
parts.append(r'''
}
fn association_case(count:usize,change_generation:bool) {
    let ctx=egui::Context::default();
    for _ in 0..8 {let _=ctx.run(Default::default(), |_|{});}
    assert!(!ctx.has_requested_repaint());
    let (wake_tx,wake_rx)=mpsc::channel();
    ctx.set_request_repaint_callback(move |_|{let _=wake_tx.send(());});
    let mut app=Associations {association_prewarm:None,association_prewarm_generation:None,
        association_prewarm_queue:Default::default(),items_generation:1,
        items:(0..count).map(|i|PathBuf::from(format!("file.ext{i}"))).collect(),cached_handlers:HashMap::new()};
    app.poll_association_handler_prewarm(&ctx);
    if change_generation {
        app.items_generation=2;
        app.items=(0..count).map(|i|PathBuf::from(format!("file.new{i}"))).collect();
    }
    let mut frames=0;
    while app.association_prewarm.is_some() || !app.association_prewarm_queue.is_empty() {
        wake_rx.recv_timeout(std::time::Duration::from_secs(5)).expect("no completion wake");
        let _=ctx.run(Default::default(), |_|{app.poll_association_handler_prewarm(&ctx);});
        frames+=1;
        assert!(frames<50);
    }
    let prefix=if change_generation {"new"} else {"ext"};
    for i in 0..count {assert_eq!(app.cached_handlers[&format!(".{prefix}{i}")],vec!["new-handler"]);}
    println!("prewarm count={count} generation_switch={change_generation}: complete from completion wakes only, frames={frames}");
}
fn ring_case(projected:&str,row:RingPickerRowId) {
    let p_b=adjustment::AdjustParams::default();
    let p_c=adjustment::AdjustParams {post_filter:PostFilter::COriginal,..Default::default()};
    let ctx=egui::Context::default();
    let mut app=App {
        ring_picker:Some(RingPickerState {
            context:RingShortcutContext::ImageFullscreen,dirty_rows:vec![RingPickerRowId::ContainerRating,row],
            original:RingPickerOriginalState {item_rating_records:vec![],container_rating:1,
                container_rating_target:Some(ring_shortcut::RingPickerContainerTarget{path_key:"B".into(),source_path:PathBuf::from("B"),meta:()}),
                post_filter:PostFilter::None,colorize:Default::default()},
            item_rating:0,container_rating:4,spread_mode:0,reading_flow:0,reading_direction:0,fit_mode:Fit,
            post_filter:PostFilter::BSelected,upscale_model_key:Some("B-selected-model".into())}),
        gamepad_state:Pad,rating_session_writes:HashMap::new(),global_search:Search{active:false},
        items_are_rating_view:false,items_are_global_search_view:false,checked:HashSet::new(),
        rating_db:Some(Db(HashMap::from([("A".into(),2),("B".into(),4),("C".into(),3)]))),rating_undo:vec![],
        current_folder:projected.into(),fullscreen_idx:if projected=="A" {None} else {Some(0)},
        settings:Settings {fullscreen_fit_mode:Fit,global_preset:Default::default()},
        spread_mode:0,reading_flow:0,reading_direction:0,
        adjustment_page_params:HashMap::from([(0,if projected=="C" {p_c.clone()} else {
            let mut preview=p_b.clone();
            if row==RingPickerRowId::PostFilter {preview.post_filter=PostFilter::BSelected;}
            preview
        })]),adjustment_favorite_params:HashMap::new(),
        durable_params:HashMap::from([("B".into(),p_b), ("C".into(),p_c)]),adjustment_undo:vec![],
    };
    app.dispatch_gamepad_batch(&ctx,GamepadFrameBatch {
        now:std::time::Instant::now(),actions:vec![],saw_input_event:false,session_ended:true,
    });
    assert_eq!(app.rating_db.as_ref().unwrap().get("A"),2);
    assert_eq!(app.rating_db.as_ref().unwrap().get("B"),4);
    assert_eq!(app.rating_db.as_ref().unwrap().get("C"),3);
    assert_eq!(app.rating_undo,vec![("B".into(),1,4)]);
    println!("ring owned by B, terminal in {projected}, row={row:?}: rating target+Undo B correct; B durable={:?}; C durable={:?}; adjustment Undo={:?}",
        app.durable_params["B"],app.durable_params["C"],app.adjustment_undo);
    if projected=="C" {
        if row==RingPickerRowId::PostFilter {
            assert_eq!(app.durable_params["C"].post_filter,PostFilter::BSelected);
            assert_eq!(app.adjustment_undo[0].before.as_ref().unwrap().post_filter,PostFilter::None);
        } else {assert_eq!(app.durable_params["C"].upscale_model.as_deref(),Some("B-selected-model"));}
        assert_eq!(app.durable_params["B"],adjustment::AdjustParams::default());
    } else if projected=="A" {
        assert!(app.adjustment_undo.is_empty());
        assert_eq!(app.durable_params["B"],adjustment::AdjustParams::default());
    } else {
        assert_eq!(app.adjustment_undo.len(),1);
        if row==RingPickerRowId::PostFilter {assert_eq!(app.durable_params["B"].post_filter,PostFilter::BSelected);}
        else {assert_eq!(app.durable_params["B"].upscale_model.as_deref(),Some("B-selected-model"));}
    }
}
fn main() {
    for count in [0,1,8,9,17,25] {association_case(count,false);}
    association_case(17,true);
    for row in [RingPickerRowId::PostFilter,RingPickerRowId::UpscaleModel] {
        for projected in ["B","A","C"] {ring_case(projected,row);}
    }
}
''')
probe=out/"flow_probe.rs"
probe.write_text("\n".join(parts),encoding="utf-8")
egui=max((root/"target/debug/deps").glob("libegui-*.rlib"),key=lambda p:p.stat().st_mtime)
exe=probe.with_suffix(".exe")
subprocess.run(["rustc","--edition=2024",str(probe),"-L",f"dependency={root/'target/debug/deps'}",
                "--extern",f"egui={egui}","-o",str(exe)],check=True,timeout=600)
result=subprocess.run([str(exe)],capture_output=True,text=True,timeout=60)
Path(__file__).with_suffix(".log").write_text(result.stdout+result.stderr,encoding="utf-8")
print(result.stdout,end="")
print(result.stderr,end="")
result.check_returncode()
