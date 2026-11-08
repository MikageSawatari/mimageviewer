//! One viewer owns one fullscreen projection, including inactive panorama state.
#[derive(Default)]
pub(crate) enum FsExclusiveView {
    #[default]
    None,
    Panorama(crate::panorama::PanoramaState),
    Depth(crate::depth_parallax::DepthViewState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExclusiveViewKind {
    Panorama,
    Depth,
}

impl FsExclusiveView {
    pub(crate) fn panorama_state(&self) -> Option<&crate::panorama::PanoramaState> {
        match self {
            Self::Panorama(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn panorama_state_mut(&mut self) -> Option<&mut crate::panorama::PanoramaState> {
        match self {
            Self::Panorama(state) => Some(state),
            _ => None,
        }
    }

    pub(crate) fn is_depth(&self) -> bool {
        matches!(self, Self::Depth(_))
    }
}
