use super::*;
use crate::vram_budget::{VramAccountant, VramBudgetMode, VramPoolBudget, VramSubsystem};

/// The real resident-readiness run retained two 8192px sources on the
/// outgoing spread and one on the incoming spread. Count their mip chains,
/// ordinary native outputs, and repeated references from sibling owners.
#[test]
fn measured_high_quality_spread_pin_fits_tenth_without_double_counting() {
    let budget = VramPoolBudget::from_pool_bytes(Some(12_655_263_744), VramBudgetMode::Fullscreen);
    assert_eq!(budget.fullscreen_bytes, Some(10_124_210_995));
    assert_eq!(page_turn_pin_limit(budget), 1_012_421_099);

    let sources = [
        (719, [5635, 8192]),
        (697, [5628, 8192]),
        (692, [1424, 2069]),
        (702, [5632, 8192]),
    ];
    // IDs here are fixtures. Sizes are the ordinary native outputs measured
    // for these pages; incoming outputs were not prepared on the skipped turn.
    let native = [
        (1001, [1293, 1881]),
        (1002, [1292, 1881]),
        (1003, [1294, 1880]),
        (1004, [1293, 1880]),
    ];
    let mut accountant = VramAccountant::default();
    for (id, size) in sources {
        accountant.add_texture_id(
            VramSubsystem::PageTurnPresentation,
            egui::TextureId::Managed(id),
            size,
            true,
        );
    }
    for (id, size) in &native[..2] {
        accountant.add_texture_id(
            VramSubsystem::PageTurnPresentation,
            egui::TextureId::User(*id),
            *size,
            false,
        );
    }
    assert_eq!(accountant.finish().total_bytes(), 773_292_152);

    let mut accountant = VramAccountant::default();
    // Cache residency, a mounted unit and another owner's submitted batch
    // must not charge the same GPU textures more than once.
    for subsystem in [
        VramSubsystem::FinalCompositeCache,
        VramSubsystem::PageTurnPresentation,
        VramSubsystem::PageTurnPresentation,
    ] {
        for (id, size) in sources {
            accountant.add_texture_id(subsystem, egui::TextureId::Managed(id), size, true);
        }
        for (id, size) in native {
            accountant.add_texture_id(subsystem, egui::TextureId::User(id), size, false);
        }
    }
    let prepared_bytes = accountant.finish().total_bytes();
    assert_eq!(prepared_bytes, 792_746_392);
    assert!(prepared_bytes > 128 * 1024 * 1024);
    assert!(prepared_bytes <= page_turn_pin_limit(budget));
}

#[test]
fn pin_limit_preserves_small_finite_pool_constraint_and_bounded_unlimited_fallback() {
    let budget =
        VramPoolBudget::from_pool_bytes(Some(100 * 1024 * 1024), VramBudgetMode::Fullscreen);
    assert_eq!(page_turn_pin_limit(budget), 8 * 1024 * 1024);
    assert!(792_746_392 > page_turn_pin_limit(budget));

    let unlimited = VramPoolBudget::from_pool_bytes(None, VramBudgetMode::Fullscreen);
    assert_eq!(page_turn_pin_limit(unlimited), 1024 * 1024 * 1024);
}
