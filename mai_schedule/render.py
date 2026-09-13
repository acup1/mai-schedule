from __future__ import annotations

from datetime import date, timedelta
import json
from typing import Iterable

from .models import Lesson, lessons_to_dicts
from .selection import week_key


def payload(group: str, lessons: Iterable[Lesson], *, kind: str, week: str | None = None) -> dict:
    result = {"group": group, "kind": kind, "lessons": lessons_to_dicts(lessons)}
    if week is not None:
        result["week"] = week
    return result


def render_json(group: str, lessons: Iterable[Lesson], *, kind: str, week: str | None = None) -> str:
    return json.dumps(payload(group, lessons, kind=kind, week=week), ensure_ascii=False, indent=2)


def render_text(group: str, lessons: Iterable[Lesson], *, title: str | None = None) -> str:
    items = list(lessons)
    lines = [f"Группа: {group}"]
    if title:
        lines.append(title)
    if not items:
        lines.append("Нет занятий.")
        return "\n".join(lines)
    current_week: str | None = None
    current_date: date | None = None
    show_week_headers = len({lesson.week for lesson in items}) > 1 or not (title or "").startswith("Неделя ")
    for lesson in items:
        if lesson.week != current_week:
            current_week = lesson.week
            if show_week_headers:
                lines.extend(("", f"Неделя {current_week}"))
            current_date = None
        if lesson.date != current_date:
            current_date = lesson.date
            lines.extend(("", f"{lesson.date:%d.%m.%Y} ({lesson.weekday})"))
        details = " | ".join(part for part in (", ".join(lesson.rooms), ", ".join(lesson.lecturers)) if part)
        row = f"  {_short_time(lesson.time_start)}–{_short_time(lesson.time_end)}  [{lesson.kind or '—'}] {lesson.name}"
        if details:
            row += f" — {details}"
        lines.append(row)
        for link in lesson.links:
            lines.append(f"      {link}")
    return "\n".join(lines)


def week_title(monday: date) -> str:
    sunday = monday + timedelta(days=6)
    return f"Неделя {week_key(monday)}: {monday:%d.%m.%Y}–{sunday:%d.%m.%Y}"


def _short_time(value: str) -> str:
    parts = value.split(":")
    if len(parts) < 2:
        return value
    try:
        return f"{int(parts[0]):02d}:{int(parts[1]):02d}"
    except ValueError:
        return value
