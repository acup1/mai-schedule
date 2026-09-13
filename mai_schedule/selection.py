from __future__ import annotations

from datetime import date, datetime, timedelta, timezone
import re
from typing import Iterable

from .models import Lesson


WEEK_RE = re.compile(r"^(\d{4})-W(\d{1,2})$", re.IGNORECASE)


def monday_for(value: date) -> date:
    return value - timedelta(days=value.weekday())


def week_key(value: date) -> str:
    year, week, _ = value.isocalendar()
    return f"{year}-W{week:02d}"


def today_in_mai() -> date:
    return datetime.now(timezone(timedelta(hours=3))).date()


def parse_week(value: str, *, today: date | None = None) -> date:
    today = today or today_in_mai()
    current = monday_for(today)
    normalised = value.strip().lower()
    if normalised in {"current", "now", "текущая"}:
        return current
    if normalised in {"next", "следующая"}:
        return current + timedelta(weeks=1)
    if normalised in {"previous", "prev", "предыдущая"}:
        return current - timedelta(weeks=1)
    if re.fullmatch(r"[+-]\d+", normalised):
        return current + timedelta(weeks=int(normalised))
    match = WEEK_RE.fullmatch(value.strip())
    if match:
        try:
            return date.fromisocalendar(int(match.group(1)), int(match.group(2)), 1)
        except ValueError as error:
            raise ValueError(f"Некорректная ISO-неделя: {value}") from error
    if re.fullmatch(r"\d{1,2}", normalised):
        try:
            return date.fromisocalendar(today.isocalendar().year, int(normalised), 1)
        except ValueError as error:
            raise ValueError(f"Некорректный номер недели: {value}") from error
    try:
        return monday_for(date.fromisoformat(value))
    except ValueError as error:
        raise ValueError(
            f"Неизвестная неделя «{value}»: используйте current, +1, YYYY-Www или YYYY-MM-DD"
        ) from error


def expand_week_selectors(values: Iterable[str], *, today: date | None = None) -> list[date]:
    selectors = list(values) or ["current"]
    result: list[date] = []
    for selector in selectors:
        if ".." not in selector:
            result.append(parse_week(selector, today=today))
            continue
        left, right = selector.split("..", 1)
        start = parse_week(left, today=today)
        end = parse_week(right, today=today)
        if end < start:
            raise ValueError(f"Конец диапазона раньше начала: {selector}")
        if (end - start).days > 7 * 104:
            raise ValueError("Диапазон не может быть длиннее 104 недель")
        cursor = start
        while cursor <= end:
            result.append(cursor)
            cursor += timedelta(weeks=1)
    return list(dict.fromkeys(result))


def select_weeks(lessons: Iterable[Lesson], mondays: Iterable[date]) -> list[Lesson]:
    keys = {week_key(monday) for monday in mondays}
    return [lesson for lesson in lessons if lesson.week in keys]
