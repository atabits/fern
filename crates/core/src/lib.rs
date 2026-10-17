//! Общее ядро трекера: модель данных и локальное хранилище.

pub mod model;
pub mod quick_add;
pub mod store;

pub use model::{Id, Kind, Priority, Tag, Task, View, ViewCounts};
pub use quick_add::{ParsedTask, parse_quick_add};
pub use store::Store;
