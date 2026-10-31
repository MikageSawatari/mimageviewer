use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use musicvideo_core::{ImageId, LayerId, LayerKind, Project};

static NEXT_TARGET_LIFETIME: AtomicU64 = AtomicU64::new(1);
static NEXT_RESOURCE_REVISION: AtomicU64 = AtomicU64::new(1);

/// Identifies one logical lifetime of a layer or image slot.
///
/// IDs in the serialized project may be reused after deletion. Async hosts use
/// this process-local token to reject a completion that belongs to the deleted
/// object rather than attaching it to a newly-created object with the same ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EditTargetLifetime(u64);

impl EditTargetLifetime {
    fn allocate() -> Self {
        Self(NEXT_TARGET_LIFETIME.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct EditTargetLifetimes {
    layers: HashMap<LayerId, EditTargetLifetime>,
    images: HashMap<ImageId, EditTargetLifetime>,
}

impl EditTargetLifetimes {
    pub(crate) fn reset(&mut self, project: &Project) {
        self.layers.clear();
        self.images.clear();
        self.reconcile(project);
    }

    pub(crate) fn reconcile(&mut self, project: &Project) {
        // An unresolved layer-only slot is still a replacement target. Keep
        // its lifetime until both the ImageRef and every layer reference end.
        let image_ids = project
            .images
            .iter()
            .map(|image| &image.id)
            .chain(project.layers.iter().filter_map(|layer| match &layer.kind {
                LayerKind::Image { image, .. } => Some(image),
                _ => None,
            }))
            .cloned()
            .collect::<HashSet<_>>();
        self.layers
            .retain(|id, _| project.layers.iter().any(|layer| layer.id == *id));
        self.images.retain(|id, _| image_ids.contains(id));
        for layer in &project.layers {
            self.layers
                .entry(layer.id)
                .or_insert_with(EditTargetLifetime::allocate);
        }
        for id in image_ids {
            self.images
                .entry(id)
                .or_insert_with(EditTargetLifetime::allocate);
        }
    }

    pub(crate) fn layer(&self, id: LayerId) -> Option<EditTargetLifetime> {
        self.layers.get(&id).copied()
    }

    pub(crate) fn image(&self, id: &ImageId) -> Option<EditTargetLifetime> {
        self.images.get(id).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceRevision(u64);

impl ResourceRevision {
    fn allocate() -> Self {
        Self(NEXT_RESOURCE_REVISION.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone)]
pub struct ResourceLease {
    revision: ResourceRevision,
    keep_alive: Arc<dyn Any + Send + Sync>,
}

impl ResourceLease {
    pub fn new<T>(keep_alive: Arc<T>) -> Self
    where
        T: Any + Send + Sync,
    {
        Self {
            revision: ResourceRevision::allocate(),
            keep_alive,
        }
    }

    pub fn revision(&self) -> ResourceRevision {
        self.revision
    }

    pub fn payload<T>(&self) -> Option<Arc<T>>
    where
        T: Any + Send + Sync,
    {
        Arc::clone(&self.keep_alive).downcast().ok()
    }
}

impl fmt::Debug for ResourceLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResourceLease")
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

impl PartialEq for ResourceLease {
    fn eq(&self, other: &Self) -> bool {
        self.revision == other.revision
    }
}

impl Eq for ResourceLease {}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResourceManifest {
    audio: Option<ResourceLease>,
    images: HashMap<ImageId, ResourceLease>,
}

impl ResourceManifest {
    pub fn audio(&self) -> Option<&ResourceLease> {
        self.audio.as_ref()
    }

    pub fn image(&self, id: &ImageId) -> Option<&ResourceLease> {
        self.images.get(id)
    }

    pub fn set_audio(&mut self, audio: Option<ResourceLease>) {
        self.audio = audio;
    }

    pub fn set_image(&mut self, id: ImageId, image: ResourceLease) {
        self.images.insert(id, image);
    }

    pub fn remove_image(&mut self, id: &ImageId) {
        self.images.remove(id);
    }

    pub fn image_leases(&self) -> impl Iterator<Item = (&ImageId, &ResourceLease)> {
        self.images.iter()
    }

    pub(crate) fn retain_project_resources(&mut self, project: &Project) {
        if project.audio.is_none() {
            self.audio = None;
        }
        self.images
            .retain(|id, _| project.images.iter().any(|image| image.id == *id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicvideo_core::{ImageRef, default_project};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct DropNotice(Arc<AtomicUsize>);

    impl Drop for DropNotice {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn image_lifetime_tracks_the_union_of_references_and_all_layer_slots() {
        let mut project = default_project(1.0);
        let id = ImageId::new(musicvideo_core::DEFAULT_BACKGROUND_IMAGE_ID);
        let mut lifetimes = EditTargetLifetimes::default();
        lifetimes.reconcile(&project);
        let lifetime = lifetimes
            .image(&id)
            .expect("layer-only slot is replaceable");

        project.images.push(ImageRef {
            id: id.clone(),
            path: "background.png".into(),
        });
        lifetimes.reconcile(&project);
        assert_eq!(lifetimes.image(&id), Some(lifetime));
        let background = project.layers.remove(0);
        lifetimes.reconcile(&project);
        assert_eq!(
            lifetimes.image(&id),
            Some(lifetime),
            "ImageRef alone keeps the slot alive"
        );
        let mut sibling = background.clone();
        sibling.id = LayerId(100);
        sibling.visible = musicvideo_core::Animatable::Const(false);
        project.layers.extend([background.clone(), sibling]);
        project.images.clear();
        lifetimes.reconcile(&project);
        assert_eq!(
            lifetimes.image(&id),
            Some(lifetime),
            "removing ImageRef preserves a referenced slot"
        );
        project.layers.remove(project.layers.len() - 2);
        lifetimes.reconcile(&project);
        assert_eq!(
            lifetimes.image(&id),
            Some(lifetime),
            "hidden sibling still owns the slot"
        );
        project.layers.pop();
        lifetimes.reconcile(&project);
        assert_eq!(lifetimes.image(&id), None);
        project.layers.push(background);
        lifetimes.reconcile(&project);
        assert_ne!(
            lifetimes.image(&id).unwrap(),
            lifetime,
            "recreated slot rejects old completions"
        );
    }

    #[test]
    fn image_lifetimes_are_local_to_each_editor_and_reset_on_project_switch() {
        let project = default_project(1.0);
        let id = ImageId::new(musicvideo_core::DEFAULT_BACKGROUND_IMAGE_ID);
        let mut first = EditTargetLifetimes::default();
        let mut sibling = EditTargetLifetimes::default();
        first.reconcile(&project);
        sibling.reconcile(&project);
        let first_lifetime = first.image(&id).unwrap();
        let sibling_lifetime = sibling.image(&id).unwrap();
        assert_ne!(first_lifetime, sibling_lifetime);
        first.reset(&project);
        assert_ne!(first.image(&id).unwrap(), first_lifetime);
        assert_eq!(sibling.image(&id), Some(sibling_lifetime));
    }

    #[test]
    fn manifest_clones_keep_payload_alive_until_the_last_history_lease_drops() {
        let drops = Arc::new(AtomicUsize::new(0));
        let payload = Arc::new(DropNotice(Arc::clone(&drops)));
        let lease = ResourceLease::new(payload);
        let mut manifest = ResourceManifest::default();
        manifest.set_audio(Some(lease));
        let history_copy = manifest.clone();

        drop(manifest);
        assert_eq!(drops.load(Ordering::Relaxed), 0);
        drop(history_copy);
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn manifest_releases_images_no_longer_referenced_by_the_project() {
        let keep = ImageId::new("keep");
        let remove = ImageId::new("remove");
        let mut project = default_project(1.0);
        project.images.push(ImageRef {
            id: keep.clone(),
            path: PathBuf::from("keep.png"),
        });
        let mut manifest = ResourceManifest::default();
        manifest.set_image(keep.clone(), ResourceLease::new(Arc::new(())));
        manifest.set_image(remove.clone(), ResourceLease::new(Arc::new(())));

        manifest.retain_project_resources(&project);

        assert!(manifest.image(&keep).is_some());
        assert!(manifest.image(&remove).is_none());
    }
}
