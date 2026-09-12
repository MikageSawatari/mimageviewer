"""Production method bodies, with in-memory DB/Undo/Shell/UI boundaries.

Checks dispatch decisions, not actual HWND, device, SQLite or installed handlers.
Does not start the application or touch the normal profile.
"""
from pathlib import Path
import re
import subprocess
root = Path(__file__).resolve().parents[3]
out = root / "target/review-v350-round3"
out.mkdir(exist_ok=True)
def method(path, name):
    src = (root/path).read_text(encoding="utf-8")
    match = re.search(r"^    (?:pub\(crate\) )?fn " + name + r"\(",src,re.M)
    assert match,name
    return src[match.start():src.index("\n    }",match.start())+6]
parts = [r'''
#![allow(dead_code)]
use std::{collections::{HashMap,HashSet},path::PathBuf,sync::mpsc};
mod external_tool {
    pub struct LaunchTarget<'a>(&'a std::path::Path);
    impl<'a> LaunchTarget<'a> {
        pub fn from_grid_item(item:Option<&'a std::path::PathBuf>)->Self { Self(item.unwrap()) }
        pub fn real_file(self)->Result<&'a std::path::Path,()> {Ok(self.0)}
    }
}
mod open_with {pub fn enumerate_handlers(_: &str)->Vec<String>{vec!["new-handler".into()]}}
struct Associations {
    association_prewarm:Option<mpsc::Receiver<Vec<(String,Vec<String>)>>>,
    association_prewarm_generation:Option<u64>, items_generation:u64,
    items:Vec<PathBuf>,cached_handlers:HashMap<String,Vec<String>>,
}
impl Associations {
''',method("src/app.rs","poll_association_handler_prewarm"),r'''
}
#[derive(Clone,Copy,PartialEq)] enum RingShortcutContext {Grid,ImageFullscreen,VideoFullscreen}
#[derive(Clone,Copy,PartialEq)] enum RingPickerRowId {ItemRating,ContainerRating}
#[derive(Clone)] struct RatingChange {path_key:String, source_path:PathBuf,meta:Option<()>,xmp_target:Option<PathBuf>,before:u8,after:u8}
struct RatingTarget {path_key:String,source_path:PathBuf,meta:Option<()>,xmp_target:Option<PathBuf>,before:u8,session_generation:u64}
struct RingPickerOriginalState {item_rating_records:Vec<RatingTarget>,container_rating:u8}
struct RingPickerState {context:RingShortcutContext,dirty_rows:Vec<RingPickerRowId>,original:RingPickerOriginalState,item_rating:u8,container_rating:u8}
struct RatingWrite {generation:u64,stars:u8}
struct Search {active:bool}
#[derive(Default)] struct Pad;
impl Pad {fn is_idle(&self)->bool{true} fn clear(&mut self){} fn cancel_west_ring(&mut self){} fn require_directional_neutral(&mut self){}}
struct RingApp {
    ring_picker:Option<RingPickerState>,gamepad_state:Pad,
    gamepad_favorite_picker:Option<()>,gamepad_location_picker:Option<()>,gamepad_video_marker_picker:Option<()>,
    rating_session_writes:HashMap<String,RatingWrite>,global_search:Search,
    items_are_rating_view:bool,items_are_global_search_view:bool,checked:HashSet<usize>,
    current_folder_rating_cache:Option<u8>,current_folder:String,
    ratings:HashMap<String,u8>,undo:Vec<(String,u8,u8)>,
}
impl RingApp {
    fn clear_native_video_picker_overlay(&mut self,_:&egui::Context){}
    fn apply_picker_item_rating_targets(&mut self,_:&RingPickerState,_:u8)->(Vec<usize>,Option<String>){unreachable!("container-only fixture")}
    fn report_rating_write_error(&mut self,_:&str){panic!("unexpected write error")}
    fn refresh_global_search_hit_stars(&mut self,_:&[usize]){}
    fn refresh_rating_view_after_rating_changes(&mut self,_:&[(String,u8)]){}
    fn rebuild_items_from_global_search(&mut self){}
    fn rebuild_visible_indices(&mut self){}
    fn preview_current_folder_rating(&mut self,stars:u8)->Result<bool,String>{
        self.ratings.insert(self.current_folder.clone(),stars);self.current_folder_rating_cache=Some(stars);Ok(true)
    }
    fn capture_container_rating_undo(&mut self,before:u8,after:u8){self.undo.push((self.current_folder.clone(),before,after));}
    fn push_rating_undo_entry(&mut self,_:Vec<RatingChange>,_:String){}
    fn show_container_rating_toast(&mut self,_:u8){}
    fn apply_ring_picker_state(&mut self,_:&egui::Context,_:RingPickerState){}
''']
for name in ["end_gamepad_input_session","commit_ring_picker","finalize_live_picker_ratings","commit_live_picker_undo"]:
    parts.append(method("src/app/gamepad_input.rs",name))
parts.append(r'''
}
fn main(){
    let exts=[".jpg",".png",".gif",".bmp",".tif",".avif",".heic",".jxl",".webp"];
    let mut app=Associations {
        association_prewarm:None,association_prewarm_generation:None,items_generation:0,
        items:exts.iter().enumerate().map(|(i,x)|PathBuf::from(format!("{i}{x}"))).collect(),
        cached_handlers:exts.iter().map(|x|(x.to_string(),vec!["old-handler".to_owned()])).collect(),
    };
    for generation in 1..=3 {
        app.items_generation=generation;app.poll_association_handler_prewarm();
        // Wait for only this deterministic stub worker; then restore its message for production poll.
        let result=app.association_prewarm.take().unwrap().recv().unwrap();
        let selected:Vec<_>=result.iter().map(|p|p.0.clone()).collect();
        let(tx,rx)=mpsc::channel();tx.send(result).unwrap();app.association_prewarm=Some(rx);
        app.poll_association_handler_prewarm();
        println!("association reload={generation}: selected={selected:?}; ninth={:?}",app.cached_handlers[".webp"]);
        assert_eq!(app.cached_handlers[".webp"],vec!["old-handler"]);
    }
    let ctx=egui::Context::default();
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
        app.end_gamepad_input_session(&ctx);
        println!("ring opened in B (before=1, preview=4), end in {projected}: A={} B={} undo={:?}",app.ratings["A"],app.ratings["B"],app.undo);
        assert_eq!(app.ratings["A"], if projected=="A" {4} else {2});
        assert_eq!(app.undo,vec![(projected.into(),1,4)]);
    }
}
''')
probe=out/"routing_probe.rs"
probe.write_text("\n".join(parts),encoding="utf-8")
egui=max((root/"target/debug/deps").glob("libegui-*.rlib"),key=lambda p:p.stat().st_mtime)
exe=probe.with_suffix(".exe")
subprocess.run(["rustc","--edition=2024",str(probe),"-L",f"dependency={root/'target/debug/deps'}","--extern",f"egui={egui}","-o",str(exe)],check=True)
result=subprocess.run([str(exe)],capture_output=True,text=True,check=True)
Path(__file__).with_suffix(".log").write_text(result.stdout,encoding="utf-8")
print(result.stdout,end="")
