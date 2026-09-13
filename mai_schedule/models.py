from __future__ import annotations

from dataclasses import asdict, dataclass
from datetime import date
import re
from typing import Any, Iterable


@dataclass(frozen=True, slots=True)
class Lesson:
    date: date
    weekday: str
    time_start: str
    time_end: str
    name: str
    kind: str
    lecturers: tuple[str, ...]
    rooms: tuple[str, ...]
    links: tuple[str, ...] = ()

    @property
    def week(self) -> str:
        year, week, _ = self.date.isocalendar()
        return f"{year}-W{week:02d}"

    def to_dict(self) -> dict[str, Any]:
        result = asdict(self)
        result["date"] = self.date.isoformat()
        result["lecturers"] = list(self.lecturers)
        result["rooms"] = list(self.rooms)
        result["links"] = list(self.links)
        return result


def _normalise_kind(value: str) -> str:
    return re.sub(r"[\s._-]+", "", value).upper().replace("Ё", "Е")


def is_lab(lesson: Lesson) -> bool:
    kind = _normalise_kind(lesson.kind)
    return kind in {"ЛР", "ЛАБ", "LAB"} or "ЛАБОРАТОР" in kind


def is_exam(lesson: Lesson) -> bool:
    kind = _normalise_kind(lesson.kind)
    return kind in {"Э", "ЭК", "ЭКЗ", "EXAM"} or "ЭКЗАМЕН" in kind


def lessons_to_dicts(lessons: Iterable[Lesson]) -> list[dict[str, Any]]:
    return [lesson.to_dict() for lesson in lessons]
