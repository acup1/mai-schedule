use std::time::Duration;

use chrono::NaiveDate;
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use serde_json::Value;
use url::Url;

use crate::models::Lesson;
use crate::{AppError, AppResult};

const GROUPS_URL: &str = "https://mai.ru/education/studies/schedule/groups.php";
const DATA_URL: &str = "https://public.mai.ru/schedule/data";
const USER_AGENT: &str = "mai-schedule/2.0 (+https://github.com/acup1/mai-schedule)";

#[derive(Clone)]
pub struct MaiScheduleClient {
    client: Client,
}

impl MaiScheduleClient {
    pub fn new(timeout_seconds: f64) -> AppResult<Self> {
        if !timeout_seconds.is_finite() || timeout_seconds <= 0.0 {
            return Err(AppError::new("--timeout должен быть положительным числом"));
        }
        let client = Client::builder()
            .timeout(Duration::from_secs_f64(timeout_seconds))
            .danger_accept_invalid_certs(true)
            .user_agent(USER_AGENT)
            .build()?;
        Ok(Self { client })
    }

    fn get_bytes(&self, url: &str) -> AppResult<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .header("Accept", "application/json,text/html")
            .send()
            .map_err(|error| {
                AppError::new(format!("Не удалось получить данные с {url}: {error}"))
            })?;
        if response.status().is_success() {
            return response
                .bytes()
                .map(|bytes| bytes.to_vec())
                .map_err(AppError::from);
        }
        Err(AppError::new(format!(
            "МАИ вернул HTTP {} для {url}",
            response.status()
        )))
    }

    fn groups_page(&self, url: &str) -> AppResult<Html> {
        let bytes = self.get_bytes(url)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| AppError::new("Страница списка групп МАИ имеет неизвестную кодировку"))?;
        Ok(Html::parse_document(&text))
    }

    pub fn catalog(&self) -> AppResult<(Vec<String>, Vec<String>)> {
        let page = self.groups_page(GROUPS_URL)?;
        let institutes = option_values(&page, "select[name=department] option")?;
        let courses = option_values(&page, "select[name=course] option")?;
        if institutes.is_empty() || courses.is_empty() {
            return Err(AppError::new(
                "На странице МАИ не найден список институтов или курсов",
            ));
        }
        Ok((institutes, courses))
    }

    pub fn list_groups(&self, institute: &str, course: &str) -> AppResult<Vec<String>> {
        let mut url = Url::parse(GROUPS_URL)?;
        url.query_pairs_mut()
            .append_pair("department", institute)
            .append_pair("course", course);
        let page = self.groups_page(url.as_str())?;
        let selector = Selector::parse("a[href]").expect("static selector is valid");
        let mut groups = Vec::new();
        for element in page.select(&selector) {
            let Some(href) = element.value().attr("href") else {
                continue;
            };
            let Ok(link) = url.join(href) else {
                continue;
            };
            for (name, value) in link.query_pairs() {
                if name == "group" {
                    push_unique(&mut groups, value.trim());
                }
            }
        }
        if groups.is_empty() {
            return Err(AppError::new(format!(
                "Для «{institute}», курс {course}, группы не найдены"
            )));
        }
        Ok(groups)
    }

    pub fn list_all_groups(&self) -> AppResult<Vec<String>> {
        let (institutes, _) = self.catalog()?;
        let mut groups = Vec::new();
        for institute in institutes {
            for group in self.list_groups(&institute, "all")? {
                push_unique(&mut groups, &group);
            }
        }
        Ok(groups)
    }

    pub fn fetch(&self, group: &str) -> AppResult<Vec<Lesson>> {
        let group = group.trim();
        if group.is_empty() {
            return Err(AppError::new("Имя группы не может быть пустым"));
        }
        let digest = format!("{:x}", md5::compute(group.as_bytes()));
        let bytes = self.get_bytes(&format!("{DATA_URL}/{digest}.json"))?;
        parse_schedule(group, &bytes)
    }
}

fn parse_schedule(group: &str, bytes: &[u8]) -> AppResult<Vec<Lesson>> {
    let raw: Value = serde_json::from_slice(bytes)
        .map_err(|_| AppError::new(format!("Некорректный JSON расписания группы {group}")))?;
    let object = raw.as_object().ok_or_else(|| {
        AppError::new(format!(
            "Некорректный JSON расписания группы {group}: ожидался объект"
        ))
    })?;
    let source_group = object
        .get("group")
        .and_then(Value::as_str)
        .unwrap_or(group)
        .trim();
    if !source_group.eq_ignore_ascii_case(group)
        && source_group.to_lowercase() != group.to_lowercase()
    {
        return Err(AppError::new(format!(
            "Источник вернул расписание другой группы: {source_group}"
        )));
    }

    let mut lessons = Vec::new();
    for (date_text, day_value) in object {
        if date_text == "group" {
            continue;
        }
        let lesson_date = NaiveDate::parse_from_str(date_text, "%d.%m.%Y")
            .map_err(|_| AppError::new(format!("Формат расписания группы {group} изменился")))?;
        let day = day_value
            .as_object()
            .ok_or_else(|| source_format_error(group))?;
        let weekday = day
            .get("day")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let pairs = day
            .get("pairs")
            .and_then(Value::as_object)
            .ok_or_else(|| source_format_error(group))?;
        for slot in pairs.values() {
            let subjects = slot.as_object().ok_or_else(|| source_format_error(group))?;
            for (name, item_value) in subjects {
                let item = item_value
                    .as_object()
                    .ok_or_else(|| source_format_error(group))?;
                let kinds: Vec<String> = item
                    .get("type")
                    .and_then(Value::as_object)
                    .map(|types| types.keys().cloned().collect())
                    .filter(|types: &Vec<String>| !types.is_empty())
                    .unwrap_or_else(|| vec![String::new()]);
                let lecturers = non_empty_values(item.get("lector"));
                let rooms = non_empty_values(item.get("room"));
                let links: Vec<String> = ["lms", "teams", "other"]
                    .into_iter()
                    .filter_map(|key| item.get(key).and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect();
                for kind in kinds {
                    lessons.push(Lesson {
                        date: lesson_date,
                        weekday: weekday.clone(),
                        time_start: string_field(item, "time_start"),
                        time_end: string_field(item, "time_end"),
                        name: name.trim().to_owned(),
                        kind: kind.trim().to_owned(),
                        lecturers: lecturers.clone(),
                        rooms: rooms.clone(),
                        links: links.clone(),
                    });
                }
            }
        }
    }
    lessons.sort_by(|left, right| {
        (
            left.date,
            time_key(&left.time_start),
            &left.name,
            &left.kind,
        )
            .cmp(&(
                right.date,
                time_key(&right.time_start),
                &right.name,
                &right.kind,
            ))
    });
    Ok(lessons)
}

fn option_values(page: &Html, selector: &str) -> AppResult<Vec<String>> {
    let selector = Selector::parse(selector).map_err(|error| AppError::new(error.to_string()))?;
    let mut values = Vec::new();
    for option in page.select(&selector) {
        let value = option.value().attr("value").unwrap_or("").trim();
        if !value.is_empty() && value != "all" {
            push_unique(&mut values, value);
        }
    }
    Ok(values)
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !values.iter().any(|existing| existing == value) {
        values.push(value.to_owned());
    }
}

fn string_field(object: &serde_json::Map<String, Value>, key: &str) -> String {
    object
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned()
}

fn non_empty_values(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_object)
        .map(|object| {
            object
                .values()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn time_key(value: &str) -> (u32, u32, u32) {
    let mut parts = value.split(':').map(str::parse::<u32>);
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Ok(hour)), Some(Ok(minute)), second) => {
            (hour, minute, second.and_then(Result::ok).unwrap_or(0))
        }
        _ => (99, 99, 99),
    }
}

fn source_format_error(group: &str) -> AppError {
    AppError::new(format!("Формат расписания группы {group} изменился"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sorts_the_published_schedule_shape() {
        let fixture = r#"{
          "group": "М3О-101БВ-26",
          "15.09.2026": {
            "day": "Вт",
            "pairs": {
              "2": {
                "Физика": {
                  "time_start": "10:45:00",
                  "time_end": "12:15:00",
                  "type": {"ЛР": {}},
                  "lector": {"1": "Иванов И.И."},
                  "room": {"1": "ГУК Б-201"},
                  "lms": "https://example.test/course",
                  "teams": "",
                  "other": ""
                }
              },
              "1": {
                "Математика": {
                  "time_start": "09:00:00",
                  "time_end": "10:30:00",
                  "type": {"ЛК": {}},
                  "lector": {},
                  "room": {}
                }
              }
            }
          }
        }"#;
        let lessons = parse_schedule("М3О-101БВ-26", fixture.as_bytes()).unwrap();

        assert_eq!(lessons.len(), 2);
        assert_eq!(lessons[0].name, "Математика");
        assert_eq!(lessons[1].kind, "ЛР");
        assert_eq!(lessons[1].rooms, ["ГУК Б-201"]);
        assert_eq!(lessons[1].links, ["https://example.test/course"]);
    }

    #[test]
    fn rejects_a_schedule_for_another_group() {
        let error = parse_schedule("A", br#"{"group":"B"}"#).unwrap_err();
        assert!(error.to_string().contains("другой группы"));
    }
}
