//! The only crate that names GPUI (ARCHITECTURE.md 2, 2.1). Everything else reaches it as
//! `nd_gpui::gpui`, and lays out with the taffy GPUI converts its styles into as `nd_gpui::taffy`.
//! The `fast` feature swaps GPUI for gpui-fast, which pins the same taffy. The pinned snapshot,
//! the fork branch and the patch queue are described in `fork.toml`; `scripts/gpui-fork` applies,
//! vendors, bumps and gates them.

#[cfg(not(feature = "fast"))]
pub use gpui;
#[cfg(feature = "fast")]
pub use gpui_fast as gpui;

#[cfg(feature = "fast")]
pub use gpui_fast_platform as gpui_platform;
#[cfg(all(feature = "platform", not(feature = "fast")))]
pub use gpui_platform;

pub use taffy;

/// The taffy surface `nd-ui`'s layout of retained backends uses. gpui-pre and gpui-fast both pin
/// taffy 0.15; when their versions part again, this is where the difference goes.
pub mod layout {
    use crate::taffy::{self, AvailableSpace, NodeId, Size, TaffyResult, TaffyTree};

    /// Taffy's type for `min_size` and `max_size`.
    pub type MinMax = taffy::LengthPercentageAuto;

    /// `compute_layout_with_measure` with a plain measure signature: known dimensions and
    /// available space in, the leaf's content size out. Taffy's measure function lays the whole
    /// leaf out, so the content size goes through `compute_leaf_layout` for padding, border and
    /// min and max sizes, as GPUI's own layout does.
    pub fn compute_with_measure<C>(
        tree: &mut TaffyTree<C>,
        root: NodeId,
        available: Size<AvailableSpace>,
        mut measure: impl FnMut(Size<Option<f32>>, Size<AvailableSpace>, Option<&mut C>) -> Size<f32>,
    ) -> TaffyResult<()> {
        tree.compute_layout_with_measure(root, available, |input, _, mut context, style| {
            taffy::compute_leaf_layout(
                input,
                style,
                |_, _| 0.0,
                |known, space| measure(known, space, context.as_deref_mut()),
            )
        })
    }
}
