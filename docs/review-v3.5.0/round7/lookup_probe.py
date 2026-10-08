"""Measure the newly selected production page-key lookup path, without App or I/O.

Extract lookup/key-normalization bodies verbatim. Three lookups model Some(params)
Undo: restore_page_params_for_target + effective_params_for_target + set indices.
The baseline keeps idx_hint, as the prior index-based set_page_params path did.
This is a CPU-cost probe, not an end-to-end UI latency benchmark.
"""
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / 'target/review-v350-round7'
OUT.mkdir(parents=True, exist_ok=True)

def function(path, name, indent=''):
    source = (ROOT / path).read_text(encoding='utf-8')
    pattern = rf'(?m)^{indent}(?:pub(?:\(crate\))? )?fn {name}\b'
    match = re.search(pattern, source)
    assert match, (path, name)
    end = source.index('\n' + indent + '}', match.start()) + len(indent) + 2
    return source[match.start():end]

parts = [r'''
#![allow(dead_code)]
use std::path::{Path,PathBuf};
use std::hint::black_box;
use std::time::Instant;
mod grid_item {
    use super::*;
    pub enum GridItem {Image(PathBuf), ZipImage{zip_path:PathBuf,entry_name:String},
        PdfPage{pdf_path:PathBuf,page_num:u32}, Other}
}
mod adjustment_db {
    use super::*;
''', function('src/adjustment_db.rs', 'normalize_path'),
function('src/adjustment_db.rs', 'zip_entry_key'), '}',
'mod edit_source { use super::*;',
function('src/edit_source.rs', 'page_key_for_grid_item'),
function('src/edit_source.rs', 'page_key_for_pdf'), '}',
'''struct PageAdjustmentTarget {page_key:String, idx_hint:Option<usize>}
struct App {items:Vec<grid_item::GridItem>}
impl App {''',
function('src/app.rs', 'page_path_key', '    '),
function('src/app.rs', 'page_adjustment_indices', '    '),
r'''
}
fn run(n:usize,m:usize,hint:bool) -> f64 {
    let app=App{items:(0..n).map(|i| grid_item::GridItem::Image(
        PathBuf::from(format!("C:/Pictures/Photos/2026/ReleaseReview/Photo-{i:08}.jpg")))).collect()};
    let targets:Vec<_>=(0..m).map(|idx| PageAdjustmentTarget {
        page_key:app.page_path_key(idx).unwrap(), idx_hint:hint.then_some(idx)}).collect();
    let started=Instant::now();
    for t in &targets {
        for _ in 0..3 { assert_eq!(black_box(app.page_adjustment_indices(black_box(t))).len(),1); }
    }
    started.elapsed().as_secs_f64()*1000.0
}
fn main() {
    for (n,m) in [(1000,1),(10000,1),(10000,100),(10000,1000),(10000,5000)] {
        let before=run(n,m,true);
        let after=run(n,m,false);
        println!("items={n}, changed={m}, indexed_lookup_ms={before:.3}, key_scan_ms={after:.3}, scanned_keys={}",n*m*3);
    }
}
''']
probe=OUT/'lookup_probe.rs'
probe.write_text('\n'.join(parts),encoding='utf-8')
binary=OUT/'lookup_probe.exe'
subprocess.run(['rustc','--edition=2024','-C','opt-level=3',str(probe),'-o',str(binary)],check=True)
subprocess.run([str(binary)],check=True)
