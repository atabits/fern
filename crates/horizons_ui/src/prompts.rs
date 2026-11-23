//! Soft reflection prompts shown in the bottom-right corner of Horizons.

use fern_i18n::Locale;

pub(crate) const PROMPT_COUNT: usize = 20;

const EN_PROMPTS: [&str; PROMPT_COUNT] = [
    "What would you do if you weren't afraid?",
    "What single task today would make this week feel meaningful?",
    "Which commitment can you let go of to make room for what matters?",
    "What will your future self thank you for starting this month?",
    "Where is your attention going that deserves less of it?",
    "What is one bold goal worth placing on your year horizon?",
    "If this week had only three wins, what would they be?",
    "What small habit today compounds into the life you want?",
    "What are you postponing simply because the first step feels unclear?",
    "Who would you love to reconnect with before this month ends?",
    "What would look effortless if you simplified it by half?",
    "Which task on your list brings energy rather than drains it?",
    "What skill would transform your next decade if you practiced it weekly?",
    "What does a calm, well-spent evening look like tonight?",
    "What promise to yourself deserves priority over urgent noise?",
    "If you could finish only one thing before Friday, what is it?",
    "What unfinished project still sparks curiosity in you?",
    "How can you make today's hardest task ten percent lighter?",
    "What milestone this year will you remember five years from now?",
    "What matters enough to carry across seasons, not just days?",
];

const RU_PROMPTS: [&str; PROMPT_COUNT] = [
    "Что бы вы сделали, если бы ничего не боялись?",
    "Какая одна задача сегодня сделает эту неделю по-настоящему значимой?",
    "От какого обязательства можно отказаться, чтобы освободить место главному?",
    "За какое начинание в этом месяце вы поблагодарите себя через год?",
    "Куда сейчас уходит внимание, хотя оно того не стоит?",
    "Какую смелую цель стоит поставить на горизонт этого года?",
    "Если бы на этой неделе было только три победы — какие они?",
    "Какая маленькая привычка сегодня строит жизнь, которой вы хотите жить?",
    "Что вы откладываете лишь потому, что первый шаг кажется неясным?",
    "С кем хочется восстановить тёплую связь до конца этого месяца?",
    "Что стало бы лёгким, если упростить это вдвое?",
    "Какая задача в вашем списке наполняет энергией, а не забирает её?",
    "Какой навык изменит ваше десятилетие, если уделять ему час каждую неделю?",
    "Как выглядит спокойный и наполненный вечер именно сегодня?",
    "Какое обещание себе важнее сегодняшней суеты?",
    "Если до пятницы можно завершить лишь одно дело — что это будет?",
    "Какой незавершённый проект всё ещё вызывает у вас живой интерес?",
    "Как сделать самую сложную задачу дня хотя бы на десять процентов проще?",
    "Какое событие этого года вы будете с теплом вспоминать через пять лет?",
    "Что достаточно важно, чтобы нести это сквозь годы, а не только дни?",
];

const ZH_CN_PROMPTS: [&str; PROMPT_COUNT] = [
    "如果无所畏惧，你会做什么？",
    "今天完成哪一件事，会让这一周变得更有意义？",
    "放下哪项承诺，能为真正重要的事腾出空间？",
    "这个月开始做哪件事，未来的你会感激现在的自己？",
    "你的注意力正流向哪里，其实并不值得那么多投入？",
    "有什么大胆的目标，值得放进今年的视野里？",
    "如果这周只追求三个成果，它们会是什么？",
    "今天哪一个微小的习惯，会复利成你想要的人生？",
    "哪件事你一直在拖延，只是因为第一步还不够清晰？",
    "在这个月结束前，你想重新联系哪位重要的人？",
    "如果把眼前的计划精简一半，什么会变得轻松起来？",
    "清单上的哪件事能带给你能量，而不是消耗你？",
    "如果每周坚持练习一项技能，哪一项会改变你的下一个十年？",
    "一个平静而充实的今晚，对你来说是什么样子？",
    "哪一个对自己的约定，比眼前的琐事更值得优先？",
    "如果周五前只能完成一件事，你会选择哪一件？",
    "哪个尚未完成的项目，至今仍然让你心生好奇？",
    "如何让今天最难的那项任务变得稍微轻松一点？",
    "今年的哪一个里程碑，五年后你依然会清晰记得？",
    "什么事情足够重要，值得跨越季节而非只停留在一天？",
];

pub(crate) fn prompt_for(locale: Locale, index: usize) -> &'static str {
    let table = match locale {
        Locale::En => &EN_PROMPTS,
        Locale::Ru => &RU_PROMPTS,
        Locale::ZhCn => &ZH_CN_PROMPTS,
    };
    table[index % table.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_locales_have_20_non_empty_prompts_and_wrap() {
        assert_eq!(prompt_for(Locale::Ru, 0), "Что бы вы сделали, если бы ничего не боялись?");
        assert_eq!(prompt_for(Locale::En, 0), "What would you do if you weren't afraid?");
        assert_eq!(prompt_for(Locale::ZhCn, 0), "如果无所畏惧，你会做什么？");
        for locale in [Locale::En, Locale::Ru, Locale::ZhCn] {
            for idx in 0..PROMPT_COUNT {
                assert!(!prompt_for(locale, idx).trim().is_empty());
            }
            assert_eq!(prompt_for(locale, PROMPT_COUNT), prompt_for(locale, 0));
        }
    }
}
