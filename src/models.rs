use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Lesson {
    pub date: NaiveDate,
    pub weekday: String,
    pub time_start: String,
    pub time_end: String,
    pub name: String,
    pub kind: String,
    pub lecturers: Vec<String>,
    pub rooms: Vec<String>,
    pub links: Vec<String>,
}

impl Lesson {
    pub fn week(&self) -> String {
        let week = self.date.iso_week();
        format!("{}-W{:02}", week.year(), week.week())
    }
}

fn normalise_kind(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_whitespace() && !matches!(character, '.' | '_' | '-'))
        .flat_map(char::to_uppercase)
        .map(|character| if character == 'Ё' { 'Е' } else { character })
        .collect()
}

pub fn is_lab(lesson: &Lesson) -> bool {
    let kind = normalise_kind(&lesson.kind);
    matches!(kind.as_str(), "ЛР" | "ЛАБ" | "LAB") || kind.contains("ЛАБОРАТОР")
}

pub fn is_exam(lesson: &Lesson) -> bool {
    let kind = normalise_kind(&lesson.kind);
    matches!(kind.as_str(), "Э" | "ЭК" | "ЭКЗ" | "EXAM") || kind.contains("ЭКЗАМЕН")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lesson(kind: &str) -> Lesson {
        Lesson {
            date: NaiveDate::from_ymd_opt(2026, 9, 14).unwrap(),
            weekday: "Пн".into(),
            time_start: "09:00:00".into(),
            time_end: "10:30:00".into(),
            name: "Test".into(),
            kind: kind.into(),
            lecturers: vec![],
            rooms: vec![],
            links: vec![],
        }
    }

    #[test]
    fn recognises_labs_and_exams() {
        assert!(is_lab(&lesson("Л.Р.")));
        assert!(is_lab(&lesson("лабораторная работа")));
        assert!(is_exam(&lesson("Экз.")));
        assert!(!is_exam(&lesson("ЛР")));
    }
}
