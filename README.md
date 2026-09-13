# Расписание МАИ

Консольная программа получает актуальное расписание учебных групп с публичных
сервисов МАИ. Есть неинтерактивный raw-режим, TUI с перелистыванием недель и
генератор текстовых файлов для GitLab Generic Package Registry.

Источник данных — официальные страницы списка групп
`mai.ru/education/studies/schedule/groups.php` и JSON-файлы
`public.mai.ru/schedule/data/`. Программа не связана с администрацией МАИ.

[Открыть расписание группы М4О-408Б-23 на текущую неделю](https://gitlab.mai.ru/api/v4/projects/GAPoroshin%2Fschedule/packages/generic/mai-schedule/latest/М4О-408Б-23/current.txt)

## Быстрый запуск

Нужен только Python 3.10 или новее. Runtime-зависимостей нет, поэтому создавать
виртуальное окружение и устанавливать пакет не обязательно:

```bash
./mai-schedule --help
./mai-schedule raw М3О-101БВ-26
```

То же самое без исполняемого файла: `python3 -m mai_schedule ...`.

### Установка через Nix Flake

Из каталога проекта:

```bash
nix profile install .
mai-schedule --help
```

После публикации репозитория программу можно установить напрямую из GitLab:

```bash
nix profile install 'gitlab:GAPoroshin/schedule?host=gitlab.mai.ru'
```

Запуск локальной копии без установки:

```bash
nix run . -- raw М3О-101БВ-26
```

Запустить программу прямо из удалённого репозитория и проверить, что flake
работает:

```bash
nix run --refresh 'gitlab:GAPoroshin/schedule?host=gitlab.mai.ru' -- --help
```

`--refresh` нужен для проверки сразу после push: он не даёт Nix использовать
закэшированный предыдущий commit.

Например, сразу получить текущую неделю группы:

```bash
nix run 'gitlab:GAPoroshin/schedule?host=gitlab.mai.ru' -- raw М3О-101БВ-26
```

Эти команды увидят новый flake после того, как `flake.nix` и `flake.lock` будут
загружены в репозиторий. Если проект приватный, используйте настроенный SSH:

```bash
nix run 'git+ssh://git@gitlab.mai.ru:2222/GAPoroshin/schedule.git' -- --help
```

Обновить установленную программу можно командой `nix profile upgrade
mai-schedule`.

### Добавление в конфигурацию NixOS через flake

В существующем системном `flake.nix` добавьте репозиторий в `inputs` и пакет в
`environment.systemPackages`:

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    maiSchedule.url = "gitlab:GAPoroshin/schedule?host=gitlab.mai.ru";
  };

  outputs = { nixpkgs, maiSchedule, ... }: {
    nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ./configuration.nix
        ({ pkgs, ... }: {
          environment.systemPackages = [
            maiSchedule.packages.${pkgs.system}.default
          ];
        })
      ];
    };
  };
}
```

Замените `my-host` на имя своей конфигурации, а `system` — на свою архитектуру,
например `aarch64-linux`. Затем примените конфигурацию:

```bash
sudo nixos-rebuild switch --flake /etc/nixos#my-host
```

Чтобы позднее получить новую версию программы и применить её:

```bash
cd /etc/nixos
nix flake update maiSchedule
sudo nixos-rebuild switch --flake .#my-host
```

### Необязательная установка через pip

Если у вашего Python работает `venv`/`ensurepip`, можно установить команду
`mai-schedule` обычным способом:

```bash
python3 -m venv .venv
source .venv/bin/activate
python -m pip install .
```

На Nix-Python создание окружения может остановиться на `ensurepip`. В таком
случае используйте `./mai-schedule`: для этой программы окружение не нужно. Если
создание `.venv` было прервано, это неполное окружение — в нём может не быть
`bin/activate`; удалите каталог `.venv` перед новой попыткой создать его.

## Получить расписание конкретной группы

Текущая неделя:

```bash
./mai-schedule raw М3О-101БВ-26
```

По воскресеньям команда без указанной недели и селектор `current` показывают
уже следующую неделю.

Конкретная неделя, несколько недель или диапазон:

```bash
./mai-schedule raw М3О-101БВ-26 2026-W38
./mai-schedule raw М3О-101БВ-26 current +1 +2
./mai-schedule raw М3О-101БВ-26 2026-W38..2026-W42
```

Вместо ISO-недели можно передать любую дату внутри неё: `2026-09-14`.
Для обработки скриптом добавьте `--json`.

Все лабораторные работы или все экзамены, найденные в расписании группы:

```bash
./mai-schedule raw М3О-101БВ-26 --labs
./mai-schedule raw М3О-101БВ-26 --exams --json
```

Обозначения берутся из поля типа занятия: лабораторные распознаются по `ЛР` и
вариантам слова «лабораторная», экзамены — по `ЭКЗ`, `ЭКЗАМЕН` и близким
вариантам. Если МАИ ещё не опубликовал экзамены, список будет пустым.

## TUI

```bash
./mai-schedule
```

Без аргументов программа предлагает стрелками выбрать институт, курс и группу.
Если группа уже известна:

```bash
./mai-schedule tui М3О-101БВ-26
```

По воскресеньям TUI также открывается сразу на следующей неделе.

Клавиши интерфейса:

- `←` / `→` — предыдущая и следующая неделя;
- `l` — все лабораторные, `e` — все экзамены, `w` — вернуться к неделе;
- `↑` / `↓`, `Page Up` / `Page Down` — прокрутка;
- `q` — выход.

Посмотреть каталог без TUI:

```bash
./mai-schedule groups
./mai-schedule groups --institute 'Институт №3' --course 1
```

## Ежедневные текстовые файлы

Команда `./mai-schedule generate --output dist` получает все группы и
создаёт такую структуру:

```text
dist/
├── groups.txt
└── М3О-101БВ-26/
    ├── current.txt
    ├── labs.txt
    ├── exams.txt
    ├── week-2026-W36.txt
    └── week-2026-W37.txt
```

Содержимое файлов группы совпадает с человекочитаемым выводом команды `raw`.
Например, `current.txt` соответствует `./mai-schedule raw М3О-101БВ-26`, а
`labs.txt` — `./mai-schedule raw М3О-101БВ-26 --labs`.
По воскресеньям `current.txt` следует тому же правилу и заранее переключается на
следующую неделю.

Pipeline публикует один Generic Package `mai-schedule`. Его содержимое каждый
день полностью заменяется свежими файлами:

```text
.../packages/generic/mai-schedule/latest/<группа>/<файл>
```

Слово `latest` — постоянная техническая часть URL, которую требует GitLab, а не
версия расписания. История снимков не хранится: перед ежедневной публикацией
предыдущий пакет удаляется, после чего загружается только актуальное расписание.
Временный job artifact живёт один день и нужен лишь для передачи `dist` между
job-ами.

Файл текущей недели для конкретной группы доступен по постоянному адресу (для
приватного проекта добавьте подходящий токен):

```bash
curl -fS \
  'https://gitlab.mai.ru/api/v4/projects/GAPoroshin%2Fschedule/packages/generic/mai-schedule/latest/%D0%9C3%D0%9E-101%D0%91%D0%92-26/current.txt'
```

Для приватного проекта передайте команде `curl` заголовок
`PRIVATE-TOKEN: <token>`.

`.gitlab-ci.yml` запускает генерацию и публикацию. Сам cron задаётся в GitLab:
**Build → Pipeline schedules → New schedule**, например `0 2 * * *` в нужной
временной зоне. Pipeline также можно запустить вручную. Публикатор сначала
использует `CI_JOB_TOKEN`. Если установленный GitLab запрещает этому токену
удалять Packages, добавьте masked CI/CD variable `PACKAGE_TOKEN` с project access
token: роль `Maintainer`, scope `api`.
