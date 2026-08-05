use super::*;

const FACET_NAME_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

pub(super) struct FacetNameCachePending {
    pub(super) generation: u64,
    pub(super) started_at: std::time::Instant,
    pub(super) rx: mpsc::Receiver<FacetNameCacheResult>,
}

pub(super) struct FacetNameCacheResult {
    generation: u64,
    names: Vec<Box<str>>,
    elapsed_ms: f64,
}

impl App {
    pub(crate) fn schedule_facet_name_filter_update(&mut self) {
        if self.facet_name_input == self.settings.facet_filter.name_query {
            self.facet_name_debounce_deadline = None;
        } else {
            self.facet_name_debounce_deadline =
                Some(std::time::Instant::now() + FACET_NAME_DEBOUNCE);
        }
    }

    pub(crate) fn clear_facet_name_filter_state(&mut self) -> bool {
        let changed = !self.facet_name_input.is_empty()
            || !self.settings.facet_filter.name_query.is_empty()
            || !self.facet_name_tokens.is_empty()
            || self.facet_name_debounce_deadline.is_some();
        self.facet_name_input.clear();
        self.settings.facet_filter.name_query.clear();
        self.facet_name_tokens.clear();
        self.facet_name_debounce_deadline = None;
        self.facet_name_cache_failed_generation = None;
        changed
    }

    pub(super) fn sync_facet_name_runtime_from_filter(&mut self) {
        self.facet_name_input = self.settings.facet_filter.name_query.clone();
        self.facet_name_tokens = crate::search_query::parse(&self.facet_name_input);
        self.facet_name_debounce_deadline = None;
        self.facet_name_cache_failed_generation = None;
        if !self.facet_name_tokens.is_empty() {
            self.ensure_facet_name_cache();
        }
    }

    pub(super) fn invalidate_facet_name_cache(&mut self) {
        self.facet_name_cache.clear();
        self.facet_name_cache_generation = None;
        self.facet_name_cache_pending = None;
        self.facet_name_cache_failed_generation = None;
    }

    fn ensure_facet_name_cache(&mut self) {
        if self.facet_name_tokens.is_empty()
            || (self.facet_name_cache_generation == Some(self.items_generation)
                && self.facet_name_cache.len() == self.items.len())
            || self
                .facet_name_cache_pending
                .as_ref()
                .is_some_and(|pending| pending.generation == self.items_generation)
            || self.facet_name_cache_failed_generation == Some(self.items_generation)
        {
            return;
        }

        self.facet_name_cache.clear();
        self.facet_name_cache_generation = None;
        self.facet_name_cache_pending = None;

        // GridItem は App が表示中ずっと所有するため、worker へは basename の生文字列だけを
        // snapshot する。小文字化と正規化済み Box<str> の確保はすべて worker 側で行う。
        let raw_names = self
            .items
            .iter()
            .map(|item| item.name().into_owned())
            .collect::<Vec<_>>();
        let generation = self.items_generation;
        let started_at = std::time::Instant::now();
        let (tx, rx) = mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("facet-name-cache".to_owned())
            .spawn(move || {
                let t0 = std::time::Instant::now();
                let names = raw_names
                    .into_iter()
                    .map(|name| name.to_lowercase().into_boxed_str())
                    .collect();
                let _ = tx.send(FacetNameCacheResult {
                    generation,
                    names,
                    elapsed_ms: t0.elapsed().as_secs_f64() * 1000.0,
                });
            });
        match spawn {
            Ok(_) => {
                self.facet_name_cache_pending = Some(FacetNameCachePending {
                    generation,
                    started_at,
                    rx,
                });
            }
            Err(err) => {
                self.facet_name_cache_failed_generation = Some(generation);
                crate::logger::log(format!("facet_name_cache: failed to spawn worker: {err}"));
            }
        }
    }

    fn apply_facet_name_input(&mut self, ctx: &egui::Context) {
        let query = self.facet_name_input.clone();
        if query == self.settings.facet_filter.name_query {
            return;
        }
        self.settings.facet_filter.name_query = query;
        self.facet_name_tokens = crate::search_query::parse(&self.settings.facet_filter.name_query);
        self.facet_name_cache_failed_generation = None;
        self.ensure_facet_name_cache();

        let color_scope_changed = self.color_filter.enabled;
        if color_scope_changed {
            self.color_filter.applied_scope_signature = None;
        }
        self.rebuild_visible_indices();
        if color_scope_changed {
            self.ensure_color_scan_for_current_scope(ctx);
        }
    }

    pub(super) fn poll_facet_name_filter(&mut self, ctx: &egui::Context) {
        let now = std::time::Instant::now();
        if let Some(deadline) = self.facet_name_debounce_deadline {
            if self.ime_input_active(ctx) {
                // 未確定文字で候補が消えないよう、composition 中は 150ms の残り時間を
                // 消費しない。確定後に改めて静止時間を待つ。
                self.facet_name_debounce_deadline = Some(now + FACET_NAME_DEBOUNCE);
                ctx.request_repaint_after(FACET_NAME_DEBOUNCE);
            } else if now >= deadline {
                self.facet_name_debounce_deadline = None;
                self.apply_facet_name_input(ctx);
            } else {
                ctx.request_repaint_after(deadline.saturating_duration_since(now));
            }
        }

        if self.facet_name_cache_generation.is_some_and(|generation| {
            generation != self.items_generation || self.facet_name_cache.len() != self.items.len()
        }) {
            self.facet_name_cache.clear();
            self.facet_name_cache_generation = None;
        }
        if self
            .facet_name_cache_pending
            .as_ref()
            .is_some_and(|pending| pending.generation != self.items_generation)
        {
            self.facet_name_cache_pending = None;
        }
        if !self.facet_name_tokens.is_empty()
            && self.facet_name_cache_generation != Some(self.items_generation)
            && self.facet_name_cache_pending.is_none()
        {
            self.ensure_facet_name_cache();
        }

        let Some(pending) = self.facet_name_cache_pending.take() else {
            return;
        };
        match pending.rx.try_recv() {
            Ok(result)
                if result.generation == self.items_generation
                    && result.names.len() == self.items.len() =>
            {
                self.facet_name_cache = result.names;
                self.facet_name_cache_generation = Some(result.generation);
                self.facet_name_cache_failed_generation = None;
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "facet_name",
                        "cache_build",
                        None,
                        0,
                        &[
                            ("items", serde_json::Value::from(self.items.len())),
                            ("ms", serde_json::Value::from(result.elapsed_ms)),
                        ],
                    );
                }
                let color_scope_changed = self.color_filter.enabled;
                if color_scope_changed {
                    self.color_filter.applied_scope_signature = None;
                }
                self.rebuild_visible_indices();
                if color_scope_changed {
                    self.ensure_color_scan_for_current_scope(ctx);
                }
                ctx.request_repaint();
            }
            Ok(_) => {
                // generation / len が一致しない結果は、別一覧の basename なので破棄する。
            }
            Err(mpsc::TryRecvError::Empty) => {
                self.facet_name_cache_pending = Some(pending);
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.facet_name_cache_failed_generation = Some(pending.generation);
                crate::logger::log(format!(
                    "facet_name_cache: worker disconnected after {:.1}ms",
                    pending.started_at.elapsed().as_secs_f64() * 1000.0
                ));
            }
        }
    }

    pub(super) fn passes_facet_name_filter(&self, idx: usize) -> bool {
        if self.settings.facet_filter.name_query.is_empty() || self.facet_name_tokens.is_empty() {
            return true;
        }
        if self.facet_name_cache_generation != Some(self.items_generation)
            || self.facet_name_cache.len() != self.items.len()
        {
            // 正規化準備中は一覧を空にせず、完成まで全件を通す。
            return true;
        }
        self.facet_name_cache.get(idx).is_some_and(|name| {
            crate::search_query::matches_lowercased_with_mode(
                &self.facet_name_tokens,
                name,
                crate::search_query::MatchMode::And,
            )
        })
    }
}
