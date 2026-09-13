from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path
import re
import sys

from .client import MaiScheduleClient, ScheduleError
from .models import Lesson, is_exam, is_lab
from .render import render_text, week_title
from .selection import current_schedule_monday, parse_week, select_weeks


def _write_text(path: Path, value: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    temporary.write_text(value.rstrip("\n") + "\n", encoding="utf-8")
    temporary.replace(path)


def write_group(output: Path, group: str, lessons: list[Lesson]) -> None:
    if not group or group in {".", ".."} or re.search(r"[/\\\0]", group):
        raise ValueError(f"Небезопасное имя группы: {group!r}")
    target = output / group
    target.mkdir(parents=True, exist_ok=True)
    for old_week in target.glob("week-*.txt"):
        old_week.unlink()
    for obsolete in ("current.json", "labs.json", "exams.json", "manifest.json"):
        (target / obsolete).unlink(missing_ok=True)
    for obsolete_week in target.glob("week-*.json"):
        obsolete_week.unlink()
    weeks = sorted({lesson.week for lesson in lessons})
    for week in weeks:
        selected = [lesson for lesson in lessons if lesson.week == week]
        monday = parse_week(week)
        _write_text(
            target / f"week-{week}.txt",
            render_text(group, selected, title=week_title(monday)),
        )
    current_monday = current_schedule_monday()
    _write_text(
        target / "current.txt",
        render_text(
            group,
            select_weeks(lessons, [current_monday]),
            title=week_title(current_monday),
        ),
    )
    _write_text(
        target / "labs.txt",
        render_text(group, filter(is_lab, lessons), title="Все лабораторные работы"),
    )
    _write_text(
        target / "exams.txt",
        render_text(group, filter(is_exam, lessons), title="Все экзамены"),
    )


def generate_all(output: Path, *, workers: int = 8, client: MaiScheduleClient | None = None) -> int:
    client = client or MaiScheduleClient()
    groups = client.list_all_groups()
    for obsolete in ("groups.json", "errors.json", "errors.txt"):
        (output / obsolete).unlink(missing_ok=True)
    _write_text(output / "groups.txt", "\n".join(groups))
    errors: list[tuple[str, str]] = []
    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = {executor.submit(client.fetch, group): group for group in groups}
        completed = 0
        for future in as_completed(futures):
            group = futures[future]
            try:
                write_group(output, group, future.result())
            except (ScheduleError, OSError, ValueError) as error:
                errors.append((group, str(error)))
                print(f"ERROR {group}: {error}", file=sys.stderr, flush=True)
            completed += 1
            print(f"[{completed}/{len(groups)}] {group}", flush=True)
    if errors:
        _write_text(
            output / "errors.txt",
            "\n".join(f"{group}: {error}" for group, error in errors),
        )
        raise ScheduleError(f"Не удалось сгенерировать расписание для {len(errors)} из {len(groups)} групп")
    return len(groups)
