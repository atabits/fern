//! Names of the icons in `assets/icons`. Each variant maps to `icons/<snake_case_name>.svg`.

use std::sync::Arc;

use strum::{EnumIter, IntoStaticStr};

#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone, EnumIter, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum IconName {
    ArrowDownUp,
    Baseline,
    Bell,
    CalendarCheck,
    CalendarDays,
    CalendarHeart,
    CalendarRange,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    CircleDot,
    CircleQuestionMark,
    Clock,
    Ellipsis,
    FileText,
    Flag,
    Inbox,
    Languages,
    Layers,
    LayoutGrid,
    List,
    ListChecks,
    MessageSquareText,
    PanelLeft,
    Pencil,
    Plus,
    RefreshCw,
    Search,
    SquareCheck,
    SquareCheckBig,
    Sunrise,
    Tag,
    Trash,
    User,
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
