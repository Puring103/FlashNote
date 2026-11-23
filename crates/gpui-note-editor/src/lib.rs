//! Structured rich-text editing. Markdown exists at the import/export boundary.
pub mod document;
pub mod mapping;
#[cfg(feature = "gpui-view")]
mod view;
#[cfg(feature = "gpui-view")]
pub use view::{Changed, RichEditor, init};
