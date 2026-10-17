//! Заполняет базу демонстрационными задачами для проверки интерфейса.
//! `cargo run -p amin_core --example seed_demo -- /path/to/tasks.db`

use amin_core::{Kind, Priority, Store};
use chrono::{Duration, Local};

fn main() -> anyhow::Result<()> {
    let path = std::env::args().nth(1).expect("укажи путь к базе");
    let store = Store::open(&path)?;
    let today = Local::now().date_naive();
    let tags = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();

    let notes = store.add_task("Идеи для постов")?;
    store.set_kind(notes.id, Kind::Note)?;
    store.set_notes(notes.id, "1. Сравнить два редактора\n2. Что важнее в телефоне")?;

    let teeth = store.add_task("Вылечить зубы")?;
    store.set_tags(teeth.id, &tags(&["здоровье"]))?;

    let rent = store.add_task("Оплатить аренду")?;
    store.set_tags(rent.id, &tags(&["Дом"]))?;
    store.set_due(rent.id, Some(today + Duration::days(26)), None)?;

    let sell = store.add_task("Распродать ненужную электронику")?;
    store.set_tags(sell.id, &tags(&["проект"]))?;
    for title in ["Продать планшет", "Продать VR-шлем"] {
        let child = store.add_task(title)?;
        store.set_parent(child.id, Some(sell.id))?;
    }

    let refactor = store.add_task("Рефакторинг виджетов на сайте")?;
    store.set_tags(refactor.id, &tags(&["проект", "работа"]))?;
    store.set_priority(refactor.id, Priority::Medium)?;

    let meter = store.add_task("Сфотографировать счётчик и отправить хозяину квартиры")?;
    store.set_due(meter.id, Some(today + Duration::days(6)), None)?;

    let lamps = store.add_task("Поменять лампочки в комнате")?;
    store.set_tags(lamps.id, &tags(&["Дом"]))?;
    let order = store.add_task("Заказать подходящие лампочки")?;
    store.set_parent(order.id, Some(lamps.id))?;

    let chargers = store.add_task("Купить нормальные зарядные блоки")?;
    store.set_due(chargers.id, Some(today), None)?;
    store.set_priority(chargers.id, Priority::High)?;

    let table = store.add_task("Сложить стол")?;
    store.set_due(table.id, Some(today + Duration::days(1)), None)?;

    store.add_task("Написать пост в блог")?;
    let trash = store.add_task("Вынести мусор")?;
    store.set_due(trash.id, Some(today - Duration::days(1)), None)?;

    let done = store.add_task("Записаться к врачу")?;
    store.set_completed(done.id, true)?;

    store.set_tag_color("здоровье", Some("#E6EA49"))?;
    store.set_tag_color("Дом", Some("#4AA6EF"))?;
    store.set_tag_color("проект", Some("#838BB0"))?;
    store.set_tag_color("работа", Some("#C2DED0"))?;
    Ok(())
}
