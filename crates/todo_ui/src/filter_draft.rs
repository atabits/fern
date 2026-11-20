//! Text belongs to the editor until validated; invalid input never enters a stored rule.
use fern_core::{Condition, ConditionValue, DateRule};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inputs {
    pub keywords: String,
    pub days: String,
    pub from: String,
    pub to: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputError {
    Days,
    Range,
}
impl Inputs {
    pub fn new(value: &ConditionValue) -> Self {
        let mut input = Self {
            keywords: String::new(),
            days: "7".into(),
            from: "0".into(),
            to: "6".into(),
        };
        match value {
            ConditionValue::Include(text) => input.keywords = text.clone(),
            ConditionValue::Date(dates) => {
                for date in dates {
                    match date {
                        DateRule::NextDays(n) => input.days = n.to_string(),
                        DateRule::Span { from, to } => {
                            input.from = from.map_or(String::new(), |n| n.to_string());
                            input.to = to.map_or(String::new(), |n| n.to_string());
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        input
    }
    pub fn apply(&self, condition: &Condition) -> Result<Condition, InputError> {
        let mut condition = condition.clone();
        match &mut condition.value {
            ConditionValue::Include(text) => *text = self.keywords.trim().to_owned(),
            ConditionValue::Date(dates) => {
                for date in dates {
                    *date = match *date {
                        DateRule::NextDays(_) => DateRule::NextDays(
                            DateRule::parse_days(&self.days).ok_or(InputError::Days)?,
                        ),
                        DateRule::Span { .. } => DateRule::parse_span(&self.from, &self.to)
                            .map_err(|_| InputError::Range)?,
                        rule => rule,
                    }
                }
            }
            _ => {}
        }
        Ok(condition)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactive_invalid_input_does_not_block_save() {
        let date = Condition::new(ConditionValue::Date(vec![DateRule::NextDays(14)]));
        let mut input = Inputs::new(&date.value);
        for bad in ["", "0", "-1", "1.5", "4294967296"] {
            input.days = bad.into();
            assert_eq!(input.apply(&date), Err(InputError::Days));
            assert_eq!(
                date.value,
                ConditionValue::Date(vec![DateRule::NextDays(14)])
            );
        }
        let priority = Condition::new(ConditionValue::Priority(vec![fern_core::Priority::High]));
        assert_eq!(input.apply(&priority), Ok(priority));
        input.days = "14".into();
        assert_eq!(input.apply(&date), Ok(date));
    }
    #[test]
    fn ranges_reopen_and_validate_both_endpoints() {
        for (from, to) in [
            (Some(-3), Some(7)),
            (None, Some(0)),
            (Some(14), None),
            (None, None),
        ] {
            let condition = Condition::new(ConditionValue::Date(vec![DateRule::Span { from, to }]));
            let mut input = Inputs::new(&condition.value);
            assert_eq!(input.apply(&condition), Ok(condition.clone()));
            input.from = "8".into();
            input.to = "7".into();
            assert_eq!(input.apply(&condition), Err(InputError::Range));
        }
    }
}
