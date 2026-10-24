use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use crate::client::MaiScheduleClient;
use crate::models::{Lesson, is_exam, is_lab};
use crate::render::{render_text, week_title};
use crate::selection::{current_schedule_monday, parse_week, select_weeks};
use crate::{AppError, AppResult};

fn write_text(path: &Path, value: &str) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = PathBuf::from(format!("{}.tmp", path.display()));
    fs::write(&temporary, format!("{}\n", value.trim_end_matches('\n')))?;
    fs::rename(temporary, path)?;
    Ok(())
}

pub fn write_group(output: &Path, group: &str, lessons: &[Lesson]) -> AppResult<()> {
    if group.is_empty() || matches!(group, "." | "..") || group.contains(['/', '\\', '\0']) {
        return Err(AppError::new(format!("Небезопасное имя группы: {group:?}")));
    }
    let target = output.join(group);
    fs::create_dir_all(&target)?;
    remove_matching_files(&target, "week-", ".txt")?;
    for obsolete in ["current.json", "labs.json", "exams.json", "manifest.json"] {
        remove_if_exists(&target.join(obsolete))?;
    }
    remove_matching_files(&target, "week-", ".json")?;

    let mut weeks: Vec<String> = lessons.iter().map(Lesson::week).collect();
    weeks.sort();
    weeks.dedup();
    for week in weeks {
        let selected: Vec<Lesson> = lessons
            .iter()
            .filter(|lesson| lesson.week() == week)
            .cloned()
            .collect();
        let monday = parse_week(&week, None)?;
        write_text(
            &target.join(format!("week-{week}.txt")),
            &render_text(group, &selected, Some(&week_title(monday))),
        )?;
    }
    let current = current_schedule_monday(None);
    write_text(
        &target.join("current.txt"),
        &render_text(
            group,
            &select_weeks(lessons, &[current]),
            Some(&week_title(current)),
        ),
    )?;
    let labs: Vec<Lesson> = lessons
        .iter()
        .filter(|lesson| is_lab(lesson))
        .cloned()
        .collect();
    write_text(
        &target.join("labs.txt"),
        &render_text(group, &labs, Some("Все лабораторные работы")),
    )?;
    let exams: Vec<Lesson> = lessons
        .iter()
        .filter(|lesson| is_exam(lesson))
        .cloned()
        .collect();
    write_text(
        &target.join("exams.txt"),
        &render_text(group, &exams, Some("Все экзамены")),
    )?;
    Ok(())
}

pub fn generate_all(output: &Path, workers: usize, client: &MaiScheduleClient) -> AppResult<usize> {
    if !(1..=32).contains(&workers) {
        return Err(AppError::new("--workers должен быть от 1 до 32"));
    }
    let groups = client.list_all_groups()?;
    fs::create_dir_all(output)?;
    for obsolete in ["groups.json", "errors.json", "errors.txt"] {
        remove_if_exists(&output.join(obsolete))?;
    }
    write_text(&output.join("groups.txt"), &groups.join("\n"))?;

    let queue = Arc::new(Mutex::new(VecDeque::from(groups.clone())));
    let (sender, receiver) = mpsc::channel();
    let worker_count = workers.min(groups.len().max(1));
    thread::scope(|scope| {
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let sender = sender.clone();
            let client = client.clone();
            scope.spawn(move || {
                loop {
                    let group = queue.lock().expect("group queue poisoned").pop_front();
                    let Some(group) = group else { break };
                    let result = client.fetch(&group);
                    if sender.send((group, result)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        let mut errors = Vec::new();
        for (index, (group, result)) in receiver.into_iter().enumerate() {
            match result.and_then(|lessons| write_group(output, &group, &lessons)) {
                Ok(()) => println!("[{}/{}] {group}", index + 1, groups.len()),
                Err(error) => {
                    eprintln!("ERROR {group}: {error}");
                    errors.push((group, error.to_string()));
                }
            }
        }
        if errors.is_empty() {
            Ok(groups.len())
        } else {
            write_text(
                &output.join("errors.txt"),
                &errors
                    .iter()
                    .map(|(group, error)| format!("{group}: {error}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )?;
            Err(AppError::new(format!(
                "Не удалось сгенерировать расписание для {} из {} групп",
                errors.len(),
                groups.len()
            )))
        }
    })
}

fn remove_if_exists(path: &Path) -> AppResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn remove_matching_files(directory: &Path, prefix: &str, suffix: &str) -> AppResult<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(prefix) && name.ends_with(suffix) && entry.file_type()?.is_file() {
            fs::remove_file(entry.path())?;
        }
    }
    Ok(())
}
