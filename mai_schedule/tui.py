from __future__ import annotations

import curses
from datetime import timedelta
from typing import Sequence

from .client import MaiScheduleClient
from .models import Lesson, is_exam, is_lab
from .render import render_text, week_title
from .selection import current_schedule_monday, select_weeks


def _choose(stdscr: curses.window, title: str, items: Sequence[str]) -> str:
    selected = 0
    while True:
        stdscr.erase()
        height, width = stdscr.getmaxyx()
        stdscr.addnstr(0, 0, title, max(1, width - 1), curses.A_BOLD)
        top = max(0, selected - max(1, height - 4) + 1)
        for row, item in enumerate(items[top : top + max(1, height - 3)], start=1):
            index = top + row - 1
            marker = "› " if index == selected else "  "
            style = curses.A_REVERSE if index == selected else curses.A_NORMAL
            try:
                stdscr.addnstr(row, 0, marker + item, max(1, width - 1), style)
            except curses.error:
                pass
        try:
            stdscr.addnstr(height - 1, 0, "↑/↓ — выбор, Enter — продолжить, q — выход", max(1, width - 1))
        except curses.error:
            pass
        key = stdscr.getch()
        if key in (curses.KEY_UP, ord("k")):
            selected = (selected - 1) % len(items)
        elif key in (curses.KEY_DOWN, ord("j")):
            selected = (selected + 1) % len(items)
        elif key in (10, 13, curses.KEY_ENTER):
            return items[selected]
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
            try:
                stdscr.addnstr(row, 0, line, max(1, width - 1), curses.A_BOLD if row < 2 else 0)
            except curses.error:
                pass
        footer = "←/→ неделя · w неделя · l лабораторные · e экзамены · ↑/↓ прокрутка · q выход"
        try:
            stdscr.addnstr(height - 1, 0, footer, max(1, width - 1), curses.A_REVERSE)
        except curses.error:
            pass
        key = stdscr.getch()
        if key in (ord("q"), 27):
            return
        if key == curses.KEY_LEFT:
            monday -= timedelta(weeks=1)
            mode, scroll = "week", 0
        elif key == curses.KEY_RIGHT:
            monday += timedelta(weeks=1)
            mode, scroll = "week", 0
        elif key == ord("w"):
            mode, scroll = "week", 0
        elif key == ord("l"):
            mode, scroll = "labs", 0
        elif key == ord("e"):
            mode, scroll = "exams", 0
        elif key in (curses.KEY_DOWN, ord("j")):
            scroll = min(max(0, len(content) - body_height), scroll + 1)
        elif key in (curses.KEY_UP, ord("k")):
            scroll = max(0, scroll - 1)
        elif key == curses.KEY_NPAGE:
            scroll = min(max(0, len(content) - body_height), scroll + body_height)
        elif key == curses.KEY_PPAGE:
            scroll = max(0, scroll - body_height)


def run_tui(client: MaiScheduleClient, group: str | None = None) -> None:
    curses.wrapper(_browser, client, group)
