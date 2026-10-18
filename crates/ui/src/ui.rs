//! Reusable UI components, in the spirit of Zed's `ui` crate: small `RenderOnce` elements with
//! builder methods and semantic colors from the theme. Import [`prelude`] when building views.

mod color;
mod components;
pub mod prelude;
mod traits;

pub use color::Color;
pub use components::*;
pub use icons::IconName;
pub use traits::{Clickable, Toggleable};
