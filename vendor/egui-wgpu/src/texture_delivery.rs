//! Optional submission diagnostics at the shared native texture-delivery owner.

use std::sync::OnceLock;
#[cfg(feature = "winit")]
use std::sync::atomic::{AtomicU64, Ordering};

/// One texture update whose writes and necessary mip generation have been submitted.
/// This is queue submission evidence, not GPU completion or application readiness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureDeliverySubmitted {
    pub renderer: u64,
    pub texture_id: epaint::TextureId,
    pub batch: u64,
    /// Level-zero bytes written by this delta (RGBA8); excludes generated mip levels.
    pub bytes: u64,
    /// Whether this update actually generated and submitted lower mip levels.
    pub mipmap: bool,
    pub site: crate::atlas_diag::Site,
}

struct Observer {
    #[cfg_attr(not(feature = "winit"), expect(dead_code))]
    enabled: fn() -> bool,
    #[cfg_attr(not(feature = "winit"), expect(dead_code))]
    submitted: fn(TextureDeliverySubmitted),
}

static OBSERVER: OnceLock<Observer> = OnceLock::new();
#[cfg(feature = "winit")]
static NEXT_BATCH: AtomicU64 = AtomicU64::new(1);

/// Install process-wide lightweight diagnostics once; returns false if already installed.
/// The enabled predicate runs before metadata collection. Callbacks run after submission,
/// outside the renderer lock, and before frees. They must not reenter App/UI or submit work.
pub fn install_texture_delivery_observer(
    enabled: fn() -> bool,
    submitted: fn(TextureDeliverySubmitted),
) -> bool {
    OBSERVER.set(Observer { enabled, submitted }).is_ok()
}

/// Diagnostic metadata only: no texture handles or delivery authority.
#[cfg(feature = "winit")]
pub(crate) struct DeliveryObservation {
    observer: &'static Observer,
    batch: u64,
    updates: Vec<TextureDeliverySubmitted>,
}

#[cfg(feature = "winit")]
impl DeliveryObservation {
    pub(crate) fn begin() -> Option<Self> {
        let observer = OBSERVER.get().filter(|observer| (observer.enabled)())?;
        Some(Self {
            observer,
            batch: NEXT_BATCH.fetch_add(1, Ordering::Relaxed),
            updates: Vec::new(),
        })
    }

    pub(crate) fn record(
        &mut self,
        renderer: u64,
        texture_id: epaint::TextureId,
        delta: &epaint::ImageDelta,
        mipmap: bool,
        site: crate::atlas_diag::Site,
    ) {
        self.updates.push(TextureDeliverySubmitted {
            renderer,
            texture_id,
            batch: self.batch,
            bytes: delta.image.width() as u64 * delta.image.height() as u64 * 4,
            mipmap,
            site,
        });
    }

    /// Called only after the owner has submitted every recorded update.
    pub(crate) fn submitted(&mut self) {
        for update in self.updates.drain(..) {
            (self.observer.submitted)(update);
        }
    }
}
