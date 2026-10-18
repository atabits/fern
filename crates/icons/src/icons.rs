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
    Bold,
    Bookmark,
    BrushCleaning,
    Cake,
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
    Code,
    Copy,
    CornerDownRight,
    Ellipsis,
    Eye,
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
    Heading,
    Heart,
    Highlighter,
    History,
    Hourglass,
    Image,
    Inbox,
    Info,
    Italic,
    Kanban,
    Keyboard,
    Languages,
    Layers,
    LayoutGrid,
    LayoutTemplate,
    Link,
    List,
    ListChecks,
    ListOrdered,
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
    Quote,
    RefreshCw,
    Repeat,
    RotateCcw,
    Rows3,
    Search,
    SeparatorHorizontal,
    Settings,
    Share,
    Square,
    SquareArrowOutUpRight,
    SquareCheck,
    SquareCheckBig,
    SquareX,
    Strikethrough,
    Sun,
    Sunrise,
    Tag,
    Timer,
    ToggleRight,
    Trash,
    Underline,
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
