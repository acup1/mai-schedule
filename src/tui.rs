use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, TryRecvError},
};
use std::thread;
use std::time::Duration as StdDuration;

use chrono::Duration as ChronoDuration;
use terminalib::{
    CellStyle, KEY_DOWN, KEY_FAIL, KEY_LEFT, KEY_RIGHT, KEY_UP, Rgb, WindowManager,
    terminal::Terminal,
};

use crate::client::MaiScheduleClient;
use crate::models::{Lesson, is_exam, is_lab};
use crate::render::{render_text, week_title};
use crate::selection::{current_schedule_monday, select_weeks};
use crate::{AppError, AppResult};

const ACCENT: Rgb = Rgb::new(93, 173, 226);
const TEXT: Rgb = Rgb::new(226, 232, 240);
const MUTED: Rgb = Rgb::new(126, 140, 156);
const SELECTED_BG: Rgb = Rgb::new(35, 55, 72);
const MATCH: Rgb = Rgb::new(250, 204, 21);
const RETRY_DELAY: StdDuration = StdDuration::from_secs(1);
const ESCAPE_CONTINUATION_TIMEOUT_MS: u64 = 25;
const ESCAPE_CONTINUATION_READS: usize = 2;
const EXIT_MESSAGE: &str = "Выход отменён";

#[derive(Debug, Eq, PartialEq)]
enum Input {
    Key(i32),
    Text(String),
    PageUp,
    PageDown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Week,
    Labs,
    Exams,
}

enum ScreenResult<T> {
    Value(T),
    Back,
}

pub fn run_tui(client: &MaiScheduleClient, group: Option<&str>) -> AppResult<()> {
    let mut terminal = Terminal::wannajustwork()?;
    let mut windows = WindowManager::new();
    if let Some(group) = group {
        let group = group.to_owned();
        let request_group = group.clone();
        let request_client = client.clone();
        let schedule = match load_with_retry(
            &mut terminal,
            &mut windows,
            "Расписание",
            &format!("Загружаю {group}…"),
            false,
            move || request_client.fetch(&request_group),
        )? {
            ScreenResult::Value(schedule) => schedule,
            ScreenResult::Back => unreachable!("back is disabled for an explicit group"),
        };
        let warning = schedule.cache_warning();
        let _ = browse_schedule(
            &mut terminal,
            &mut windows,
            &group,
            &schedule.lessons,
            warning.as_deref(),
            false,
        )?;
        return Ok(());
    }
    pick_group_and_browse(&mut terminal, &mut windows, client)
}

fn pick_group_and_browse(
    terminal: &mut Terminal,
    windows: &mut WindowManager,
    client: &MaiScheduleClient,
) -> AppResult<()> {
    let catalog_client = client.clone();
    let (institutes, courses) = match load_with_retry(
        terminal,
        windows,
        "Каталог",
        "Загружаю институты…",
        false,
        move || catalog_client.catalog(),
    )? {
        ScreenResult::Value(catalog) => catalog,
        ScreenResult::Back => unreachable!("the catalog is the root screen"),
    };

    'institutes: loop {
        let institute = match choose(
            terminal,
            windows,
            "Институт",
            "Выберите подразделение",
            &institutes,
            false,
        )? {
            ScreenResult::Value(institute) => institute,
            ScreenResult::Back => continue,
        };

        'courses: loop {
            let course = match choose(terminal, windows, "Курс", &institute, &courses, true)? {
                ScreenResult::Value(course) => course,
                ScreenResult::Back => continue 'institutes,
            };
            let groups_client = client.clone();
            let groups_institute = institute.clone();
            let groups_course = course.clone();
            let groups = match load_with_retry(
                terminal,
                windows,
                "Группы",
                "Загружаю список…",
                true,
                move || groups_client.list_groups(&groups_institute, &groups_course),
            )? {
                ScreenResult::Value(groups) => groups,
                ScreenResult::Back => continue 'courses,
            };

            'groups: loop {
                let group = match choose(
                    terminal,
                    windows,
                    "Группа",
                    &format!("{institute} · {course} курс"),
                    &groups,
                    true,
                )? {
                    ScreenResult::Value(group) => group,
                    ScreenResult::Back => continue 'courses,
                };
                let schedule_client = client.clone();
                let schedule_group = group.clone();
                let schedule = match load_with_retry(
                    terminal,
                    windows,
                    "Расписание",
                    &format!("Загружаю {group}…"),
                    true,
                    move || schedule_client.fetch(&schedule_group),
                )? {
                    ScreenResult::Value(schedule) => schedule,
                    ScreenResult::Back => continue 'groups,
                };
                let warning = schedule.cache_warning();
                match browse_schedule(
                    terminal,
                    windows,
                    &group,
                    &schedule.lessons,
                    warning.as_deref(),
                    true,
                )? {
                    ScreenResult::Back => continue 'groups,
                    ScreenResult::Value(()) => unreachable!("schedule browsing has no completion"),
                }
            }
        }
    }
}

fn choose(
    terminal: &mut Terminal,
    windows: &mut WindowManager,
    title: &str,
    subtitle: &str,
    items: &[String],
    can_go_back: bool,
) -> AppResult<ScreenResult<String>> {
    if items.is_empty() {
        return Err(AppError::new("Список для выбора пуст"));
    }
    let mut selected = 0usize;
    let mut scroll = 0usize;
    let mut search = String::new();
    let mut searching = false;
    let mut status = String::new();
    loop {
        let (height, _) = terminal.getsize();
        let body_height = usize::from(height.saturating_sub(6)).max(1);
        keep_visible(&mut scroll, selected, body_height, items.len());
        let footer = if searching {
            format!("/{search}█")
        } else if status.is_empty() {
            navigation_footer("↑↓ переместить   enter выбрать   / поиск", can_go_back)
        } else {
            status.clone()
        };
        draw(
            terminal,
            windows,
            title,
            subtitle,
            items,
            scroll,
            Some(selected),
            &search,
            &footer,
        )?;
        let input = read_input(terminal)?;
        if is_quit(&input) {
            return Err(AppError::new(EXIT_MESSAGE));
        }
        match input {
            Input::Key(27) if searching => {
                searching = false;
                status.clear();
            }
            Input::Key(27) if can_go_back => return Ok(ScreenResult::Back),
            Input::Key(27) => {}
            Input::Key(8 | 127) if searching => {
                search.pop();
                select_first_match(items, &search, &mut selected, &mut status);
            }
            Input::Key(10 | 13) if searching => searching = false,
            Input::Text(text) if searching => {
                search.push_str(&text);
                select_first_match(items, &search, &mut selected, &mut status);
            }
            Input::Key(KEY_UP) => selected = selected.saturating_sub(1),
            Input::Key(KEY_DOWN) => selected = (selected + 1).min(items.len() - 1),
            Input::PageUp => selected = selected.saturating_sub(body_height),
            Input::PageDown => selected = (selected + body_height).min(items.len() - 1),
            Input::Key(10 | 13) => return Ok(ScreenResult::Value(items[selected].clone())),
            Input::Text(text) => match command_char(&text) {
                Some('/') => {
                    searching = true;
                    status.clear();
                }
                Some('j') => selected = (selected + 1).min(items.len() - 1),
                Some('k') => selected = selected.saturating_sub(1),
                Some('n') if !search.is_empty() => {
                    if let Some(index) = find_match(items, &search, selected, 1) {
                        selected = index;
                    }
                }
                Some('N') if !search.is_empty() => {
                    if let Some(index) = find_match(items, &search, selected, -1) {
                        selected = index;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn browse_schedule(
    terminal: &mut Terminal,
    windows: &mut WindowManager,
    group: &str,
    lessons: &[Lesson],
    warning: Option<&str>,
    can_go_back: bool,
) -> AppResult<ScreenResult<()>> {
    let screen_title = warning
        .map(|message| format!("⚠ {message}"))
        .unwrap_or_else(|| "MAI  /  SCHEDULE".into());
    let mut monday = current_schedule_monday(None);
    let mut mode = Mode::Week;
    let mut scroll = 0usize;
    let mut search = String::new();
    let mut searching = false;
    let mut match_line = None;
    let mut status = String::new();
    loop {
        let (visible, title): (Vec<Lesson>, String) = match mode {
            Mode::Week => (select_weeks(lessons, &[monday]), week_title(monday)),
            Mode::Labs => (
                lessons
                    .iter()
                    .filter(|lesson| is_lab(lesson))
                    .cloned()
                    .collect(),
                "Все лабораторные работы".into(),
            ),
            Mode::Exams => (
                lessons
                    .iter()
                    .filter(|lesson| is_exam(lesson))
                    .cloned()
                    .collect(),
                "Все экзамены".into(),
            ),
        };
        let content: Vec<String> = render_text(group, &visible, Some(&title))
            .lines()
            .map(str::to_owned)
            .collect();
        let (height, _) = terminal.getsize();
        let body_height = usize::from(height.saturating_sub(6)).max(1);
        scroll = scroll.min(content.len().saturating_sub(body_height));
        let footer = if searching {
            format!("/{search}█")
        } else if status.is_empty() {
            navigation_footer(
                "←→ неделя   ↑↓ прокрутка   l лабы   e экзамены   / поиск",
                can_go_back,
            )
        } else {
            status.clone()
        };
        draw(
            terminal,
            windows,
            &screen_title,
            &title,
            &content,
            scroll,
            None,
            &search,
            &footer,
        )?;
        let input = read_input(terminal)?;
        if is_quit(&input) {
            return Err(AppError::new(EXIT_MESSAGE));
        }
        match input {
            Input::Key(27) if searching => {
                searching = false;
                search.clear();
                match_line = None;
                status.clear();
            }
            Input::Key(27) if !search.is_empty() => {
                search.clear();
                match_line = None;
                status.clear();
            }
            Input::Key(27) if can_go_back => return Ok(ScreenResult::Back),
            Input::Key(27) => {}
            Input::Key(8 | 127) if searching => {
                search.pop();
                update_content_match(
                    &content,
                    &search,
                    &mut scroll,
                    &mut match_line,
                    &mut status,
                    body_height,
                );
            }
            Input::Key(10 | 13) if searching => searching = false,
            Input::Text(text) if searching => {
                search.push_str(&text);
                update_content_match(
                    &content,
                    &search,
                    &mut scroll,
                    &mut match_line,
                    &mut status,
                    body_height,
                );
            }
            Input::Key(KEY_LEFT) => {
                monday -= ChronoDuration::weeks(1);
                reset_view(&mut mode, &mut scroll, &mut match_line, &mut status);
            }
            Input::Key(KEY_RIGHT) => {
                monday += ChronoDuration::weeks(1);
                reset_view(&mut mode, &mut scroll, &mut match_line, &mut status);
            }
            Input::Key(KEY_UP) => scroll = scroll.saturating_sub(1),
            Input::Key(KEY_DOWN) => {
                scroll = (scroll + 1).min(content.len().saturating_sub(body_height));
            }
            Input::PageUp => scroll = scroll.saturating_sub(body_height),
            Input::PageDown => {
                scroll = (scroll + body_height).min(content.len().saturating_sub(body_height));
            }
            Input::Text(text) => match command_char(&text) {
                Some('/') => {
                    searching = true;
                    status.clear();
                }
                Some('j') => {
                    scroll = (scroll + 1).min(content.len().saturating_sub(body_height));
                }
                Some('k') => scroll = scroll.saturating_sub(1),
                Some('w') => {
                    reset_view(&mut mode, &mut scroll, &mut match_line, &mut status);
                }
                Some('l') => {
                    mode = Mode::Labs;
                    scroll = 0;
                }
                Some('e') => {
                    mode = Mode::Exams;
                    scroll = 0;
                }
                Some('n') | Some('N') if !search.is_empty() => {
                    let direction = if command_char(&text) == Some('N') {
                        -1
                    } else {
                        1
                    };
                    let start = match_line.unwrap_or(scroll);
                    if let Some(index) = find_match(&content, &search, start, direction) {
                        match_line = Some(index);
                        scroll = center_scroll(index, body_height, content.len());
                        status = format!("/{search}");
                    } else {
                        status = format!("Не найдено: {search}");
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
}

fn load_with_retry<T, F>(
    terminal: &mut Terminal,
    windows: &mut WindowManager,
    title: &str,
    message: &str,
    can_go_back: bool,
    operation: F,
) -> AppResult<ScreenResult<T>>
where
    T: Send + 'static,
    F: FnMut() -> AppResult<T> + Send + 'static,
{
    let (receiver, cancelled) = spawn_retrying_load(operation, RETRY_DELAY);
    let mut lines = vec![message.to_owned()];
    loop {
        match receiver.try_recv() {
            Ok(Ok(value)) => {
                cancelled.store(true, Ordering::Relaxed);
                return Ok(ScreenResult::Value(value));
            }
            Ok(Err(error)) => {
                lines = vec![
                    "Не удалось загрузить.".into(),
                    error.to_string(),
                    "Повторяю автоматически…".into(),
                ];
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                return Err(AppError::new("Фоновая загрузка неожиданно остановилась"));
            }
        }
        draw(
            terminal,
            windows,
            "MAI  /  SCHEDULE",
            title,
            &lines,
            0,
            None,
            "",
            &navigation_footer("Загрузка", can_go_back),
        )?;
        let input = read_input(terminal)?;
        if is_quit(&input) {
            cancelled.store(true, Ordering::Relaxed);
            return Err(AppError::new(EXIT_MESSAGE));
        }
        if matches!(input, Input::Key(27)) && can_go_back {
            cancelled.store(true, Ordering::Relaxed);
            return Ok(ScreenResult::Back);
        }
    }
}

fn spawn_retrying_load<T, F>(
    mut operation: F,
    retry_delay: StdDuration,
) -> (Receiver<AppResult<T>>, Arc<AtomicBool>)
where
    T: Send + 'static,
    F: FnMut() -> AppResult<T> + Send + 'static,
{
    let (sender, receiver) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    thread::spawn(move || {
        while !worker_cancelled.load(Ordering::Relaxed) {
            let result = operation();
            let finished = result.is_ok();
            if sender.send(result).is_err() || finished {
                break;
            }
            thread::sleep(retry_delay);
        }
    });
    (receiver, cancelled)
}

fn navigation_footer(prefix: &str, can_go_back: bool) -> String {
    if can_go_back {
        format!("{prefix}   esc назад   q выйти")
    } else {
        format!("{prefix}   q выйти")
    }
}

fn is_quit(input: &Input) -> bool {
    matches!(input, Input::Text(text) if command_char(text) == Some('q'))
}

fn reset_view(
    mode: &mut Mode,
    scroll: &mut usize,
    match_line: &mut Option<usize>,
    status: &mut String,
) {
    *mode = Mode::Week;
    *scroll = 0;
    *match_line = None;
    status.clear();
}

fn update_content_match(
    content: &[String],
    search: &str,
    scroll: &mut usize,
    match_line: &mut Option<usize>,
    status: &mut String,
    body_height: usize,
) {
    if search.is_empty() {
        *match_line = None;
        status.clear();
    } else if let Some(index) = find_match(content, search, content.len().saturating_sub(1), 1) {
        *match_line = Some(index);
        *scroll = center_scroll(index, body_height, content.len());
        *status = format!("/{search}");
    } else {
        *status = format!("Не найдено: {search}");
    }
}

#[allow(clippy::too_many_arguments)]
fn draw(
    terminal: &mut Terminal,
    windows: &mut WindowManager,
    title: &str,
    subtitle: &str,
    lines: &[String],
    scroll: usize,
    selected: Option<usize>,
    query: &str,
    footer: &str,
) -> io::Result<()> {
    windows.del_all_windows();
    let (height, width) = terminal.getsize();
    let id = windows.new_window(0);
    let window = windows.get_mut(id).expect("new window exists");
    window.toggle_fullscreen(height, width);
    window.border = -1;
    window.filling = true;
    if width < 36 || height < 10 {
        window.posprint_styled(
            0,
            1,
            "Увеличьте окно терминала · q выйти",
            CellStyle::foreground(MUTED),
        );
        windows.render_windows(terminal)?;
        return Ok(());
    }
    let available = usize::from(width.saturating_sub(4));
    let title_color = if title.starts_with('⚠') {
        MATCH
    } else {
        ACCENT
    };
    window.posprint_styled(
        0,
        2,
        truncate(title, available),
        CellStyle::foreground(title_color),
    );
    window.posprint_styled(
        1,
        2,
        truncate(subtitle, available),
        CellStyle::foreground(TEXT),
    );
    window.posprint_styled(
        2,
        2,
        "─".repeat(available),
        CellStyle::foreground(SELECTED_BG),
    );

    let body_height = usize::from(height.saturating_sub(6)).max(1);
    for (row, (index, line)) in lines
        .iter()
        .enumerate()
        .skip(scroll)
        .take(body_height)
        .enumerate()
    {
        let text = truncate(line, available);
        let style = if selected == Some(index) {
            CellStyle::new(Some(TEXT), Some(SELECTED_BG))
        } else if !query.is_empty() && line.to_lowercase().contains(&query.to_lowercase()) {
            CellStyle::foreground(MATCH)
        } else if line.is_empty()
            || line.starts_with("Неделя ")
            || (!line.starts_with(' ') && line.contains('('))
        {
            CellStyle::foreground(MUTED)
        } else {
            CellStyle::foreground(TEXT)
        };
        let printed = if selected == Some(index) {
            format!("{text:<available$}")
        } else {
            text
        };
        window.posprint_styled(row as i32 + 3, 2, printed, style);
    }
    let separator_row = i32::from(height) - 2;
    window.posprint_styled(
        separator_row,
        2,
        "─".repeat(available),
        CellStyle::foreground(SELECTED_BG),
    );
    window.posprint_styled(
        separator_row + 1,
        2,
        truncate(footer, available),
        CellStyle::foreground(MUTED),
    );
    windows.render_windows(terminal)?;
    Ok(())
}

fn read_input(terminal: &mut Terminal) -> io::Result<Input> {
    let key = terminal.get_key(100, false)?;
    let mut bytes = terminal_input_bytes(terminal);
    if bytes == b"\x1b" {
        for _ in 0..ESCAPE_CONTINUATION_READS {
            terminal.get_key(ESCAPE_CONTINUATION_TIMEOUT_MS, false)?;
            let continuation = terminal_input_bytes(terminal);
            if continuation.is_empty() {
                break;
            }
            bytes.extend_from_slice(&continuation);
            if decode_escape_sequence(&bytes).is_some() {
                break;
            }
        }
    }
    Ok(decode_input(&bytes, key))
}

fn terminal_input_bytes(terminal: &Terminal) -> Vec<u8> {
    let count = *terminal.readed.lock().expect("input length lock poisoned");
    let buffer = *terminal
        .reading_buffer
        .lock()
        .expect("input buffer lock poisoned");
    buffer[..count].to_vec()
}

fn decode_input(bytes: &[u8], key: i32) -> Input {
    if let Some(input) = decode_escape_sequence(bytes) {
        return input;
    }
    if bytes == b"\x1b" {
        return Input::Key(27);
    }
    if matches!(
        key,
        KEY_LEFT | KEY_RIGHT | KEY_UP | KEY_DOWN | KEY_FAIL | 8 | 10 | 13 | 27 | 127
    ) {
        return Input::Key(key);
    }
    if let Ok(text) = std::str::from_utf8(bytes)
        && !text.is_empty()
    {
        return Input::Text(text.to_owned());
    }
    Input::Key(key)
}

fn decode_escape_sequence(bytes: &[u8]) -> Option<Input> {
    match bytes {
        b"\x1b[A" => Some(Input::Key(KEY_UP)),
        b"\x1b[B" => Some(Input::Key(KEY_DOWN)),
        b"\x1b[C" => Some(Input::Key(KEY_RIGHT)),
        b"\x1b[D" => Some(Input::Key(KEY_LEFT)),
        b"\x1b[5~" | b"\x1b[1;2A" => Some(Input::PageUp),
        b"\x1b[6~" | b"\x1b[1;2B" => Some(Input::PageDown),
        _ => None,
    }
}

fn command_char(text: &str) -> Option<char> {
    let character = text.chars().next()?;
    const RUSSIAN: &str = "ёйцукенгшщзхъфывапролджэячсмитьбю.ЁЙЦУКЕНГШЩЗХЪФЫВАПРОЛДЖЭЯЧСМИТЬБЮ,";
    const ASCII: &str = "\x60qwertyuiop[]asdfghjkl;'zxcvbnm,./~QWERTYUIOP{}ASDFGHJKL:\"ZXCVBNM<>";
    if let Some(index) = RUSSIAN.chars().position(|candidate| candidate == character) {
        ASCII.chars().nth(index)
    } else {
        Some(character)
    }
}

fn select_first_match(items: &[String], query: &str, selected: &mut usize, status: &mut String) {
    if query.is_empty() {
        status.clear();
    } else if let Some(index) = items
        .iter()
        .position(|item| item.to_lowercase().contains(&query.to_lowercase()))
    {
        *selected = index;
        *status = format!("/{query}");
    } else {
        *status = format!("Не найдено: {query}");
    }
}

fn find_match(lines: &[String], query: &str, start: usize, direction: isize) -> Option<usize> {
    if lines.is_empty() || query.is_empty() {
        return None;
    }
    let needle = query.to_lowercase();
    for offset in 1..=lines.len() {
        let index = (start as isize + direction * offset as isize).rem_euclid(lines.len() as isize)
            as usize;
        if lines[index].to_lowercase().contains(&needle) {
            return Some(index);
        }
    }
    None
}

fn keep_visible(scroll: &mut usize, selected: usize, height: usize, length: usize) {
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + height {
        *scroll = selected + 1 - height;
    }
    *scroll = (*scroll).min(length.saturating_sub(height));
}

fn center_scroll(index: usize, height: usize, length: usize) -> usize {
    index
        .saturating_sub(height / 2)
        .min(length.saturating_sub(height))
}

fn truncate(value: &str, width: usize) -> String {
    let count = value.chars().count();
    if count <= width {
        return value.to_owned();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let mut result: String = value.chars().take(width - 1).collect();
    result.push('…');
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn russian_layout_maps_to_commands() {
        assert_eq!(command_char("й"), Some('q'));
        assert_eq!(command_char("д"), Some('l'));
        assert_eq!(command_char("у"), Some('e'));
    }

    #[test]
    fn match_search_wraps_in_both_directions() {
        let lines = vec!["alpha".into(), "beta".into(), "alpha two".into()];
        assert_eq!(find_match(&lines, "alpha", 2, 1), Some(0));
        assert_eq!(find_match(&lines, "alpha", 0, -1), Some(2));
    }

    #[test]
    fn retries_failed_loads_without_confirmation() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let worker_attempts = Arc::clone(&attempts);
        let (receiver, _) = spawn_retrying_load(
            move || {
                let attempt = worker_attempts.fetch_add(1, Ordering::Relaxed);
                if attempt < 2 {
                    Err(AppError::new("temporary failure"))
                } else {
                    Ok(42)
                }
            },
            StdDuration::ZERO,
        );

        assert!(receiver.recv().unwrap().is_err());
        assert!(receiver.recv().unwrap().is_err());
        assert_eq!(receiver.recv().unwrap().unwrap(), 42);
        assert_eq!(attempts.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn quit_is_global_for_both_keyboard_layouts() {
        assert!(is_quit(&Input::Text("q".into())));
        assert!(is_quit(&Input::Text("й".into())));
    }

    #[test]
    fn arrow_sequences_are_not_mistaken_for_escape() {
        assert_eq!(decode_input(b"\x1b[D", 27), Input::Key(KEY_LEFT));
        assert_eq!(decode_input(b"\x1b[C", 27), Input::Key(KEY_RIGHT));
        assert_eq!(decode_input(b"\x1b", 27), Input::Key(27));
    }
}
