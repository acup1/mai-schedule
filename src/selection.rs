use std::collections::HashSet;

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, Utc, Weekday};

use crate::models::Lesson;
use crate::{AppError, AppResult};

pub fn monday_for(value: NaiveDate) -> NaiveDate {
    value - Duration::days(i64::from(value.weekday().num_days_from_monday()))
}

pub fn week_key(value: NaiveDate) -> String {
    let week = value.iso_week();
    format!("{}-W{:02}", week.year(), week.week())
}

pub fn today_in_mai() -> NaiveDate {
    let offset = FixedOffset::east_opt(3 * 60 * 60).expect("valid Moscow offset");
    let now: DateTime<FixedOffset> = Utc::now().with_timezone(&offset);
    now.date_naive()
}

pub fn current_schedule_monday(today: Option<NaiveDate>) -> NaiveDate {
    let today = today.unwrap_or_else(today_in_mai);
    let monday = monday_for(today);
    if today.weekday() == Weekday::Sun {
        monday + Duration::weeks(1)
    } else {
        monday
    }
}

pub fn parse_week(value: &str, today: Option<NaiveDate>) -> AppResult<NaiveDate> {
    let today = today.unwrap_or_else(today_in_mai);
    let current = current_schedule_monday(Some(today));
    let normalized = value.trim().to_lowercase();
    match normalized.as_str() {
        "current" | "now" | "текущая" => return Ok(current),
        "next" | "следующая" => return Ok(current + Duration::weeks(1)),
        "previous" | "prev" | "предыдущая" => {
            return Ok(current - Duration::weeks(1));
        }
        _ => {}
    }
    if (normalized.starts_with('+') || normalized.starts_with('-'))
        && normalized[1..]
            .chars()
            .all(|character| character.is_ascii_digit())
    {
        let offset: i64 = normalized
            .parse()
            .map_err(|_| AppError::new(format!("Некорректное смещение недели: {value}")))?;
        return Ok(current + Duration::weeks(offset));
    }
    if let Some((year, week)) = parse_iso_week(&normalized) {
        return NaiveDate::from_isoywd_opt(year, week, Weekday::Mon)
            .ok_or_else(|| AppError::new(format!("Некорректная ISO-неделя: {value}")));
    }
    if normalized
        .chars()
        .all(|character| character.is_ascii_digit())
        && normalized.len() <= 2
    {
        let week: u32 = normalized
            .parse()
            .map_err(|_| AppError::new(format!("Некорректный номер недели: {value}")))?;
        return NaiveDate::from_isoywd_opt(today.iso_week().year(), week, Weekday::Mon)
            .ok_or_else(|| AppError::new(format!("Некорректный номер недели: {value}")));
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(monday_for)
        .map_err(|_| {
            AppError::new(format!(
                "Неизвестная неделя «{value}»: используйте current, +1, YYYY-Www или YYYY-MM-DD"
            ))
        })
}

fn parse_iso_week(value: &str) -> Option<(i32, u32)> {
    let (year, week) = value.split_once("-w")?;
    if year.len() != 4 || week.is_empty() || week.len() > 2 {
        return None;
    }
    Some((year.parse().ok()?, week.parse().ok()?))
}

pub fn expand_week_selectors(
    values: &[String],
    today: Option<NaiveDate>,
) -> AppResult<Vec<NaiveDate>> {
    let selectors: Vec<&str> = if values.is_empty() {
        vec!["current"]
    } else {
        values.iter().map(String::as_str).collect()
    };
    let mut result = Vec::new();
    for selector in selectors {
        if let Some((left, right)) = selector.split_once("..") {
            let start = parse_week(left, today)?;
            let end = parse_week(right, today)?;
            if end < start {
                return Err(AppError::new(format!(
                    "Конец диапазона раньше начала: {selector}"
                )));
            }
            if (end - start).num_days() > 7 * 104 {
                return Err(AppError::new("Диапазон не может быть длиннее 104 недель"));
            }
            let mut cursor = start;
            while cursor <= end {
                if !result.contains(&cursor) {
                    result.push(cursor);
                }
                cursor += Duration::weeks(1);
            }
        } else {
            let monday = parse_week(selector, today)?;
            if !result.contains(&monday) {
                result.push(monday);
            }
        }
    }
    Ok(result)
}

pub fn select_weeks(lessons: &[Lesson], mondays: &[NaiveDate]) -> Vec<Lesson> {
    let keys: HashSet<String> = mondays.iter().copied().map(week_key).collect();
    lessons
        .iter()
        .filter(|lesson| keys.contains(&lesson.week()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn sunday_selects_next_week() {
        assert_eq!(
            current_schedule_monday(Some(date(2026, 9, 20))),
            date(2026, 9, 21)
        );
    }

    #[test]
    fn expands_ranges_and_deduplicates() {
        let values = vec!["2026-W38..2026-W40".into(), "2026-W39".into()];
        let weeks = expand_week_selectors(&values, Some(date(2026, 9, 14))).unwrap();
        assert_eq!(
            weeks,
            vec![date(2026, 9, 14), date(2026, 9, 21), date(2026, 9, 28)]
        );
    }
}
