//! The emoji the list icon picker offers, by the original picker's categories, each with English
//! words to search by.

/// The categories, in the picker's order, with the emoji shown on their tab.
pub const CATEGORIES: [(&str, &[(&str, &str)]); 8] = [
    ("😀", PEOPLE),
    ("🐶", NATURE),
    ("🍔", FOOD),
    ("⚽", ACTIVITIES),
    ("🚗", TRAVEL),
    ("💡", OBJECTS),
    ("💯", SYMBOLS),
    ("🚩", FLAGS),
];

const PEOPLE: &[(&str, &str)] = &[
    ("😀", "grinning smile happy"),
    ("😃", "smile happy joy"),
    ("😄", "smile laugh happy"),
    ("😁", "grin beam"),
    ("😆", "laugh squint"),
    ("😅", "sweat relief"),
    ("🤣", "rolling laugh"),
    ("😂", "joy tears laugh"),
    ("🙂", "slight smile"),
    ("🙃", "upside down"),
    ("😉", "wink"),
    ("😊", "blush smile"),
    ("😇", "halo angel"),
    ("🥰", "love hearts"),
    ("😍", "heart eyes love"),
    ("🤩", "star struck"),
    ("😘", "kiss"),
    ("😋", "yum tasty"),
    ("😛", "tongue"),
    ("🤔", "thinking"),
    ("😐", "neutral"),
    ("😴", "sleep tired"),
    ("😎", "cool sunglasses"),
    ("🤓", "nerd study"),
    ("🥳", "party celebrate"),
    ("😭", "cry sad"),
    ("😡", "angry"),
    ("🤯", "mind blown"),
    ("👍", "thumbs up like"),
    ("👏", "clap"),
    ("🙏", "pray thanks"),
    ("💪", "muscle strong gym"),
    ("👀", "eyes look"),
    ("🧠", "brain mind"),
    ("👶", "baby child"),
    ("👨‍👩‍👧", "family"),
];

const NATURE: &[(&str, &str)] = &[
    ("🐶", "dog pet"),
    ("🐱", "cat pet"),
    ("🐭", "mouse"),
    ("🐰", "rabbit bunny"),
    ("🦊", "fox"),
    ("🐻", "bear"),
    ("🐼", "panda"),
    ("🐨", "koala"),
    ("🐯", "tiger"),
    ("🦁", "lion"),
    ("🐮", "cow"),
    ("🐷", "pig"),
    ("🐸", "frog"),
    ("🐵", "monkey"),
    ("🐔", "chicken"),
    ("🐧", "penguin"),
    ("🐦", "bird"),
    ("🦋", "butterfly"),
    ("🐝", "bee"),
    ("🐢", "turtle"),
    ("🐟", "fish"),
    ("🐳", "whale"),
    ("🌵", "cactus plant"),
    ("🌲", "tree forest"),
    ("🌿", "herb leaf"),
    ("🍀", "clover luck"),
    ("🌸", "blossom flower"),
    ("🌻", "sunflower"),
    ("🌹", "rose flower"),
    ("🪴", "plant pot garden"),
    ("☀️", "sun weather"),
    ("🌙", "moon night"),
    ("⭐", "star"),
    ("🔥", "fire hot"),
    ("🌈", "rainbow"),
    ("❄️", "snow winter"),
];

const FOOD: &[(&str, &str)] = &[
    ("🍎", "apple fruit"),
    ("🍌", "banana fruit"),
    ("🍇", "grapes fruit"),
    ("🍓", "strawberry"),
    ("🍉", "watermelon"),
    ("🍋", "lemon"),
    ("🥑", "avocado"),
    ("🥕", "carrot vegetable"),
    ("🥦", "broccoli vegetable"),
    ("🍞", "bread bakery"),
    ("🧀", "cheese"),
    ("🥚", "egg"),
    ("🍳", "cooking breakfast"),
    ("🥗", "salad diet"),
    ("🍔", "burger"),
    ("🍕", "pizza"),
    ("🌮", "taco"),
    ("🍣", "sushi"),
    ("🍜", "noodles ramen"),
    ("🍝", "pasta"),
    ("🍰", "cake dessert"),
    ("🍪", "cookie"),
    ("🍫", "chocolate"),
    ("🍿", "popcorn movie"),
    ("☕", "coffee"),
    ("🍵", "tea"),
    ("🥤", "drink soda"),
    ("🍺", "beer"),
    ("🍷", "wine"),
    ("🛒", "shopping cart groceries"),
];

const ACTIVITIES: &[(&str, &str)] = &[
    ("⚽", "soccer football"),
    ("🏀", "basketball"),
    ("🏈", "american football"),
    ("⚾", "baseball"),
    ("🎾", "tennis"),
    ("🏐", "volleyball"),
    ("🏓", "ping pong"),
    ("🏸", "badminton"),
    ("🥊", "boxing"),
    ("🏋️", "weight lifting gym"),
    ("🧘", "yoga meditation"),
    ("🏃", "run running"),
    ("🚴", "cycling bike"),
    ("🏊", "swim swimming"),
    ("⛷️", "ski"),
    ("🧗", "climbing"),
    ("🎯", "target goal"),
    ("🎮", "game video"),
    ("🎲", "dice board game"),
    ("🧩", "puzzle"),
    ("🎨", "art paint"),
    ("🎬", "movie film"),
    ("🎤", "sing microphone"),
    ("🎧", "music headphones"),
    ("🎸", "guitar"),
    ("🎹", "piano"),
    ("📷", "photo camera"),
    ("🏆", "trophy win"),
    ("🎉", "party"),
    ("🎁", "gift present"),
];

const TRAVEL: &[(&str, &str)] = &[
    ("🚗", "car drive"),
    ("🚕", "taxi"),
    ("🚌", "bus"),
    ("🚲", "bicycle"),
    ("🛵", "scooter"),
    ("🚆", "train"),
    ("🚇", "metro subway"),
    ("✈️", "plane flight travel"),
    ("🚀", "rocket launch"),
    ("🚢", "ship boat"),
    ("⛵", "sailboat"),
    ("🗺️", "map"),
    ("🧭", "compass"),
    ("🏔️", "mountain"),
    ("🏕️", "camping"),
    ("🏖️", "beach vacation"),
    ("🏝️", "island"),
    ("🏠", "home house"),
    ("🏡", "garden house"),
    ("🏢", "office building work"),
    ("🏥", "hospital health"),
    ("🏫", "school"),
    ("🏦", "bank"),
    ("🏪", "store shop"),
    ("⛪", "church"),
    ("🗽", "statue"),
    ("🌍", "world earth"),
    ("🌃", "city night"),
];

const OBJECTS: &[(&str, &str)] = &[
    ("💡", "idea light bulb"),
    ("📱", "phone mobile"),
    ("💻", "laptop computer work"),
    ("🖥️", "desktop computer"),
    ("⌨️", "keyboard"),
    ("🖨️", "printer"),
    ("📚", "books read study"),
    ("📖", "book read"),
    ("📝", "memo note write"),
    ("✏️", "pencil"),
    ("📌", "pin"),
    ("📎", "paperclip"),
    ("📅", "calendar date"),
    ("📊", "chart report"),
    ("📈", "growth chart"),
    ("💼", "briefcase work"),
    ("📁", "folder"),
    ("🗂️", "dividers files"),
    ("✉️", "mail letter"),
    ("📦", "package box"),
    ("🔑", "key"),
    ("🔒", "lock"),
    ("🔧", "wrench tool fix"),
    ("🔨", "hammer tool"),
    ("🧹", "broom clean"),
    ("🧺", "laundry basket"),
    ("💊", "pill medicine"),
    ("💰", "money bag"),
    ("💳", "card payment"),
    ("⏰", "alarm clock"),
    ("🛏️", "bed sleep"),
    ("🎓", "graduation study"),
];

const SYMBOLS: &[(&str, &str)] = &[
    ("💯", "hundred perfect"),
    ("❤️", "heart love red"),
    ("🧡", "orange heart"),
    ("💛", "yellow heart"),
    ("💚", "green heart"),
    ("💙", "blue heart"),
    ("💜", "purple heart"),
    ("🖤", "black heart"),
    ("✅", "check done"),
    ("❌", "cross no"),
    ("⚠️", "warning"),
    ("❗", "exclamation important"),
    ("❓", "question"),
    ("💤", "sleep zzz"),
    ("♻️", "recycle"),
    ("⚡", "lightning energy"),
    ("✨", "sparkles"),
    ("🔔", "bell"),
    ("🎵", "music note"),
    ("➕", "plus"),
    ("🔴", "red circle"),
    ("🟠", "orange circle"),
    ("🟡", "yellow circle"),
    ("🟢", "green circle"),
    ("🔵", "blue circle"),
    ("🟣", "purple circle"),
    ("⬛", "black square"),
    ("⬜", "white square"),
];

const FLAGS: &[(&str, &str)] = &[
    ("🚩", "red flag"),
    ("🏁", "chequered finish"),
    ("🏳️", "white flag"),
    ("🏴", "black flag"),
    ("🏳️‍🌈", "rainbow flag"),
    ("🇺🇳", "united nations"),
    ("🇪🇺", "europe eu"),
    ("🇺🇸", "united states usa"),
    ("🇬🇧", "united kingdom uk"),
    ("🇩🇪", "germany"),
    ("🇫🇷", "france"),
    ("🇪🇸", "spain"),
    ("🇮🇹", "italy"),
    ("🇷🇺", "russia"),
    ("🇺🇦", "ukraine"),
    ("🇯🇵", "japan"),
    ("🇨🇳", "china"),
    ("🇮🇳", "india"),
    ("🇧🇷", "brazil"),
    ("🇨🇦", "canada"),
];

/// The emoji whose words contain the query, in the picker's order.
pub fn search(query: &str) -> Vec<&'static str> {
    let query = query.trim().to_lowercase();
    CATEGORIES
        .iter()
        .flat_map(|(_, items)| items.iter())
        .filter(|(_, words)| query.is_empty() || words.contains(&query))
        .map(|(emoji, _)| *emoji)
        .collect()
}

/// An emoji from all categories, picked by `seed`.
pub fn pick(seed: u64) -> &'static str {
    let all: Vec<&str> = CATEGORIES.iter().flat_map(|(_, items)| items.iter().map(|(e, _)| *e)).collect();
    all[(seed % all.len() as u64) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_by_words() {
        assert_eq!(search("pizza"), ["🍕"]);
        assert!(search("heart").contains(&"❤️"));
        assert!(search("").len() > 200);
        assert!(search("nothing like this").is_empty());
        assert_eq!(pick(0), "😀");
    }
}
