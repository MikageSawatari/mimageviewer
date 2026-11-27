//! One viewer owns one fullscreen projection, including inactive panorama state.
#[derive(Default)]
pub(crate) enum FsExclusiveView {
    #[default]
    None,
    Panorama(crate::panorama::PanoramaState),
    Depth(Box<crate::depth_parallax::DepthViewState>),
    PlaneView(crate::plane_view::PlaneViewState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExclusiveViewKind {
    Panorama,
    Depth,
    PlaneView,
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

    pub(crate) fn is_plane_view(&self) -> bool {
        matches!(self, Self::PlaneView(_))
    }

    /// These projections own live GPU state; panorama retains its existing intent lifecycle.
    pub(crate) fn retires_for_owner_change(&self) -> bool {
        matches!(self, Self::Depth(_) | Self::PlaneView(_))
    }
}
