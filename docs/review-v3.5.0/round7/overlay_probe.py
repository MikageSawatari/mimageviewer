"""Re-run round6's retained-HUD probe against the T02 owner teardown.

Extract current production commit/dispatch/clear/setter bodies; only HWND/player,
DB and registry mount boundaries are simulated. No app or user profile is used.
"""
from pathlib import Path

here = Path(__file__).resolve().parent
source = (here.parent/'round6/overlay_probe.py').read_text(encoding='utf-8')
source = source.replace("here/'undo_probe.py'", "here.parent/'round6/undo_probe.py'")
source = source.replace('exec(prefix)', '''exec(prefix)
out=root/'target/review-v350-round7'
out.mkdir(exist_ok=True)
parts[2]=parts[2].replace('pub struct PageAdjustmentTarget {pub page_key:String}',
    'pub struct PageAdjustmentTarget {pub page_key:String,pub idx_hint:Option<usize>}')
parts[2]=parts[2].replace('impl App {', ''' + repr('''impl App {
    fn page_adjustment_target_for_idx(&self,idx:usize)->Option<app::PageAdjustmentTarget> {
        Some(app::PageAdjustmentTarget{page_key:self.current_folder.clone(),idx_hint:Some(idx)})
    }
''') + ''')
''')
source = source.replace("['capture_container_rating_undo_for_target','capture_adjust_full'",
    "['page_adjust_undo_scope','capture_container_rating_undo_for_target','capture_adjust_full'")
source = source.replace('for projected in ["B","A"]', 'for projected in ["B","A","C","closed"]')
source = source.replace('let current_cache=if projected=="B"', 'let mut current_cache=if projected=="B"')
source = source.replace('let ctx=egui::Context::default();', '''
        if projected=="closed" { contexts.clear(); }
        let other_visible=std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        if projected=="C" {current_cache.insert(0,FsCacheEntry::Video{player:Player(other_visible.clone())});}
        let ctx=egui::Context::default();''')
source = source.replace('fullscreen_idx:if projected=="B"', 'fullscreen_idx:if projected=="B" || projected=="C"')
source = source.replace('assert_eq!(native_visible,projected=="A");', '''
        assert_eq!(native_visible,projected=="closed");
        assert!(other_visible.load(std::sync::atomic::Ordering::SeqCst),"unrelated player must not be cleared");''')
exec(compile(source, str(here/'overlay_probe.py'), 'exec'))
