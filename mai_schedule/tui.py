from __future__ import annotations

import curses
from datetime import timedelta
from typing import Sequence

from .client import MaiScheduleClient
from .models import Lesson, is_exam, is_lab
from .render import render_text, week_title
from .selection import current_schedule_monday, select_weeks


def _get_input_key(stdscr: curses.window) -> str | int:
    get_wch = getattr(stdscr, "get_wch", None)
    if get_wch is not None:
        return get_wch()
    code = stdscr.getch()
    if code > 255:
        return code
    if code < 128:
        return chr(code)
    if code & 0b11110000 == 0b11110000:
        length = 4
    elif code & 0b11100000 == 0b11100000:
        length = 3
    elif code & 0b11000000 == 0b11000000:
        length = 2
    else:
        return chr(code)
    encoded = bytearray([code])
    for _ in range(length - 1):
        continuation = stdscr.getch()
        if not 0 <= continuation <= 255:
            return ""
        encoded.append(continuation)
    return encoded.decode("utf-8", errors="ignore")


def _search_prompt(stdscr: curses.window, previous: str = "") -> str | None:
    query = ""
    try:
        curses.curs_set(1)
    except curses.error:
        pass
    try:
        while True:
            height, width = stdscr.getmaxyx()
            row = max(0, height - 1)
            prompt = "/" + query
            try:
                stdscr.move(row, 0)
                stdscr.clrtoeol()
                stdscr.addnstr(row, 0, prompt, max(1, width - 1), curses.A_BOLD)
                stdscr.move(row, min(max(0, width - 1), len(prompt)))
                stdscr.refresh()
            except curses.error:
                pass
            key = _get_input_key(stdscr)
            if key in ("\n", "\r", curses.KEY_ENTER):
                return query or previous
            if key in ("\x1b", 27):
                return None
            if key in ("\b", "\x7f", curses.KEY_BACKSPACE):
                query = query[:-1]
            elif isinstance(key, str) and key.isprintable():
                query += key
    finally:
        try:
            curses.curs_set(0)
        except curses.error:
            pass


def _find_match(lines: Sequence[str], query: str, start: int, direction: int) -> int | None:
    if not lines or not query:
        return None
    needle = query.casefold()
    for offset in range(1, len(lines) + 1):
        index = (start + direction * offset) % len(lines)
        if needle in lines[index].casefold():
            return index
    return None


def _draw_line(
    stdscr: curses.window,
    row: int,
    text: str,
    width: int,
    *,
    style: int = curses.A_NORMAL,
    query: str = "",
) -> None:
    available = max(1, width - 1)
    clipped = text[:available]
    if not query or query.casefold() not in clipped.casefold():
        try:
            stdscr.addnstr(row, 0, clipped, available, style)
        except curses.error:
            pass
        return

    needle = query.casefold()
    folded = clipped.casefold()
    position = 0
    column = 0
    while position < len(clipped):
        match = folded.find(needle, position)
        end = match if match >= 0 else len(clipped)
        if end > position:
            part = clipped[position:end]
            try:
                stdscr.addnstr(row, column, part, available - column, style)
            except curses.error:
                return
            column += len(part)
        if match < 0 or column >= available:
            return
        part = clipped[match : match + len(query)]
        try:
            stdscr.addnstr(
                row,
                column,
                part,
                available - column,
                style | curses.A_BOLD | curses.A_UNDERLINE,
            )
        except curses.error:
            return
        column += len(part)
        position = match + len(query)


def _choose(stdscr: curses.window, title: str, items: Sequence[str]) -> str:
    selected = 0
    search = ""
    status = ""
    while True:
        stdscr.erase()
        height, width = stdscr.getmaxyx()
        _draw_line(stdscr, 0, title, width, style=curses.A_BOLD)
        top = max(0, selected - max(1, height - 4) + 1)
        for row, item in enumerate(items[top : top + max(1, height - 3)], start=1):
            index = top + row - 1
            marker = "› " if index == selected else "  "
            style = curses.A_BOLD if index == selected else curses.A_NORMAL
            _draw_line(stdscr, row, marker + item, width, style=style, query=search)
        footer = status or "↑/↓ — выбор, Enter — продолжить, / — поиск, n/N — дальше/назад, q — выход"
        try:
            stdscr.addnstr(height - 1, 0, footer, max(1, width - 1), curses.A_DIM)
        except curses.error:
            pass
        key = stdscr.getch()
        if key in (curses.KEY_UP, ord("k")):
            selected = (selected - 1) % len(items)
            status = ""
        elif key in (curses.KEY_DOWN, ord("j")):
            selected = (selected + 1) % len(items)
            status = ""
        elif key in (10, 13, curses.KEY_ENTER):
            return items[selected]
        elif key == ord("/"):
            entered = _search_prompt(stdscr, search)
            if entered:
                search = entered
                match = _find_match(items, search, selected - 1, 1)
                if match is None:
                    status = f"Не найдено: {search}"
                else:
                    selected, status = match, f"/{search}"
        elif key in (ord("n"), ord("N")) and search:
            direction = 1 if key == ord("n") else -1
            match = _find_match(items, search, selected, direction)
            if match is None:
                status = f"Не найдено: {search}"
            else:
                selected, status = match, f"/{search}"
        elif key in (ord("q"), 27):
            raise KeyboardInterrupt


def _show_loading(stdscr: curses.window, message: str) -> None:
    stdscr.erase()
    height, width = stdscr.getmaxyx()
    try:
        stdscr.addnstr(0, 0, message, max(1, width - 1), curses.A_BOLD)
    except curses.error:
        pass
    stdscr.refresh()


def _pick_group(stdscr: curses.window, client: MaiScheduleClient) -> str:
    _show_loading(stdscr, "Загружаю список институтов…")
    institutes, courses = client.catalog()
    institute = _choose(stdscr, "Выберите институт", institutes)
    course = _choose(stdscr, f"{institute}: выберите курс", courses)
    _show_loading(stdscr, "Загружаю список групп…")
    groups = client.list_groups(institute, course)
    return _choose(stdscr, f"{institute}, {course} курс: выберите группу", groups)


def _browser(stdscr: curses.window, client: MaiScheduleClient, group: str | None) -> None:
    if curses.has_colors():
        try:
            curses.start_color()
            curses.use_default_colors()
        except curses.error:
            pass
    try:
        curses.curs_set(0)
    except curses.error:
        pass
    stdscr.keypad(True)
    group = group or _pick_group(stdscr, client)
    _show_loading(stdscr, f"Загружаю расписание {group}…")
    lessons = client.fetch(group)
    monday = current_schedule_monday()
    mode = "week"
    scroll = 0
    search = ""
    match_line: int | None = None
    status = ""
    while True:
        if mode == "labs":
            visible = [lesson for lesson in lessons if is_lab(lesson)]
            title = "Все лабораторные работы"
        elif mode == "exams":
            visible = [lesson for lesson in lessons if is_exam(lesson)]
            title = "Все экзамены"
        else:
            visible = select_weeks(lessons, [monday])
            title = week_title(monday)
        content = render_text(group, visible, title=title).splitlines()
        stdscr.erase()
        height, width = stdscr.getmaxyx()
        body_height = max(1, height - 2)
        scroll = min(scroll, max(0, len(content) - body_height))
        for row, line in enumerate(content[scroll : scroll + body_height]):
            absolute_row = scroll + row
            style = curses.A_BOLD if absolute_row < 2 else curses.A_NORMAL
            _draw_line(stdscr, row, line, width, style=style, query=search)
        footer = status or "←/→ неделя · / поиск · n/N дальше/назад · l лабораторные · e экзамены · q выход"
        try:
            stdscr.addnstr(height - 1, 0, footer, max(1, width - 1), curses.A_DIM)
        except curses.error:
            pass
        key = stdscr.getch()
        if key == ord("q"):
            return
        if key == 27:
            if search:
                search, match_line, status = "", None, ""
                continue
            return
        if key == curses.KEY_LEFT:
            monday -= timedelta(weeks=1)
            mode, scroll, match_line, status = "week", 0, None, ""
        elif key == curses.KEY_RIGHT:
            monday += timedelta(weeks=1)
            mode, scroll, match_line, status = "week", 0, None, ""
        elif key == ord("w"):
            mode, scroll, match_line, status = "week", 0, None, ""
        elif key == ord("l"):
            mode, scroll, match_line, status = "labs", 0, None, ""
        elif key == ord("e"):
            mode, scroll, match_line, status = "exams", 0, None, ""
        elif key == ord("/"):
            entered = _search_prompt(stdscr, search)
            if entered:
                search = entered
                match = _find_match(content, search, scroll - 1, 1)
                if match is None:
                    status = f"Не найдено: {search}"
                else:
                    match_line, status = match, f"/{search}"
                    scroll = min(max(0, match - body_height // 2), max(0, len(content) - body_height))
        elif key in (ord("n"), ord("N")) and search:
            direction = 1 if key == ord("n") else -1
            start = match_line if match_line is not None else scroll
            match = _find_match(content, search, start, direction)
            if match is None:
                status = f"Не найдено: {search}"
            else:
                match_line, status = match, f"/{search}"
                scroll = min(max(0, match - body_height // 2), max(0, len(content) - body_height))
        elif key in (curses.KEY_DOWN, ord("j")):
            scroll = min(max(0, len(content) - body_height), scroll + 1)
            status = ""
        elif key in (curses.KEY_UP, ord("k")):
            scroll = max(0, scroll - 1)
            status = ""
        elif key == curses.KEY_NPAGE:
            scroll = min(max(0, len(content) - body_height), scroll + body_height)
            status = ""
        elif key == curses.KEY_PPAGE:
            scroll = max(0, scroll - body_height)
            status = ""


def run_tui(client: MaiScheduleClient, group: str | None = None) -> None:
    curses.wrapper(_browser, client, group)
