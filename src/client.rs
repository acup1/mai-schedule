use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, Weekday};
use reqwest::blocking::Client;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::models::Lesson;
use crate::{AppError, AppResult};

const GROUPS_URL: &str = "https://mai.ru/education/studies/schedule/groups.php";
const DATA_URL: &str = "https://public.mai.ru/schedule/data";
const FALLBACK_DATA_URL: &str = "https://schedule.maisch.ru/calendar/";
const USER_AGENT: &str = "mai-schedule/2.0 (+https://github.com/acup1/mai-schedule)";

#[derive(Clone)]
pub struct MaiScheduleClient {
    client: Client,
    cache_dir: PathBuf,
}

#[derive(Debug)]
pub struct FetchedSchedule {
    pub lessons: Vec<Lesson>,
    pub cached_at: Option<String>,
}

impl FetchedSchedule {
    pub fn cache_warning(&self) -> Option<String> {
        self.cached_at.as_ref().map(|updated_at| {
            format!(
                "Не удалось обновить расписание — показан кеш от {updated_at}, возможны неточности"
            )
        })
    }
}

#[derive(Deserialize, Serialize)]
struct CachedSchedule {
    group: String,
    updated_at: String,
    lessons: Vec<Lesson>,
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
        Ok(Self {
            client,
            cache_dir: default_cache_dir(),
        })
    }

    fn get_bytes(&self, url: &str) -> AppResult<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .header("Accept", "application/json,text/calendar,text/html")
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
            "Источник вернул HTTP {} для {url}",
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

    pub fn fetch(&self, group: &str) -> AppResult<FetchedSchedule> {
        let group = group.trim();
        if group.is_empty() {
            return Err(AppError::new("Имя группы не может быть пустым"));
        }

        let official_error = match self.fetch_official(group) {
            Ok(lessons) => return Ok(self.fresh_schedule(group, lessons)),
            Err(error) => error,
        };

        let fallback_error = match self.fetch_fallback(group) {
            Ok(lessons) => return Ok(self.fresh_schedule(group, lessons)),
            Err(error) => error,
        };

        if let Ok(Some(cached)) = read_cache(&self.cache_dir, group) {
            return Ok(FetchedSchedule {
                lessons: cached.lessons,
                cached_at: Some(cached.updated_at),
            });
        }

        Err(AppError::new(format!(
            "Не удалось загрузить расписание {group}. Официальный источник: {official_error}. Резервный источник: {fallback_error}"
        )))
    }

    fn fetch_official(&self, group: &str) -> AppResult<Vec<Lesson>> {
        let digest = format!("{:x}", md5::compute(group.as_bytes()));
        let bytes = self.get_bytes(&format!("{DATA_URL}/{digest}.json"))?;
        parse_schedule(group, &bytes)
    }

    fn fetch_fallback(&self, group: &str) -> AppResult<Vec<Lesson>> {
        let url = fallback_schedule_url(group)?;
        let bytes = self.get_bytes(url.as_str())?;
        parse_ics_schedule(group, &bytes)
    }

    fn fresh_schedule(&self, group: &str, lessons: Vec<Lesson>) -> FetchedSchedule {
        let cached = CachedSchedule {
            group: group.to_owned(),
            updated_at: Local::now().format("%d.%m.%Y %H:%M").to_string(),
            lessons,
        };
        let _ = write_cache(&self.cache_dir, &cached);
        FetchedSchedule {
            lessons: cached.lessons,
            cached_at: None,
        }
    }
}

fn fallback_schedule_url(group: &str) -> AppResult<Url> {
    let mut url = Url::parse(FALLBACK_DATA_URL)?;
    url.path_segments_mut()
        .map_err(|_| AppError::new("Некорректный URL резервного расписания"))?
        .pop_if_empty()
        .push(&format!("{group}.ics"));
    Ok(url)
}

fn default_cache_dir() -> PathBuf {
    if let Some(path) = env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(path).join("mai-schedule");
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home)
            .join("Library")
            .join("Caches")
            .join("mai-schedule");
    }
    #[cfg(target_os = "windows")]
    if let Some(path) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(path).join("mai-schedule");
    }
    if let Some(home) = env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join("mai-schedule");
    }
    env::temp_dir().join("mai-schedule")
}

fn cache_path(cache_dir: &Path, group: &str) -> PathBuf {
    let digest = format!("{:x}", md5::compute(group.as_bytes()));
    cache_dir.join("schedules").join(format!("{digest}.json"))
}

fn write_cache(cache_dir: &Path, schedule: &CachedSchedule) -> AppResult<()> {
    let path = cache_path(cache_dir, &schedule.group);
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("Некорректный путь кеша расписаний"))?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temporary, serde_json::to_vec(schedule)?)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn read_cache(cache_dir: &Path, group: &str) -> AppResult<Option<CachedSchedule>> {
    let path = cache_path(cache_dir, group);
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let cached: CachedSchedule = serde_json::from_slice(&bytes)?;
    if cached.group != group {
        return Err(AppError::new("Кеш содержит расписание другой группы"));
    }
    Ok(Some(cached))
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
    sort_lessons(&mut lessons);
    Ok(lessons)
}

#[derive(Default)]
struct IcsEvent {
    start: Option<String>,
    end: Option<String>,
    summary: Option<String>,
    location: Option<String>,
    description: Option<String>,
    cancelled: bool,
}

fn parse_ics_schedule(group: &str, bytes: &[u8]) -> AppResult<Vec<Lesson>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| AppError::new(format!("Некорректный ICS расписания группы {group}")))?;
    if !text.contains(group) {
        return Err(AppError::new(format!(
            "Резервный источник вернул расписание другой группы вместо {group}"
        )));
    }

    let mut lessons = Vec::new();
    let mut event = None;
    for line in unfold_ics_lines(text) {
        match line.as_str() {
            "BEGIN:VEVENT" => event = Some(IcsEvent::default()),
            "END:VEVENT" => {
                if let Some(event) = event.take()
                    && !event.cancelled
                {
                    lessons.push(lesson_from_ics_event(group, event)?);
                }
            }
            _ => {
                let Some(current) = event.as_mut() else {
                    continue;
                };
                let Some((head, raw_value)) = line.split_once(':') else {
                    continue;
                };
                let property = head.split(';').next().unwrap_or(head);
                let value = unescape_ics(raw_value);
                match property {
                    "DTSTART" => current.start = Some(value),
                    "DTEND" => current.end = Some(value),
                    "SUMMARY" => current.summary = Some(value),
                    "LOCATION" => current.location = Some(value),
                    "DESCRIPTION" => current.description = Some(value),
                    "STATUS" if value.eq_ignore_ascii_case("CANCELLED") => {
                        current.cancelled = true;
                    }
                    _ => {}
                }
            }
        }
    }
    if lessons.is_empty() {
        return Err(AppError::new(format!(
            "Резервный источник вернул пустое расписание группы {group}"
        )));
    }
    sort_lessons(&mut lessons);
    Ok(lessons)
}

fn unfold_ics_lines(text: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if let Some(continuation) = line.strip_prefix([' ', '\t'])
            && let Some(previous) = lines.last_mut()
        {
            previous.push_str(continuation);
        } else {
            lines.push(line.to_owned());
        }
    }
    lines
}

fn unescape_ics(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        match characters.next() {
            Some('n' | 'N') => result.push('\n'),
            Some(next @ ('\\' | ',' | ';')) => result.push(next),
            Some(next) => result.push(next),
            None => result.push('\\'),
        }
    }
    result
}

fn lesson_from_ics_event(group: &str, event: IcsEvent) -> AppResult<Lesson> {
    let start = parse_ics_datetime(group, event.start.as_deref())?;
    let end = parse_ics_datetime(group, event.end.as_deref())?;
    let summary = event
        .summary
        .ok_or_else(|| AppError::new(format!("В ICS группы {group} нет названия занятия")))?;
    let (kind, name) = split_ics_summary(&summary);
    let description = event.description.unwrap_or_default();
    let lecturers = description_field(&description, "Преподаватель:")
        .filter(|value| *value != "Преподаватель не указан")
        .map(split_ics_list)
        .unwrap_or_default();
    let rooms = event
        .location
        .map(|value| split_ics_list(&value))
        .unwrap_or_default();
    Ok(Lesson {
        date: start.date(),
        weekday: weekday_name(start.weekday()).to_owned(),
        time_start: start.time().format("%H:%M:%S").to_string(),
        time_end: end.time().format("%H:%M:%S").to_string(),
        name,
        kind,
        lecturers,
        rooms,
        links: Vec::new(),
    })
}

fn parse_ics_datetime(group: &str, value: Option<&str>) -> AppResult<NaiveDateTime> {
    let value = value
        .ok_or_else(|| AppError::new(format!("В ICS группы {group} нет времени занятия")))?
        .trim_end_matches('Z');
    ["%Y%m%dT%H%M%S", "%Y%m%dT%H%M"]
        .into_iter()
        .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
        .ok_or_else(|| AppError::new(format!("Некорректное время в ICS группы {group}")))
}

fn split_ics_summary(summary: &str) -> (String, String) {
    if let Some(rest) = summary.strip_prefix('[')
        && let Some((kind, name)) = rest.split_once(']')
    {
        return (kind.trim().to_owned(), name.trim().to_owned());
    }
    (String::new(), summary.trim().to_owned())
}

fn description_field<'a>(description: &'a str, label: &str) -> Option<&'a str> {
    description
        .lines()
        .find_map(|line| line.strip_prefix(label).map(str::trim))
        .filter(|value| !value.is_empty())
}

fn split_ics_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

fn weekday_name(weekday: Weekday) -> &'static str {
    match weekday {
        Weekday::Mon => "Пн",
        Weekday::Tue => "Вт",
        Weekday::Wed => "Ср",
        Weekday::Thu => "Чт",
        Weekday::Fri => "Пт",
        Weekday::Sat => "Сб",
        Weekday::Sun => "Вс",
    }
}

fn sort_lessons(lessons: &mut [Lesson]) {
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

    const ICS_FIXTURE: &str = "BEGIN:VCALENDAR\r\n\
X-WR-CALNAME:Расписание М3О-101БВ-26\r\n\
BEGIN:VEVENT\r\n\
DTSTART;TZID=Europe/Moscow:20260915T104500\r\n\
DTEND;TZID=Europe/Moscow:20260915T121500\r\n\
SUMMARY:[ЛР] Физика\r\n\
LOCATION:ГУК Б-201\\, ГУК Б-202\r\n\
DESCRIPTION:Предмет: Физика\\nТип занятия: ЛР\\nПреподаватель: Иванов И.И.\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

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

    #[test]
    fn parses_the_fallback_ics_shape() {
        let lessons = parse_ics_schedule("М3О-101БВ-26", ICS_FIXTURE.as_bytes()).unwrap();

        assert_eq!(lessons.len(), 1);
        assert_eq!(
            lessons[0].date,
            NaiveDate::from_ymd_opt(2026, 9, 15).unwrap()
        );
        assert_eq!(lessons[0].weekday, "Вт");
        assert_eq!(lessons[0].time_start, "10:45:00");
        assert_eq!(lessons[0].kind, "ЛР");
        assert_eq!(lessons[0].name, "Физика");
        assert_eq!(lessons[0].lecturers, ["Иванов И.И."]);
        assert_eq!(lessons[0].rooms, ["ГУК Б-201", "ГУК Б-202"]);
    }

    #[test]
    fn builds_the_fallback_url_without_an_empty_path_segment() {
        let url = fallback_schedule_url("М3О-101БВ-26").unwrap();
        assert_eq!(
            url.as_str(),
            "https://schedule.maisch.ru/calendar/%D0%9C3%D0%9E-101%D0%91%D0%92-26.ics"
        );
    }

    #[test]
    fn writes_and_reads_a_group_cache() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "mai-schedule-cache-test-{}-{unique}",
            std::process::id(),
        ));
        let lessons = parse_ics_schedule("М3О-101БВ-26", ICS_FIXTURE.as_bytes()).unwrap();
        let cached = CachedSchedule {
            group: "М3О-101БВ-26".into(),
            updated_at: "29.10.2026 18:30".into(),
            lessons,
        };

        write_cache(&directory, &cached).unwrap();
        let restored = read_cache(&directory, &cached.group).unwrap().unwrap();

        assert_eq!(restored.updated_at, cached.updated_at);
        assert_eq!(restored.lessons, cached.lessons);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reports_when_cached_data_may_be_stale() {
        let schedule = FetchedSchedule {
            lessons: Vec::new(),
            cached_at: Some("29.10.2026 18:30".into()),
        };
        let warning = schedule.cache_warning().unwrap();
        assert!(warning.contains("возможны неточности"));
        assert!(warning.contains("29.10.2026 18:30"));
    }
}
