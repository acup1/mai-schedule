from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime, timezone
import json
from pathlib import Path
import re
import sys
from typing import Iterable

from .client import MaiScheduleClient, ScheduleError
from .models import Lesson, is_exam, is_lab, lessons_to_dicts
from .selection import monday_for, select_weeks, today_in_mai, week_key


def _write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    with temporary.open("w", encoding="utf-8") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    temporary.replace(path)


def _document(group: str, kind: str, lessons: Iterable[Lesson], generated_at: str, **extra: object) -> dict:
    result = {
        "group": group,
        "kind": kind,
        "generated_at": generated_at,
        "lessons": lessons_to_dicts(lessons),
    }
    result.update(extra)
    return result


def write_group(output: Path, group: str, lessons: list[Lesson], generated_at: str) -> None:
    if not group or group in {".", ".."} or re.search(r"[/\\\0]", group):
        raise ValueError(f"Небезопасное имя группы: {group!r}")
    target = output / group
    target.mkdir(parents=True, exist_ok=True)
    for old_week in target.glob("week-*.json"):
        old_week.unlink()
    weeks = sorted({lesson.week for lesson in lessons})
    for week in weeks:
        selected = [lesson for lesson in lessons if lesson.week == week]
        _write_json(target / f"week-{week}.json", _document(group, "week", selected, generated_at, week=week))
    current_monday = monday_for(today_in_mai())
    current_week = week_key(current_monday)
    _write_json(
        target / "current.json",
        _document(group, "current", select_weeks(lessons, [current_monday]), generated_at, week=current_week),
    )
    _write_json(target / "labs.json", _document(group, "labs", filter(is_lab, lessons), generated_at))
    _write_json(target / "exams.json", _document(group, "exams", filter(is_exam, lessons), generated_at))
    _write_json(
        target / "manifest.json",
        {
            "group": group,
            "generated_at": generated_at,
            "current_week": current_week,
            "files": ["current.json", "labs.json", "exams.json"]
            + [f"week-{week}.json" for week in weeks],
        },
    )


def generate_all(output: Path, *, workers: int = 8, client: MaiScheduleClient | None = None) -> int:
    client = client or MaiScheduleClient()
    generated_at = datetime.now(timezone.utc).isoformat()
    groups = client.list_all_groups()
    _write_json(output / "groups.json", {"generated_at": generated_at, "groups": groups})
    errors: list[tuple[str, str]] = []
    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = {executor.submit(client.fetch, group): group for group in groups}
        completed = 0
        for future in as_completed(futures):
            group = futures[future]
            try:
                write_group(output, group, future.result(), generated_at)
            except (ScheduleError, OSError, ValueError) as error:
                errors.append((group, str(error)))
                print(f"ERROR {group}: {error}", file=sys.stderr, flush=True)
            completed += 1
            print(f"[{completed}/{len(groups)}] {group}", flush=True)
    if errors:
        _write_json(output / "errors.json", {"generated_at": generated_at, "errors": errors})
        raise ScheduleError(f"Не удалось сгенерировать расписание для {len(errors)} из {len(groups)} групп")
    return len(groups)
