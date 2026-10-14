use super::*;

/// Pure editor unit operations, never viewer toggles. Positions name the current
/// draft; a rejected operation leaves the original draft and revision untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnitEdit {
    MoveUnit {
        from: usize,
        to: usize,
    },
    SplitPair {
        unit: usize,
    },
    /// Extract a real page; a Blank moves with its Single. A Pair's remaining
    /// member becomes a Single in its old slot. Destination is an insertion
    /// boundary in the resulting unit list (after extraction).
    MovePage {
        page: CompositionPageRef,
        to: usize,
    },
    /// Pair two consecutive real Singles; explicitly removes their companions.
    PairSingles {
        first_unit: usize,
        /// Explicit previewable consent to clear a cover belonging to this Pair
        /// and its helpers in the same draft operation.
        clear_cover: bool,
    },
    InsertBlank {
        page: CompositionPageRef,
        placement: SinglePlacement,
    },
    SetSingle {
        page: CompositionPageRef,
        placement: SinglePlacement,
        companion: SingleCompanion,
    },
    SetHidden {
        page: CompositionPageRef,
        hidden: bool,
    },
    SetDirection(ReadingDirection),
    /// Setting a cover splits its Pair; changing/clearing cover removes existing
    /// helpers explicitly as part of this one draft operation.
    SetCover(Option<CompositionPageRef>),
}

impl BookComposition {
    pub fn edited(&self, domain: &PageDomain, edit: UnitEdit) -> CompositionResult<Self> {
        self.validate(domain)?;
        let mut draft = self.clone();
        match edit {
            UnitEdit::MoveUnit { from, to } => {
                if from >= draft.units.len() || to >= draft.units.len() {
                    return Err(CompositionError::Invalid("unit position out of range"));
                }
                let unit = draft.units.remove(from);
                draft.units.insert(to, unit);
            }
            UnitEdit::SplitPair { unit } => draft.split_pair(unit)?,
            UnitEdit::MovePage { page, to } => {
                let unit = draft.extract_page(&page)?;
                if to > draft.units.len() {
                    return Err(CompositionError::Invalid("unit position out of range"));
                }
                draft.units.insert(to, unit);
            }
            UnitEdit::PairSingles {
                first_unit,
                clear_cover,
            } => {
                let next = first_unit
                    .checked_add(1)
                    .ok_or(CompositionError::Invalid("unit position out of range"))?;
                let Some(CompositionUnit::Single { page: first, .. }) = draft.units.get(first_unit)
                else {
                    return Err(CompositionError::Invalid(
                        "pair requires two consecutive Singles",
                    ));
                };
                let Some(CompositionUnit::Single { page: second, .. }) = draft.units.get(next)
                else {
                    return Err(CompositionError::Invalid(
                        "pair requires two consecutive Singles",
                    ));
                };
                let first = first.clone();
                let second = second.clone();
                if clear_cover
                    && draft.cover.as_ref().is_some_and(|cover| {
                        cover.matching_key() == first.matching_key()
                            || cover.matching_key() == second.matching_key()
                    })
                {
                    draft.cover = None;
                    draft.remove_cover_helpers();
                }
                draft.units[first_unit] = CompositionUnit::Pair { first, second };
                draft.units.remove(next);
            }
            UnitEdit::InsertBlank { page, placement } => {
                let unit = draft.find_page(&page)?;
                if matches!(
                    &draft.units[unit],
                    CompositionUnit::Single {
                        companion: SingleCompanion::Blank,
                        ..
                    }
                ) {
                    return Err(CompositionError::Invalid(
                        "page already has a blank companion",
                    ));
                }
                draft.set_single(&page, placement, SingleCompanion::Blank)?;
            }
            UnitEdit::SetSingle {
                page,
                placement,
                companion,
            } => draft.set_single(&page, placement, companion)?,
            UnitEdit::SetHidden { page, hidden } => {
                draft.find_page(&page)?;
                let exists = draft
                    .hidden_pages
                    .iter()
                    .any(|value| value.matching_key() == page.matching_key());
                if hidden && !exists {
                    draft.hidden_pages.push(page);
                } else if !hidden {
                    draft
                        .hidden_pages
                        .retain(|value| value.matching_key() != page.matching_key());
                }
            }
            UnitEdit::SetDirection(direction) => draft.direction = direction,
            UnitEdit::SetCover(cover) => {
                if let Some(page) = &cover {
                    let unit = draft.find_page(page)?;
                    if matches!(draft.units[unit], CompositionUnit::Pair { .. }) {
                        draft.split_pair(unit)?;
                    }
                }
                if draft.cover.as_ref().map(CompositionPageRef::matching_key)
                    != cover.as_ref().map(CompositionPageRef::matching_key)
                {
                    draft.remove_cover_helpers();
                    draft.cover = cover;
                }
            }
        }
        draft.validate(domain)?;
        // Revision is the later store's responsibility, not a draft command counter.
        Ok(draft)
    }

    fn find_page(&self, page: &CompositionPageRef) -> CompositionResult<usize> {
        self.units
            .iter()
            .position(|unit| {
                unit.pages()
                    .any(|value| value.matching_key() == page.matching_key())
            })
            .ok_or(CompositionError::Invalid("page is not registered"))
    }

    fn remove_cover_helpers(&mut self) {
        for unit in &mut self.units {
            if let CompositionUnit::Single { companion, .. } = unit
                && matches!(companion, SingleCompanion::FrontCoverSupplement { .. })
            {
                *companion = SingleCompanion::Vacant;
            }
        }
    }

    fn split_pair(&mut self, unit: usize) -> CompositionResult<()> {
        let Some(CompositionUnit::Pair { first, second }) = self.units.get(unit) else {
            return Err(CompositionError::Invalid("unit is not a Pair"));
        };
        let first = first.clone();
        let second = second.clone();
        self.units[unit] = CompositionUnit::Single {
            page: first,
            placement: SinglePlacement::Leading,
            companion: SingleCompanion::Vacant,
        };
        self.units.insert(
            unit + 1,
            CompositionUnit::Single {
                page: second,
                placement: SinglePlacement::Trailing,
                companion: SingleCompanion::Vacant,
            },
        );
        Ok(())
    }

    fn set_single(
        &mut self,
        page: &CompositionPageRef,
        placement: SinglePlacement,
        companion: SingleCompanion,
    ) -> CompositionResult<()> {
        let unit = self.find_page(page)?;
        if matches!(self.units[unit], CompositionUnit::Pair { .. }) {
            self.split_pair(unit)?;
        }
        let unit = self.find_page(page)?;
        // Keep stored identity spelling, even when selection used a case/slash alias.
        let CompositionUnit::Single {
            placement: target,
            companion: other,
            ..
        } = &mut self.units[unit]
        else {
            unreachable!()
        };
        *target = placement;
        *other = companion;
        Ok(())
    }

    fn extract_page(&mut self, page: &CompositionPageRef) -> CompositionResult<CompositionUnit> {
        let unit = self.find_page(page)?;
        if matches!(self.units[unit], CompositionUnit::Pair { .. }) {
            self.split_pair(unit)?;
        }
        let unit = self.find_page(page)?;
        Ok(self.units.remove(unit))
    }
}
