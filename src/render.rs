use chrono::{Duration, NaiveDate};
use serde_json::{Map, Value};

use crate::AppResult;
use crate::models::Lesson;
use crate::selection::week_key;

pub fn render_json(
    group: &str,
    lessons: &[Lesson],
    kind: &str,
    week: Option<&str>,
) -> AppResult<String> {
    let mut payload = Map::new();
    payload.insert("group".into(), Value::String(group.into()));
    payload.insert("kind".into(), Value::String(kind.into()));
    if let Some(week) = week {
        payload.insert("week".into(), Value::String(week.into()));
    }
    payload.insert("lessons".into(), serde_json::to_value(lessons)?);
    Ok(serde_json::to_string_pretty(&payload)?)
}

pub fn render_text(group: &str, lessons: &[Lesson], title: Option<&str>) -> String {
    let mut lines = vec![format!("Группа: {group}")];
    if let Some(title) = title {
        lines.push(title.to_owned());
    }
    if lessons.is_empty() {
        lines.push("Нет занятий.".into());
        return lines.join("\n");
    }
    let distinct_weeks: std::collections::HashSet<String> =
        lessons.iter().map(Lesson::week).collect();
    let show_week_headers = distinct_weeks.len() > 1 || !title.unwrap_or("").starts_with("Неделя ");
    let mut current_week = String::new();
    let mut current_date = None;
    for lesson in lessons {
        let week = lesson.week();
        if week != current_week {
            current_week = week.clone();
            if show_week_headers {
                lines.push(String::new());
                lines.push(format!("Неделя {week}"));
            }
            current_date = None;
        }
        if current_date != Some(lesson.date) {
            current_date = Some(lesson.date);
            lines.push(String::new());
            lines.push(format!(
                "{} ({})",
                lesson.date.format("%d.%m.%Y"),
                lesson.weekday
            ));
        }
        let details = [lesson.rooms.join(", "), lesson.lecturers.join(", ")]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" | ");
        let kind = if lesson.kind.is_empty() {
            "—"
        } else {
            &lesson.kind
        };
        let mut row = format!(
            "  {}–{}  [{kind}] {}",
            short_time(&lesson.time_start),
            short_time(&lesson.time_end),
            lesson.name
        );
        if !details.is_empty() {
            row.push_str(" — ");
            row.push_str(&details);
        }
        lines.push(row);
        for link in &lesson.links {
            lines.push(format!("      {link}"));
        }
    }
    lines.join("\n")
}

pub fn week_title(monday: NaiveDate) -> String {
    let sunday = monday + Duration::days(6);
    format!(
        "Неделя {}: {}–{}",
        week_key(monday),
        monday.format("%d.%m.%Y"),
        sunday.format("%d.%m.%Y")
    )
}

fn short_time(value: &str) -> String {
    let mut parts = value.split(':');
    match (parts.next(), parts.next()) {
        (Some(hour), Some(minute)) => match (hour.parse::<u32>(), minute.parse::<u32>()) {
            (Ok(hour), Ok(minute)) => format!("{hour:02}:{minute:02}"),
            _ => value.into(),
        },
        _ => value.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn title_crosses_year_boundaries() {
        let monday = NaiveDate::from_ymd_opt(2025, 12, 29).unwrap();
        assert_eq!(week_title(monday), "Неделя 2026-W01: 29.12.2025–04.01.2026");
        assert_eq!(monday.iso_week().year(), 2026);
    }
}
