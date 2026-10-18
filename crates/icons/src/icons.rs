//! Names of the icons in `assets/icons`. Each variant maps to `icons/<snake_case_name>.svg`.

use std::sync::Arc;

use strum::{EnumIter, IntoStaticStr};

#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone, EnumIter, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum IconName {
    ArchiveRestore,
    ArrowDownUp,
    Baseline,
    Bell,
    BrushCleaning,
    CalendarCheck,
    CalendarDays,
    CalendarHeart,
    CalendarPlus,
    CalendarRange,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Circle,
    CircleDot,
    CircleQuestionMark,
    Clock,
    Ellipsis,
    FileText,
    Flag,
    Gift,
    Heart,
    Hourglass,
    Inbox,
    Languages,
    Layers,
    LayoutGrid,
    List,
    ListChecks,
    MessageSquareText,
    Moon,
    PanelLeft,
    Pencil,
    Plus,
    RefreshCw,
    Repeat,
    RotateCcw,
    Search,
    SquareCheck,
    SquareCheckBig,
    Sun,
    Sunrise,
    Tag,
    Trash,
    User,
    X,
}

impl IconName {
    /// The asset path of this icon.
    pub fn path(self) -> Arc<str> {
        let file_stem: &'static str = self.into();
        format!("icons/{file_stem}.svg").into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn every_icon_has_a_file() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        for icon in IconName::iter() {
            assert!(dir.join(&*icon.path()).exists(), "missing file for {icon:?}");
        }
    }
}
