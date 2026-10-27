use super::*;

/// Validated changes to the only persisted order and page-specific directives.
/// Destinations address the resulting order after removal; no derived unit is editable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositionEdit {
    MovePage {
        page: CompositionPageRef,
        to: usize,
    },
    SetOrder(Vec<CompositionPageRef>),
    SetHidden {
        page: CompositionPageRef,
        hidden: bool,
    },
    SetDirection(ReadingDirection),
    SetCover(bool),
    SetOverride {
        page: CompositionPageRef,
        directive: Option<PageOverride>,
    },
}

impl BookComposition {
    pub fn edited(&self, domain: &PageDomain, edit: CompositionEdit) -> CompositionResult<Self> {
        self.validate(domain)?;
        let mut draft = self.clone();
        match edit {
            CompositionEdit::MovePage { page, to } => {
                let from = draft
                    .page_order
                    .iter()
                    .position(|stored| stored.matching_key() == page.matching_key())
                    .ok_or(CompositionError::Invalid("page is not registered"))?;
                if to >= draft.page_order.len() {
                    return Err(CompositionError::Invalid(
                        "page destination is out of range",
                    ));
                }
                let page = draft.page_order.remove(from);
                draft.page_order.insert(to, page);
            }
            CompositionEdit::SetOrder(order) => {
                let before: std::collections::HashSet<_> = draft
                    .pages()
                    .map(CompositionPageRef::matching_key)
                    .collect();
                let after: std::collections::HashSet<_> =
                    order.iter().map(CompositionPageRef::matching_key).collect();
                if before != after || order.len() != draft.page_order.len() {
                    return Err(CompositionError::Invalid(
                        "order must preserve every registered page",
                    ));
                }
                draft.page_order = order;
            }
            CompositionEdit::SetHidden { page, hidden } => {
                let page = draft
                    .pages()
                    .find(|stored| stored.matching_key() == page.matching_key())
                    .cloned()
                    .ok_or(CompositionError::Invalid("page is not registered"))?;
                let was_hidden = draft
                    .hidden_pages
                    .iter()
                    .any(|stored| stored.matching_key() == page.matching_key());
                if hidden && !was_hidden {
                    draft.hidden_pages.push(page);
                } else if !hidden && was_hidden {
                    draft
                        .hidden_pages
                        .retain(|stored| stored.matching_key() != page.matching_key());
                }
            }
            CompositionEdit::SetDirection(direction) => draft.direction = direction,
            CompositionEdit::SetCover(enabled) => draft.cover_enabled = enabled,
            CompositionEdit::SetOverride { page, directive } => {
                let page = draft
                    .pages()
                    .find(|stored| stored.matching_key() == page.matching_key())
                    .cloned()
                    .ok_or(CompositionError::Invalid("page is not registered"))?;
                if draft.page_override(&page) != directive {
                    draft
                        .page_overrides
                        .retain(|entry| entry.page.matching_key() != page.matching_key());
                    if let Some(directive) = directive {
                        draft
                            .page_overrides
                            .push(PageOverrideEntry { page, directive });
                    }
                }
            }
        }
        draft.validate(domain)?;
        Ok(draft)
    }
}
