//! Names of the icons in `assets/icons`. Each variant maps to `icons/<snake_case_name>.svg`.

use std::sync::Arc;

use strum::{EnumIter, IntoStaticStr};

#[derive(Debug, PartialEq, Eq, Hash, Copy, Clone, EnumIter, IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum IconName {
    AlarmClock,
    Archive,
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
    ChartGantt,
    ChartNoAxesColumn,
    Check,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronsUpDown,
    Circle,
    CircleCheck,
    CircleDot,
    CircleQuestionMark,
    Clock,
    Copy,
    CornerDownRight,
    Ellipsis,
    FernCalendar,
    FernCountdown,
    FernFocus,
    FernHabit,
    FernMatrix,
    FernTasks,
    FileText,
    Flag,
    Flame,
    Funnel,
    Gift,
    Heart,
    Hourglass,
    Image,
    Inbox,
    Info,
    Kanban,
    Keyboard,
    Languages,
    Layers,
    LayoutGrid,
    LayoutTemplate,
    Link,
    List,
    ListChecks,
    ListTree,
    Mail,
    MapPin,
    MessageSquareText,
    Minus,
    Moon,
    PaintRoller,
    PanelLeft,
    Paperclip,
    Pause,
    Pencil,
    Percent,
    Pin,
    PinOff,
    Play,
    Plus,
    Printer,
    RefreshCw,
    Repeat,
    RotateCcw,
    Rows3,
    Search,
    Settings,
    Share,
    Square,
    SquareCheck,
    SquareCheckBig,
    SquareX,
    Sun,
    Sunrise,
    Tag,
    Timer,
    ToggleRight,
    Trash,
    User,
    X,
    Zap,
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
