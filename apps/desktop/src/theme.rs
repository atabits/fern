use amin_core::Priority;
use gpui::{Rgba, rgb, rgba};

pub fn background() -> Rgba {
    rgb(0x1b1c20)
}

pub fn surface() -> Rgba {
    rgb(0x24262b)
}

pub fn surface_hover() -> Rgba {
    rgb(0x2c2f35)
}

pub fn border() -> Rgba {
    rgb(0x34373e)
}

pub fn text() -> Rgba {
    rgb(0xe6e7ea)
}

pub fn text_muted() -> Rgba {
    rgb(0x80848e)
}

pub fn accent() -> Rgba {
    rgb(0x4772fa)
}

pub fn selection() -> Rgba {
    rgba(0x4772fa55)
}

pub fn danger() -> Rgba {
    rgb(0xe5484d)
}

pub fn sidebar() -> Rgba {
    rgb(0x16171a)
}

pub fn row_hover() -> Rgba {
    rgb(0x23252a)
}

pub fn row_selected() -> Rgba {
    rgb(0x2a2d33)
}

pub fn row_separator() -> Rgba {
    rgb(0x26282d)
}

pub fn checkbox() -> Rgba {
    rgb(0x9a9ca3)
}

pub fn lists_panel() -> Rgba {
    rgb(0x18191d)
}

pub fn priority_high() -> Rgba {
    rgb(0xe5484d)
}

pub fn priority_medium() -> Rgba {
    rgb(0xf5a524)
}

pub fn priority_low() -> Rgba {
    rgb(0x4772fa)
}

/// Цвет флажка и рамки чекбокса для приоритета.
pub fn priority(priority: Priority) -> Rgba {
    match priority {
        Priority::High => priority_high(),
        Priority::Medium => priority_medium(),
        Priority::Low => priority_low(),
        Priority::None => checkbox(),
    }
}
