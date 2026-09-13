from __future__ import annotations

import argparse
from concurrent.futures import ThreadPoolExecutor, as_completed
import json
import os
from pathlib import Path
import sys
import time
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode
from urllib.request import Request, urlopen


PACKAGE_NAME = "mai-schedule"
# GitLab требует сегмент версии в URL Generic Package. Постоянное значение
# используется только как часть адреса: историю снимков мы не храним.
PACKAGE_VERSION = "latest"


class PublishError(RuntimeError):
    pass


def _auth_headers() -> dict[str, str]:
    package_token = os.environ.get("PACKAGE_TOKEN")
    if package_token:
        return {"PRIVATE-TOKEN": package_token}
    job_token = os.environ.get("CI_JOB_TOKEN")
    if job_token:
        return {"JOB-TOKEN": job_token}
    raise PublishError("Не задан CI_JOB_TOKEN или PACKAGE_TOKEN")


def _error_text(error: HTTPError) -> str:
    try:
        return error.read().decode("utf-8", errors="replace")
    except OSError:
        return ""


def _open(request: Request, expected: set[int]):
    try:
        response = urlopen(request, timeout=60)
    except HTTPError as error:
        details = _error_text(error)
        suffix = f": {details}" if details else ""
        raise PublishError(f"GitLab API вернул HTTP {error.code}{suffix}") from error
    except URLError as error:
        raise PublishError(f"GitLab API недоступен: {error.reason}") from error
    if response.status not in expected:
        response.close()
        raise PublishError(f"GitLab API вернул неожиданный HTTP {response.status}")
    return response


def find_existing_packages(
    base_url: str,
    project_id: str,
    headers: dict[str, str],
) -> list[int]:
    result: list[int] = []
    page = 1
    while True:
        query = urlencode(
            {
                "package_type": "generic",
                "package_name": PACKAGE_NAME,
                "package_version": PACKAGE_VERSION,
                "per_page": 100,
                "page": page,
            }
        )
        url = f"{base_url}/projects/{quote(project_id, safe='')}/packages?{query}"
        request = Request(url, headers=headers, method="GET")
        with _open(request, {200}) as response:
            payload = json.loads(response.read())
            next_page = response.headers.get("X-Next-Page", "")
        if not isinstance(payload, list):
            raise PublishError("GitLab API вернул некорректный список пакетов")
        for package in payload:
            if (
                package.get("name") == PACKAGE_NAME
                and package.get("version") == PACKAGE_VERSION
                and package.get("package_type") == "generic"
            ):
                result.append(int(package["id"]))
        if not next_page:
            return result
        page = int(next_page)


def delete_package(
    base_url: str,
    project_id: str,
    headers: dict[str, str],
    package_id: int,
) -> None:
    url = f"{base_url}/projects/{quote(project_id, safe='')}/packages/{package_id}"
    request = Request(url, headers=headers, method="DELETE")
    try:
        with _open(request, {204}):
            return
    except PublishError as error:
        if "HTTP 403" in str(error):
            raise PublishError(
                "GitLab запретил удаление прежнего расписания. Разрешите job token "
                "управлять Packages API либо добавьте masked-переменную PACKAGE_TOKEN "
                "с project access token (роль Maintainer, scope api)."
            ) from error
        raise


def upload(
    base_url: str,
    project_id: str,
    headers: dict[str, str],
    source: Path,
    remote_name: str,
) -> None:
    encoded_name = quote(remote_name, safe="/")
    url = (
        f"{base_url}/projects/{quote(project_id, safe='')}/packages/generic/"
        f"{PACKAGE_NAME}/{PACKAGE_VERSION}/{encoded_name}"
    )
    data = source.read_bytes()
    request_headers = {**headers, "Content-Type": "application/json"}
    for attempt in range(3):
        request = Request(url, data=data, headers=request_headers, method="PUT")
        try:
            with _open(request, {200, 201}):
                return
        except PublishError:
            if attempt == 2:
                raise
            time.sleep(2**attempt)


def _files_to_publish(directory: Path) -> list[tuple[Path, str]]:
    groups_file = directory / "groups.json"
    if not groups_file.is_file():
        raise PublishError(f"Не найден {groups_file}")
    try:
        catalog = json.loads(groups_file.read_text(encoding="utf-8"))
        groups = catalog["groups"]
    except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
        raise PublishError(f"Некорректный {groups_file}: {error}") from error
    if not isinstance(groups, list) or not all(isinstance(group, str) for group in groups):
        raise PublishError(f"Некорректный список групп в {groups_file}")

    files: list[tuple[Path, str]] = [(groups_file, "groups.json")]
    for group in groups:
        group_directory = directory / group
        if not group_directory.is_dir():
            raise PublishError(f"Не найден каталог группы {group_directory}")
        group_files = sorted(group_directory.glob("*.json"))
        if not group_files:
            raise PublishError(f"В {group_directory} нет JSON-файлов")
        files.extend((path, f"{group}/{path.name}") for path in group_files)
    return files


def publish(directory: Path, *, workers: int = 8) -> int:
    base_url = os.environ.get("CI_API_V4_URL")
    project_id = os.environ.get("CI_PROJECT_ID")
    if not base_url or not project_id:
        raise PublishError("Не заданы CI_API_V4_URL и CI_PROJECT_ID")
    headers = _auth_headers()
    files = _files_to_publish(directory)

    old_packages = find_existing_packages(base_url, project_id, headers)
    for package_id in old_packages:
        delete_package(base_url, project_id, headers, package_id)
        print(f"Удалён прежний пакет {PACKAGE_NAME}/{PACKAGE_VERSION} (id={package_id})", flush=True)

    # Первый файл создаёт единственный package/version до параллельной загрузки
    # остальных файлов и тем самым исключает гонку создания пакета.
    first_source, first_remote_name = files[0]
    upload(base_url, project_id, headers, first_source, first_remote_name)
    print(f"[1/{len(files)}] {first_remote_name}", flush=True)
    remaining = files[1:]
    if not remaining:
        return 1

    with ThreadPoolExecutor(max_workers=workers) as executor:
        futures = {
            executor.submit(upload, base_url, project_id, headers, source, remote_name): remote_name
            for source, remote_name in remaining
        }
        completed = 1
        for future in as_completed(futures):
            remote_name = futures[future]
            future.result()
            completed += 1
            print(f"[{completed}/{len(files)}] {remote_name}", flush=True)
    return len(files)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Полностью заменить ежедневное расписание в GitLab Generic Package Registry"
    )
    parser.add_argument("directory", type=Path)
    parser.add_argument("--workers", type=int, default=8)
    args = parser.parse_args(argv)
    if not 1 <= args.workers <= 32:
        parser.error("--workers должен быть от 1 до 32")
    try:
        count = publish(args.directory, workers=args.workers)
    except (OSError, PublishError, ValueError) as error:
        print(f"Ошибка публикации: {error}", file=sys.stderr)
        return 1
    print(f"Опубликовано файлов: {count}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
