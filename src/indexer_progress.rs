//! インデクサ進捗レポーター。
//!
//! `search_walker` / `ingest_worker` から "今何してる" を短いメッセージで通知し、
//! `indexer_supervisor` → UI に伝える軽量チャネル。
//!
//! ## なぜ別モジュールか
//!
//! `SupervisorStats` に `current_activity: Option<String>` を載せるだけだと、
//! 下位モジュール (walker / ingest_worker) が `indexer_supervisor::SupervisorStats`
//! を import する必要があり、層構造が逆転してしまう。代わりに共通の軽量型を
//! ここに置き、supervisor が Arc で保持して下位に渡す。
//!
//! ## 使い方
//!
//! ```ignore
//! let reporter = ProgressReporter::new();
//! // walker / ingest 側
//! reporter.set("scanning: C:\\foo\\bar\\baz");
//! // UI 側
//! if let Some(msg) = reporter.snapshot() { println!("{msg}"); }
//! ```
//!
//! 書き込み/読み出しは `Mutex<Option<String>>` で直列化する。
//! Walker のホットループではスロットルして使う (毎エントリ lock は過剰)。

use std::sync::{Arc, Mutex};

/// 軽量な "今何してる" レポーター。Clone で同じ共有状態を参照する。
#[derive(Clone, Default)]
pub struct ProgressReporter {
    inner: Arc<Mutex<Option<String>>>,
}

impl ProgressReporter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 現在の作業メッセージを設定する (以前の値は上書き)。
    pub fn set<S: Into<String>>(&self, msg: S) {
        if let Ok(mut g) = self.inner.lock() {
            *g = Some(msg.into());
        }
    }

    /// 作業完了を示し、メッセージをクリアする。
    pub fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            *g = None;
        }
    }

    /// 現在の値のスナップショット。
    pub fn snapshot(&self) -> Option<String> {
        self.inner.lock().ok().and_then(|g| g.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_and_snapshot_roundtrip() {
        let r = ProgressReporter::new();
        assert_eq!(r.snapshot(), None);
        r.set("hello");
        assert_eq!(r.snapshot().as_deref(), Some("hello"));
        r.set("world");
        assert_eq!(r.snapshot().as_deref(), Some("world"));
        r.clear();
        assert_eq!(r.snapshot(), None);
    }

    #[test]
    fn clone_shares_state() {
        let a = ProgressReporter::new();
        let b = a.clone();
        a.set("x");
        assert_eq!(b.snapshot().as_deref(), Some("x"));
        b.set("y");
        assert_eq!(a.snapshot().as_deref(), Some("y"));
    }
}
