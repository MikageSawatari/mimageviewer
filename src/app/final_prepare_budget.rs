//! D3 GPU admission foundation. No production caller starts speculative work
//! until D3-2/3/4 have installed their bounded CPU and resource owners.
use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RegistrationState {
    #[default]
    Available,
    OrdinaryUpload,
    SpeculativeSubmitted,
}

#[derive(Default)]
pub(super) struct SpeculativeGpuFrame {
    frame: Option<u64>,
    state: RegistrationState,
}

impl SpeculativeGpuFrame {
    pub(super) fn begin(&mut self, root_frame: u64) {
        if self.frame != Some(root_frame) {
            self.frame = Some(root_frame);
            self.state = RegistrationState::Available;
        }
    }

    pub(super) fn note_ordinary_upload(&mut self) {
        if self.state == RegistrationState::Available {
            self.state = RegistrationState::OrdinaryUpload;
        }
    }

    #[allow(dead_code)] // Activated only with the bounded D3 producers.
    fn claim(&mut self) -> bool {
        if self.frame.is_none() || self.state != RegistrationState::Available {
            return false;
        }
        self.state = RegistrationState::SpeculativeSubmitted;
        true
    }
}

/// Options are the allocation's real options, including Nearest's absent mip
/// chain. Cache hits and shared handles bypass this allocation API and consume
/// no registration slot.
pub(super) fn speculative_vram_admitted(
    resident_bytes: u64,
    size: [usize; 2],
    options: egui::TextureOptions,
    budget: Option<u64>,
) -> bool {
    let texels = crate::vram_budget::texture_size_texels(size, options.mipmap_mode.is_some());
    let increment = crate::vram_budget::rgba8_texels_to_bytes(texels);
    budget.is_none_or(|limit| {
        resident_bytes
            .checked_add(increment)
            .is_some_and(|total| total <= limit)
    })
}

impl App {
    #[allow(dead_code)] // D3-1 never invokes an allocation through this gate.
    pub(super) fn admit_speculative_gpu_allocation(
        &mut self,
        size: [usize; 2],
        options: egui::TextureOptions,
    ) -> bool {
        // The normal resident walker includes every mounted/at-rest/parked owner
        // and live presentation/batch leases. Never evict to make D3 fit.
        speculative_vram_admitted(
            self.vram_accounting_snapshot().total_bytes(),
            size,
            options,
            self.vram_pool_budget().pool_bytes,
        ) && self.speculative_gpu_frame.claim()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d3_registration_budget_survives_reentry_and_ordinary_upload_after_claim() {
        let mut frame = SpeculativeGpuFrame::default();
        assert!(!frame.claim());
        frame.begin(10);
        assert!(frame.claim());
        frame.begin(10); // viewport, context swap, discard retry
        frame.note_ordinary_upload(); // allowed, does not refund speculative work
        assert!(!frame.claim());
        frame.begin(11);
        frame.note_ordinary_upload();
        assert!(!frame.claim());
        frame.begin(12);
        assert!(frame.claim());
    }

    #[test]
    fn d3_vram_admission_uses_actual_options_and_exact_mip_sum() {
        // Resident includes thumbnails as well as fullscreen allocations. Its
        // matching budget is the whole pool, not the fullscreen suballocation.
        let budget = crate::vram_budget::VramPoolBudget::from_pool_bytes(
            Some(1_000),
            crate::vram_budget::VramBudgetMode::Fullscreen,
        );
        assert!(speculative_vram_admitted(
            750,
            [5, 5],
            egui::TextureOptions::NEAREST,
            budget.pool_bytes,
        ));
        let size = [8000, 8192];
        let base = 8000 * 8192 * 4;
        assert!(speculative_vram_admitted(
            0,
            size,
            egui::TextureOptions::NEAREST,
            Some(base)
        ));
        assert!(!speculative_vram_admitted(
            1,
            size,
            egui::TextureOptions::NEAREST,
            Some(base)
        ));
        assert!(!speculative_vram_admitted(
            0,
            size,
            DISPLAY_IMAGE_TEXTURE_OPTIONS,
            Some(base)
        ));
        assert!(speculative_vram_admitted(
            0,
            size,
            DISPLAY_IMAGE_TEXTURE_OPTIONS,
            Some(349_525_084)
        ));
        assert!(!speculative_vram_admitted(
            0,
            size,
            DISPLAY_IMAGE_TEXTURE_OPTIONS,
            Some(349_525_083)
        ));
        assert!(speculative_vram_admitted(
            u64::MAX,
            size,
            egui::TextureOptions::NEAREST,
            None
        ));
        assert!(!speculative_vram_admitted(
            u64::MAX,
            size,
            egui::TextureOptions::NEAREST,
            Some(u64::MAX)
        ));
    }
}
