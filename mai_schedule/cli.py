from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

from .client import MaiScheduleClient, ScheduleError
from .generator import generate_all
from .models import is_exam, is_lab
from .render import render_json, render_text, week_title
from .selection import expand_week_selectors, select_weeks


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(prog="mai-schedule", description="Расписание учебных групп МАИ")
    parser.add_argument("--timeout", type=float, default=20.0, help="таймаут HTTP-запроса, секунды")
    subparsers = parser.add_subparsers(dest="command")

    raw = subparsers.add_parser("raw", help="неинтерактивно вывести расписание")
    raw.add_argument("group", help="название группы, например М3О-101БВ-26")
    raw.add_argument(
        "weeks",
        nargs="*",
        help="current, +1, YYYY-Www, YYYY-MM-DD, диапазон A..B; по умолчанию current",
    )
    filters = raw.add_mutually_exclusive_group()
    filters.add_argument("--labs", action="store_true", help="все лабораторные работы")
    filters.add_argument("--exams", action="store_true", help="все экзамены")
    raw.add_argument("--json", action="store_true", help="вывести JSON")

    tui = subparsers.add_parser("tui", help="интерактивный терминальный интерфейс")
    tui.add_argument("group", nargs="?", help="группа; без аргумента будет интерактивный выбор")

    groups = subparsers.add_parser("groups", help="получить список групп")
    groups.add_argument("--institute", help="например: Институт №3")
    groups.add_argument("--course", default="all", help="курс от 1 до 6")
    groups.add_argument("--json", action="store_true", help="вывести JSON")

    generate = subparsers.add_parser("generate", help="сгенерировать текстовые файлы для всех групп")
    generate.add_argument("--output", type=Path, default=Path("dist"))
    generate.add_argument("--workers", type=int, default=8)
    return parser


def _raw(args: argparse.Namespace, client: MaiScheduleClient) -> None:
    lessons = client.fetch(args.group)
    if args.labs:
        selected = list(filter(is_lab, lessons))
        kind, title, week = "labs", "Все лабораторные работы", None
    elif args.exams:
        selected = list(filter(is_exam, lessons))
        kind, title, week = "exams", "Все экзамены", None
    else:
        mondays = expand_week_selectors(args.weeks)
        selected = select_weeks(lessons, mondays)
        keys = [monday.isocalendar() for monday in mondays]
        week_names = [f"{item.year}-W{item.week:02d}" for item in keys]
        kind = "week" if len(week_names) == 1 else "weeks"
        week = week_names[0] if len(week_names) == 1 else None
        title = "; ".join(week_title(monday) for monday in mondays)
    if args.json:
        print(render_json(args.group, selected, kind=kind, week=week))
    else:
        print(render_text(args.group, selected, title=title))


def _groups(args: argparse.Namespace, client: MaiScheduleClient) -> None:
    if args.institute:
        groups = client.list_groups(args.institute, args.course)
        value: object = groups
    else:
        institutes, courses = client.catalog()
        value = {"institutes": institutes, "courses": courses}
    if args.json:
        print(json.dumps(value, ensure_ascii=False, indent=2))
    elif isinstance(value, dict):
        print("Институты:")
        print("\n".join(f"  {item}" for item in value["institutes"]))
        print("Курсы:", ", ".join(value["courses"]))
    else:
        print("\n".join(value))


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    client = MaiScheduleClient(timeout=args.timeout)
    try:
        if args.command == "raw":
            _raw(args, client)
        elif args.command == "groups":
            _groups(args, client)
        elif args.command == "generate":
            if not 1 <= args.workers <= 32:
                parser.error("--workers должен быть от 1 до 32")
            count = generate_all(args.output, workers=args.workers, client=client)
            print(f"Сгенерировано групп: {count}")
        else:
            if not sys.stdin.isatty() or not sys.stdout.isatty():
                parser.error("TUI требует терминал; для скриптов используйте подкоманду raw")
            from .tui import run_tui

            run_tui(client, getattr(args, "group", None))
    except KeyboardInterrupt:
        return 130
    except BrokenPipeError:
        return 0
    except (ScheduleError, ValueError, OSError) as error:
        print(f"Ошибка: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
