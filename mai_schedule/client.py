from __future__ import annotations

from datetime import datetime
import hashlib
from html.parser import HTMLParser
import json
import socket
import ssl
import time
from typing import Any
from urllib.error import HTTPError, URLError
from urllib.parse import parse_qs, urlencode, urlparse
from urllib.request import Request, urlopen

from .models import Lesson


GROUPS_URL = "https://mai.ru/education/studies/schedule/groups.php"
DATA_URL = "https://public.mai.ru/schedule/data/{digest}.json"
USER_AGENT = "mai-schedule/1.0 (+https://github.com/acup1/mai-schedule)"


class ScheduleError(RuntimeError):
    """An expected network or source-format error."""


class _SelectAndGroupParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__()
        self.select: str | None = None
        self.option_value: str | None = None
        self.institutes: list[str] = []
        self.courses: list[str] = []
        self.groups: list[str] = []

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        attributes = dict(attrs)
        if tag == "select":
            self.select = attributes.get("name")
        elif tag == "option" and self.select:
            self.option_value = attributes.get("value")
        elif tag == "a":
            href = attributes.get("href") or ""
            query = parse_qs(urlparse(href).query)
            if "group" in query:
                group = query["group"][0].strip()
                if group and group not in self.groups:
                    self.groups.append(group)

    def handle_endtag(self, tag: str) -> None:
        if tag == "select":
            self.select = None
        elif tag == "option" and self.option_value is not None:
            value = self.option_value.strip()
            if value and value != "all":
                target = self.institutes if self.select == "department" else self.courses
                if value not in target:
                    target.append(value)
            self.option_value = None


class MaiScheduleClient:
    def __init__(self, *, timeout: float = 20.0, retries: int = 3) -> None:
        self.timeout = timeout
        self.retries = retries
        self.ssl_context = ssl._create_unverified_context()

    def _get(self, url: str) -> bytes:
        request = Request(url, headers={"User-Agent": USER_AGENT, "Accept": "application/json,text/html"})
        last_error: Exception | None = None
        for attempt in range(self.retries):
            try:
                with urlopen(request, timeout=self.timeout, context=self.ssl_context) as response:
                    return response.read()
            except HTTPError as error:
                if error.code < 500:
                    raise ScheduleError(f"МАИ вернул HTTP {error.code} для {url}") from error
                last_error = error
            except (URLError, TimeoutError, socket.timeout) as error:
                last_error = error
            if attempt + 1 < self.retries:
                time.sleep(0.5 * (2**attempt))
        raise ScheduleError(f"Не удалось получить данные с {url}: {last_error}") from last_error

    def _parse_groups_page(self, url: str) -> _SelectAndGroupParser:
        try:
            html = self._get(url).decode("utf-8")
        except UnicodeDecodeError as error:
            raise ScheduleError("Страница списка групп МАИ имеет неизвестную кодировку") from error
        parser = _SelectAndGroupParser()
        parser.feed(html)
        return parser

    def catalog(self) -> tuple[list[str], list[str]]:
        parser = self._parse_groups_page(GROUPS_URL)
        if not parser.institutes or not parser.courses:
            raise ScheduleError("На странице МАИ не найден список институтов или курсов")
        return parser.institutes, parser.courses

    def list_groups(self, institute: str, course: str | int = "all") -> list[str]:
        query = urlencode({"department": institute, "course": str(course)})
        parser = self._parse_groups_page(f"{GROUPS_URL}?{query}")
        if not parser.groups:
            raise ScheduleError(f"Для «{institute}», курс {course}, группы не найдены")
        return parser.groups

    def list_all_groups(self) -> list[str]:
        institutes, _ = self.catalog()
        groups: list[str] = []
        for institute in institutes:
            for group in self.list_groups(institute, "all"):
                if group not in groups:
                    groups.append(group)
        return groups

    def fetch(self, group: str) -> list[Lesson]:
        group = group.strip()
        if not group:
            raise ScheduleError("Имя группы не может быть пустым")
        digest = hashlib.md5(group.encode("utf-8"), usedforsecurity=False).hexdigest()
        try:
            raw: dict[str, Any] = json.loads(self._get(DATA_URL.format(digest=digest)))
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise ScheduleError(f"Некорректный JSON расписания группы {group}") from error
        if not isinstance(raw, dict):
            raise ScheduleError(f"Некорректный JSON расписания группы {group}: ожидался объект")
        source_group = str(raw.pop("group", group)).strip()
        if source_group.casefold() != group.casefold():
            raise ScheduleError(f"Источник вернул расписание другой группы: {source_group}")
        lessons: list[Lesson] = []
        try:
            for date_text, day_data in raw.items():
                lesson_date = datetime.strptime(date_text, "%d.%m.%Y").date()
                weekday = str(day_data.get("day", ""))
                for slot in day_data.get("pairs", {}).values():
                    for name, item in slot.items():
                        kinds = tuple(str(value) for value in item.get("type", {}).keys()) or ("",)
                        lecturers = _non_empty_values(item.get("lector", {}))
                        rooms = _non_empty_values(item.get("room", {}))
                        links = tuple(
                            str(item[key]).strip()
                            for key in ("lms", "teams", "other")
                            if str(item.get(key, "")).strip()
                        )
                        for kind in kinds:
                            lessons.append(
                                Lesson(
                                    date=lesson_date,
                                    weekday=weekday,
                                    time_start=str(item.get("time_start", "")),
                                    time_end=str(item.get("time_end", "")),
                                    name=str(name).strip(),
                                    kind=kind.strip(),
                                    lecturers=lecturers,
                                    rooms=rooms,
                                    links=links,
                                )
                            )
        except (AttributeError, TypeError, ValueError) as error:
            raise ScheduleError(f"Формат расписания группы {group} изменился") from error
        return sorted(lessons, key=lambda item: (item.date, _time_key(item.time_start), item.name, item.kind))


def _non_empty_values(value: Any) -> tuple[str, ...]:
    if not isinstance(value, dict):
        return ()
    return tuple(str(item).strip() for item in value.values() if str(item).strip())


def _time_key(value: str) -> tuple[int, int, int]:
    try:
        parts = [int(part) for part in value.split(":")]
        padded = (parts + [0, 0, 0])[:3]
        return padded[0], padded[1], padded[2]
    except ValueError:
        return (99, 99, 99)
