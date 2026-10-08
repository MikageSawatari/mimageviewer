"""Review-only extraction of U01 resolver and lookup, plus optimized CPU probe.

No application, database, device or normal profile is opened. Function bodies are
read verbatim from the current source. Small types substitute unrelated fields.
"""
from pathlib import Path

here = Path(__file__).resolve().parent
old = (here.parent/'round7/lookup_probe.py').read_text(encoding='utf-8')
prefix = old.split('\nparts = [', 1)[0]
exec(prefix)
OUT = ROOT/'target/review-v350-round8'
OUT.mkdir(parents=True, exist_ok=True)

parts = [r'''
#![allow(dead_code)]
use std::path::{Path,PathBuf};
use std::hint::black_box;
use std::time::Instant;
mod grid_item {
    use super::*;
    #[derive(Clone)]
    pub enum GridItem {Image(PathBuf), ZipImage{zip_path:PathBuf,entry_name:String},
        PdfPage{pdf_path:PathBuf,page_num:u32}, Other}
}
mod adjustment_db { use super::*;
''', function('src/adjustment_db.rs', 'normalize_path'),
function('src/adjustment_db.rs', 'zip_entry_key'), '}',
'mod edit_source { use super::*;',
function('src/edit_source.rs', 'page_key_for_grid_item'),
function('src/edit_source.rs', 'page_key_for_pdf'), '}',
r'''
mod app {
    #[derive(Debug,Clone,Copy,PartialEq,Eq)]
    pub enum PageIndexHint {Unresolved,At(usize),Absent}
}
use app::PageIndexHint;
#[derive(Clone,Debug)]
struct PageAdjustmentTarget {page_key:String,idx_hint:PageIndexHint}
#[derive(Clone,Debug)]
enum AdjustUndoScope {PageKey(PageAdjustmentTarget),Page(usize),Favorite(u64),Global}
struct AdjustmentChange {scope:AdjustUndoScope}
struct App {items:Vec<grid_item::GridItem>}
impl App {
''', function('src/app.rs', 'page_path_key', '    '),
function('src/app.rs', 'page_adjustment_indices', '    '),
function('src/undo_ops.rs', 'resolve_restore_scopes', '    '),
r'''
}
fn targets(scopes:Vec<AdjustUndoScope>)->Vec<AdjustmentChange> {
    scopes.into_iter().map(|scope|AdjustmentChange{scope}).collect()
}
fn hits(app:&App,changes:&[AdjustmentChange])->Vec<Vec<usize>> {
    changes.iter().filter_map(|c| match &c.scope {
        AdjustUndoScope::PageKey(t)=>Some(app.page_adjustment_indices(t)), _=>None
    }).collect()
}
fn lifecycle() {
    use grid_item::GridItem::*;
    let a=Image(PathBuf::from("C:/pictures/a.jpg"));
    let b=ZipImage{zip_path:PathBuf::from("C:/pictures/book.zip"),entry_name:"Chapter/Page.JPG".into()};
    let c=PdfPage{pdf_path:PathBuf::from("C:/pictures/book.pdf"),page_num:3};
    let mut app=App{items:vec![a.clone(),b.clone(),c.clone()]};
    let changes:Vec<_>=(0..3).map(|i|AdjustmentChange{scope:AdjustUndoScope::PageKey(
        PageAdjustmentTarget{page_key:app.page_path_key(i).unwrap(),idx_hint:PageIndexHint::Unresolved})}).collect();
    let at=targets(app.resolve_restore_scopes(&changes));
    assert_eq!(hits(&app,&at),vec![vec![0],vec![1],vec![2]]);
    app.items=vec![Image(PathBuf::from("C:/unrelated/image.jpg"))];
    let absent=targets(app.resolve_restore_scopes(&at));
    assert_eq!(hits(&app,&absent),vec![vec![],vec![],vec![]]);
    app.items=vec![c,b,a];
    let moved=targets(app.resolve_restore_scopes(&absent));
    assert_eq!(hits(&app,&moved),vec![vec![2],vec![1],vec![0]]);
    assert_eq!(hits(&app,&at),vec![vec![],vec![1],vec![]],"stale At must never touch unrelated page");
    app.items.push(app.items[1].clone());
    let duplicates=targets(app.resolve_restore_scopes(&moved));
    assert_eq!(hits(&app,&duplicates),vec![vec![2],vec![1,3],vec![0]]);
    let globals=targets(app.resolve_restore_scopes(&[
        AdjustmentChange{scope:AdjustUndoScope::Global},
        AdjustmentChange{scope:AdjustUndoScope::Favorite(5)},
        AdjustmentChange{scope:AdjustUndoScope::Page(8)}]));
    assert!(matches!(globals[0].scope,AdjustUndoScope::Global));
    assert!(matches!(globals[1].scope,AdjustUndoScope::Favorite(5)));
    assert!(matches!(globals[2].scope,AdjustUndoScope::Page(8)));
    println!("lifecycle PASS: Image/ZIP/PDF; At -> Absent -> reordered At; stale At rejected; duplicate keys preserved; Global/Favorite/Page passthrough");
}
fn run(n:usize,m:usize,present:bool,resolve:bool)->f64 {
    let app=App{items:(0..n).map(|i| grid_item::GridItem::Image(
        PathBuf::from(format!("C:/Pictures/Photos/2026/ReleaseReview/Photo-{i:08}.jpg")))).collect()};
    let changes:Vec<_>=(0..m).map(|idx|AdjustmentChange{scope:AdjustUndoScope::PageKey(
        PageAdjustmentTarget{page_key:if present {app.page_path_key(idx).unwrap()}else{format!("absent-{idx}")},
        idx_hint:PageIndexHint::Unresolved})}).collect();
    let start=Instant::now();
    let changes=if resolve {targets(app.resolve_restore_scopes(&changes))} else {changes};
    for c in &changes {
        if let AdjustUndoScope::PageKey(t)=&c.scope {
            for _ in 0..3 {assert_eq!(black_box(app.page_adjustment_indices(black_box(t))).len(),usize::from(present));}
        }
    }
    start.elapsed().as_secs_f64()*1000.0
}
fn main() {
    lifecycle();
    for (n,m,present) in [(10000,1,true),(10000,1000,true),(10000,5000,true),(10000,5000,false)] {
        let old=run(n,m,present,false);
        let new=run(n,m,present,true);
        println!("items={n}, changed={m}, present={present}, unresolved_ms={old:.3}, bulk_resolved_ms={new:.3}");
    }
}
''']
probe=OUT/'lookup_probe.rs'
probe.write_text('\n'.join(parts),encoding='utf-8')
binary=OUT/'lookup_probe.exe'
subprocess.run(['rustc','--edition=2024','-C','opt-level=3',str(probe),'-o',str(binary)],check=True)
subprocess.run([str(binary)],check=True)
