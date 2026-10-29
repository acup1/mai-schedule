use std::io::IsTerminal;
use std::path::PathBuf;

use serde_json::json;

use crate::client::MaiScheduleClient;
use crate::generator::generate_all;
use crate::models::{Lesson, is_exam, is_lab};
use crate::render::{render_json, render_text, week_title};
use crate::selection::{expand_week_selectors, select_weeks};
use crate::{AppError, AppResult};

const HELP: &str = "\
Расписание учебных групп МАИ\n\n\
Использование:\n  mai-schedule [--timeout SECONDS] [COMMAND]\n\n\
Команды:\n  raw GROUP [WEEKS...] [--labs|--exams] [--json]\n  tui [GROUP]\n  groups [--institute NAME] [--course 1..6] [--json]\n  generate [--output DIR] [--workers 1..32]\n\n\
Без команды открывается TUI.\n\
Селекторы недель: current, +1, YYYY-Www, YYYY-MM-DD, A..B.\n\
Таймаут сетевых запросов по умолчанию: 2 секунды.";

const DEFAULT_TIMEOUT_SECONDS: f64 = 2.0;

pub fn run(mut args: Vec<String>) -> AppResult<()> {
    let timeout = take_value_option(&mut args, "--timeout")?
        .map(|value| {
            value
                .parse::<f64>()
                .map_err(|_| AppError::new("Некорректное значение --timeout"))
        })
        .transpose()?
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS);
    if args
        .iter()
        .any(|argument| matches!(argument.as_str(), "--help" | "-h"))
    {
        println!("{HELP}");
        return Ok(());
    }
    let client = MaiScheduleClient::new(timeout)?;
    if args.is_empty() {
        return tui(&client, None);
    }
    let command = args.remove(0);
    match command.as_str() {
        "raw" => raw(&client, args),
        "tui" => {
            if args.len() > 1 {
                return Err(AppError::new("tui принимает не более одной группы"));
            }
            tui(&client, args.first().map(String::as_str))
        }
        "groups" => groups(&client, args),
        "generate" => generate(&client, args),
        _ => Err(AppError::new(format!(
            "Неизвестная команда: {command}\n\n{HELP}"
        ))),
    }
}

fn raw(client: &MaiScheduleClient, mut args: Vec<String>) -> AppResult<()> {
    let json_output = take_flag(&mut args, "--json");
    let labs = take_flag(&mut args, "--labs");
    let exams = take_flag(&mut args, "--exams");
    if labs && exams {
        return Err(AppError::new("--labs и --exams взаимоисключающие"));
    }
    let group = args
        .first()
        .cloned()
        .ok_or_else(|| AppError::new("raw требует имя группы"))?;
    let weeks = args.into_iter().skip(1).collect::<Vec<_>>();
    reject_options(&weeks)?;
    let schedule = client.fetch(&group)?;
    if let Some(warning) = schedule.cache_warning() {
        eprintln!("Предупреждение: {warning}");
    }
    let lessons = schedule.lessons;
    let (selected, kind, week, title): (Vec<Lesson>, &str, Option<String>, String) = if labs {
        (
            lessons.into_iter().filter(is_lab).collect(),
            "labs",
            None,
            "Все лабораторные работы".into(),
        )
    } else if exams {
        (
            lessons.into_iter().filter(is_exam).collect(),
            "exams",
            None,
            "Все экзамены".into(),
        )
    } else {
        let mondays = expand_week_selectors(&weeks, None)?;
        let selected = select_weeks(&lessons, &mondays);
        let names: Vec<String> = mondays
            .iter()
            .map(|monday| crate::selection::week_key(*monday))
            .collect();
        let kind = if names.len() == 1 { "week" } else { "weeks" };
        let week = (names.len() == 1).then(|| names[0].clone());
        let title = mondays
            .iter()
            .map(|monday| week_title(*monday))
            .collect::<Vec<_>>()
            .join("; ");
        (selected, kind, week, title)
    };
    if json_output {
        println!("{}", render_json(&group, &selected, kind, week.as_deref())?);
    } else {
        println!("{}", render_text(&group, &selected, Some(&title)));
    }
    Ok(())
}

fn groups(client: &MaiScheduleClient, mut args: Vec<String>) -> AppResult<()> {
    let json_output = take_flag(&mut args, "--json");
    let institute = take_value_option(&mut args, "--institute")?;
    let course = take_value_option(&mut args, "--course")?.unwrap_or_else(|| "all".into());
    reject_options(&args)?;
    if !args.is_empty() {
        return Err(AppError::new("Лишние аргументы для groups"));
    }
    if let Some(institute) = institute {
        let values = client.list_groups(&institute, &course)?;
        if json_output {
            println!("{}", serde_json::to_string_pretty(&values)?);
        } else {
            println!("{}", values.join("\n"));
        }
    } else {
        let (institutes, courses) = client.catalog()?;
        if json_output {
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &json!({"institutes": institutes, "courses": courses})
                )?
            );
        } else {
            println!(
                "Институты:\n{}",
                institutes
                    .iter()
                    .map(|item| format!("  {item}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            println!("Курсы: {}", courses.join(", "));
        }
    }
    Ok(())
}

fn generate(client: &MaiScheduleClient, mut args: Vec<String>) -> AppResult<()> {
    let output = take_value_option(&mut args, "--output")?
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("dist"));
    let workers = take_value_option(&mut args, "--workers")?
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| AppError::new("Некорректное значение --workers"))
        })
        .transpose()?
        .unwrap_or(8);
    reject_options(&args)?;
    if !args.is_empty() {
        return Err(AppError::new("Лишние аргументы для generate"));
    }
    let count = generate_all(&output, workers, client)?;
    println!("Сгенерировано групп: {count}");
    Ok(())
}

fn tui(client: &MaiScheduleClient, group: Option<&str>) -> AppResult<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(AppError::new(
            "TUI требует терминал; для скриптов используйте raw",
        ));
    }
    match crate::tui::run_tui(client, group) {
        Err(error) if error.0 == "Выход отменён" => Ok(()),
        result => result,
    }
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|argument| argument == name) {
        args.remove(index);
        true
    } else {
        false
    }
}

fn take_value_option(args: &mut Vec<String>, name: &str) -> AppResult<Option<String>> {
    if let Some(index) = args
        .iter()
        .position(|argument| argument == name || argument.starts_with(&format!("{name}=")))
    {
        let argument = args.remove(index);
        if let Some((_, value)) = argument.split_once('=') {
            if value.is_empty() {
                return Err(AppError::new(format!("Для {name} нужно значение")));
            }
            return Ok(Some(value.to_owned()));
        }
        if index >= args.len() {
            return Err(AppError::new(format!("Для {name} нужно значение")));
        }
        return Ok(Some(args.remove(index)));
    }
    Ok(None)
}

fn reject_options(args: &[String]) -> AppResult<()> {
    if let Some(option) = args.iter().find(|argument| argument.starts_with("--")) {
        return Err(AppError::new(format!("Неизвестная опция: {option}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_network_timeout_is_two_seconds() {
        assert_eq!(DEFAULT_TIMEOUT_SECONDS, 2.0);
    }

    #[test]
    fn extracts_options_in_both_forms() {
        let mut args = vec!["raw".into(), "--timeout=2.5".into()];
        assert_eq!(
            take_value_option(&mut args, "--timeout").unwrap(),
            Some("2.5".into())
        );
        assert_eq!(args, vec!["raw"]);

        let mut args = vec!["--workers".into(), "4".into()];
        assert_eq!(
            take_value_option(&mut args, "--workers").unwrap(),
            Some("4".into())
        );
        assert!(args.is_empty());
    }
}
