//! Curated timeless quotes about time, localized for English, Russian, and Simplified Chinese.

use chrono::{Datelike, NaiveDate};
use fern_i18n::Locale;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quote {
    pub text: &'static str,
    pub author: &'static str,
}

struct LocalizedQuote {
    en: Quote,
    ru: Quote,
    zh_cn: Quote,
}

pub const QUOTE_COUNT: usize = 24;

const QUOTES: [LocalizedQuote; QUOTE_COUNT] = [
    LocalizedQuote {
        en: Quote {
            text: "It is not that we have a short time to live, but that we waste a lot of it.",
            author: "Seneca",
        },
        ru: Quote {
            text: "Не мало у нас времени, а много мы его теряем.",
            author: "Сенека",
        },
        zh_cn: Quote {
            text: "不是我们拥有的生命太短，而是我们浪费的时间太多。",
            author: "塞涅卡",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "You could leave life right now. Let that determine what you do and say and think.",
            author: "Marcus Aurelius",
        },
        ru: Quote {
            text: "Ты можешь покинуть жизнь в любой миг. Пусть это определяет всё, что ты делаешь, говоришь и мыслишь.",
            author: "Марк Аврелий",
        },
        zh_cn: Quote {
            text: "你随时可能离开人世。让这个念头决定你的一言一行与所思所想。",
            author: "马可·奥勒留",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "The distinction between the past, present and future is only a stubbornly persistent illusion.",
            author: "Albert Einstein",
        },
        ru: Quote {
            text: "Различие между прошлым, настоящим и будущим — всего лишь упорно сохраняющаяся иллюзия.",
            author: "Альберт Эйнштейн",
        },
        zh_cn: Quote {
            text: "过去、现在与未来之间的区别，不过是一种挥之不去的顽固幻觉。",
            author: "阿尔伯特·爱因斯坦",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Lost time is never found again.",
            author: "Benjamin Franklin",
        },
        ru: Quote {
            text: "Потерянного времени не воротишь.",
            author: "Бенджамин Франклин",
        },
        zh_cn: Quote {
            text: "逝去的光阴一去不复返。",
            author: "本杰明·富兰克林",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Your time is limited, so don't waste it living someone else's life.",
            author: "Steve Jobs",
        },
        ru: Quote {
            text: "Ваше время ограничено, не тратьте его, живя чужой жизнью.",
            author: "Стив Джобс",
        },
        zh_cn: Quote {
            text: "你的时间有限，不要浪费在重复别人的生活上。",
            author: "史蒂夫·乔布斯",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Time is not an object, but an idea.",
            author: "Fyodor Dostoevsky",
        },
        ru: Quote {
            text: "Время не предмет, а идея.",
            author: "Фёдор Достоевский",
        },
        zh_cn: Quote {
            text: "时间不是实体，而是一种观念。",
            author: "费奥多尔·陀思妥耶夫斯基",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "The two most powerful warriors are patience and time.",
            author: "Leo Tolstoy",
        },
        ru: Quote {
            text: "Нет на свете сильнее воинов, чем терпение и время.",
            author: "Лев Толстой",
        },
        zh_cn: Quote {
            text: "世上最强大的两位战士，是耐心与时间。",
            author: "列夫·托尔斯泰",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "If you love life, don't waste time, for time is what life is made up of.",
            author: "Bruce Lee",
        },
        ru: Quote {
            text: "Если ты любишь жизнь, не трать времени зря, ведь жизнь соткана из времени.",
            author: "Брюс Ли",
        },
        zh_cn: Quote {
            text: "如果你热爱生命，就不要浪费时间，因为生命正是由时间构成的。",
            author: "李小龙",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Time flows away like the water in the river, never ceasing day or night.",
            author: "Confucius",
        },
        ru: Quote {
            text: "Всё течёт, как эта река, не останавливаясь ни днём, ни ночью.",
            author: "Конфуций",
        },
        zh_cn: Quote {
            text: "逝者如斯夫，不舍昼夜。",
            author: "孔子",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Each new hour holds new chances for new beginnings.",
            author: "Maya Angelou",
        },
        ru: Quote {
            text: "Каждый новый час несёт новые возможности для новых начинаний.",
            author: "Майя Энджелоу",
        },
        zh_cn: Quote {
            text: "每一个崭新的小时，都蕴藏着重新开始的新契机。",
            author: "玛雅·安吉罗",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "A man who dares to waste one hour of time has not discovered the value of life.",
            author: "Charles Darwin",
        },
        ru: Quote {
            text: "Человек, который осмеливается потратить впустую хотя бы час времени, ещё не осознал ценности жизни.",
            author: "Чарльз Дарвин",
        },
        zh_cn: Quote {
            text: "敢于浪费哪怕一小时时间的人，还没有真正懂得生命的价值。",
            author: "查尔斯·达尔文",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Time stays long enough for anyone who will use it.",
            author: "Leonardo da Vinci",
        },
        ru: Quote {
            text: "Время остаётся достаточно долгим для тех, кто умеет им пользоваться.",
            author: "Леонардо да Винчи",
        },
        zh_cn: Quote {
            text: "对于善用时间的人而言，时间总是足够充裕。",
            author: "列奥纳多·达·芬奇",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Better three hours too soon than a minute too late.",
            author: "William Shakespeare",
        },
        ru: Quote {
            text: "Лучше на три часа раньше, чем на минуту позже.",
            author: "Уильям Шекспир",
        },
        zh_cn: Quote {
            text: "宁可早到三小时，不可迟到一分钟。",
            author: "威廉·莎士比亚",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "You must live in the present, launch yourself on every wave, find your eternity in each moment.",
            author: "Henry David Thoreau",
        },
        ru: Quote {
            text: "Живи в настоящем, бросайся на каждую волну, находи вечность в каждом мгновении.",
            author: "Генри Дэвид Торо",
        },
        zh_cn: Quote {
            text: "要活在当下，迎着每一朵浪花前行，在每一个瞬间寻得永恒。",
            author: "亨利·戴维·梭罗",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Just in terms of allocation of time resources, religion is not very efficient. There's a lot more I could be doing on a Sunday morning.",
            author: "Bill Gates",
        },
        ru: Quote {
            text: "С точки зрения распределения времени религия не слишком эффективна. Воскресным утром я могу заняться куда более полезными делами.",
            author: "Билл Гейтс",
        },
        zh_cn: Quote {
            text: "仅从时间资源的分配来看，宗教并不算高效。周日清晨我还有许多更值得做的事。",
            author: "比尔·盖茨",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Time is my estate; time is the field I till.",
            author: "Johann Wolfgang von Goethe",
        },
        ru: Quote {
            text: "Время — вот моё владение, время — нива моя.",
            author: "Иоганн Вольфганг фон Гёте",
        },
        zh_cn: Quote {
            text: "时间是我的领地，时间是我耕耘的田野。",
            author: "约翰·沃尔夫冈·冯·歌德",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Time is what we want most, but what we use worst.",
            author: "William Penn",
        },
        ru: Quote {
            text: "Время — это то, чего мы хотим больше всего и что используем хуже всего.",
            author: "Уильям Пенн",
        },
        zh_cn: Quote {
            text: "时间是我们最渴望拥有、却最不善珍惜的东西。",
            author: "威廉·佩恩",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "No one ever steps in the same river twice, for it is not the same river and they are not the same person.",
            author: "Heraclitus",
        },
        ru: Quote {
            text: "Нельзя войти в одну и ту же реку дважды: ни река, ни человек уже не те.",
            author: "Гераклит",
        },
        zh_cn: Quote {
            text: "人不能两次踏入同一条河流，因为河水已变，人亦非昨。",
            author: "赫拉克利特",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Seize the day, putting as little trust as possible in tomorrow.",
            author: "Horace",
        },
        ru: Quote {
            text: "Лови день, как можно меньше доверяя завтрашнему.",
            author: "Гораций",
        },
        zh_cn: Quote {
            text: "把握今日，尽量少寄望于明日。",
            author: "贺拉斯",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "An hour, once it lodges in the queer element of the human spirit, may be stretched to fifty or a hundred times its clock length.",
            author: "Virginia Woolf",
        },
        ru: Quote {
            text: "Час, попав в причудливую стихию человеческого духа, способен растянуться в пятьдесят или сто раз.",
            author: "Вирджиния Вулф",
        },
        zh_cn: Quote {
            text: "一个小时一旦落入人类心灵的奇异世界，便可能拉长至钟表刻度的五十倍甚至百倍。",
            author: "弗吉尼亚·伍尔夫",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "It is the time you have wasted for your rose that makes your rose so important.",
            author: "Antoine de Saint-Exupéry",
        },
        ru: Quote {
            text: "Твоя роза так дорога тебе потому, что ты отдавал ей всё своё время.",
            author: "Антуан де Сент-Экзюпери",
        },
        zh_cn: Quote {
            text: "正是你为玫瑰花费的时间，才使你的玫瑰变得如此重要。",
            author: "安托万·德·圣埃克苏佩里",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Time is a created thing. To say 'I don't have time' is like saying 'I don't want to.'",
            author: "Lao Tzu",
        },
        ru: Quote {
            text: "Время — вещь сотворённая. Сказать «у меня нет времени» — всё равно что сказать «я не хочу».",
            author: "Лао-цзы",
        },
        zh_cn: Quote {
            text: "时间是人创造的尺度。说“没有时间”，往往只是在说“不愿如此”。",
            author: "老子",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "Short as life is, we make it still shorter by the careless waste of time.",
            author: "Victor Hugo",
        },
        ru: Quote {
            text: "Жизнь и без того коротка, а мы делаем её ещё короче, беспечно растрачивая время.",
            author: "Виктор Гюго",
        },
        zh_cn: Quote {
            text: "人生本已短暂，我们却因漫不经心地挥霍时间而令它更加短促。",
            author: "维克多·雨果",
        },
    },
    LocalizedQuote {
        en: Quote {
            text: "We are born once and cannot be born twice; yet you, who are not master of tomorrow, postpone your joy.",
            author: "Epicurus",
        },
        ru: Quote {
            text: "Мы рождаемся лишь однажды и не можем родиться вновь; ты же, не властный над завтрашним днём, откладываешь радость.",
            author: "Эпикур",
        },
        zh_cn: Quote {
            text: "人生只有一次，无法重来；而你既不能主宰明天，又何必推迟今日的喜悦。",
            author: "伊壁鸠鲁",
        },
    },
];

pub fn quote_for(locale: Locale, index: usize) -> Quote {
    let entry = &QUOTES[index % QUOTES.len()];
    match locale {
        Locale::En => entry.en,
        Locale::Ru => entry.ru,
        Locale::ZhCn => entry.zh_cn,
    }
}

pub fn default_index_for_date(date: NaiveDate) -> usize {
    (date.ordinal0() as usize) % QUOTES.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_quotes_are_populated_across_locales() {
        assert_eq!(QUOTES.len(), 24);
        for index in 0..QUOTES.len() {
            for locale in [Locale::En, Locale::Ru, Locale::ZhCn] {
                let q = quote_for(locale, index);
                assert!(!q.text.trim().is_empty(), "empty text at {index} for {locale:?}");
                assert!(!q.author.trim().is_empty(), "empty author at {index} for {locale:?}");
            }
        }
        // Index wraps cleanly beyond QUOTE_COUNT.
        assert_eq!(quote_for(Locale::En, 0), quote_for(Locale::En, QUOTE_COUNT));
        assert_eq!(quote_for(Locale::Ru, 14), quote_for(Locale::Ru, 14 + QUOTE_COUNT * 3));
    }

    #[test]
    fn default_index_uses_ordinal0_modulo_len() {
        let jan1 = NaiveDate::from_ymd_opt(2026, 1, 1).expect("valid date");
        let jan2 = NaiveDate::from_ymd_opt(2026, 1, 2).expect("valid date");
        let nov23 = NaiveDate::from_ymd_opt(2026, 11, 23).expect("valid date");
        assert_eq!(default_index_for_date(jan1), 0);
        assert_eq!(default_index_for_date(jan2), 1);
        assert_eq!(default_index_for_date(nov23), (nov23.ordinal0() as usize) % QUOTE_COUNT);
        assert_eq!(quote_for(Locale::En, default_index_for_date(nov23)).author, "Bill Gates");
    }
}
