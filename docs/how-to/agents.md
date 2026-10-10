# nxr для агентов

Одна страница-якорь: намерение агента -> команда -> ожидаемый результат.
Каждая команда ниже существует.
Установка: `cargo install nexus-raw` (или `npm install @nexus-raw/nxr`, если окружение node).
Команда `service eula --accept` требует nx-admin на сервере.
Чтение доступно любому пользователю, у которого сервер отвечает.

## Cookbook

| Намерение | Команда | Успех выглядит как | Exit |
|:--|:--|:--|:--|
| сервер жив? | `nxr service status <url>` | `alive` / `alive, writable` | 0 |
| что это за репа? | `nxr service repo <url>` | `name format type url` | 0 |
| что где лежит (любой формат)? | `nxr service assets <repo-url>` | NDJSON-строки `asset` + `summary` | 0 |
| сервер не пишет! | `nxr service eula <url> --accept` | `accepted` | 0 |
| что в raw-папке? | `nxr ls <dir-url>` | по строке на entry | 0 |
| то же, для скрипта | `nxr ls <dir-url> --json` | NDJSON: `{"entry":...,"kind":...}` | 0 |
| скачать файл | `nxr get <url> -o <file>` | байты на диске, digest сверён | 0 |
| скачать в stdout | `nxr get <url> -` | поток в stdout | 0 |
| скачать папку | `nxr down <dir-url> --dest <dir>` | `downloaded N` | 0 |
| загрузить папку | `nxr up <dir> <dir-url>` | `uploaded N` + `.sha256`-маркеры | 0 |
| чем отличаются? | `nxr diff <dir> <dir-url>` | дельта: local/remote/same | 0 |
| перенести версию | `nxr mirror <src-url> <dst-url>` | `mirrored N` | 0 |
| переименовать версию | `nxr mv <src-url> <dst-url>` | mirror + удаление источника | 0 |
| удалить версию | `nxr rm --version <ver-url>` | маркер удалён первым, затем байты | 0 |
| снять pointer | `nxr point --clear <ptr-url>` | pointer-файл удалён | 0 |
| проверить локально | `nxr verify <dir>` | digest и маркеры сходятся | 0 |
| диагностика доступа | `nxr doctor <url>` | креды/TLS/достижимость по пунктам | 0 |

Примечание: `ls` и все перечисления требуют источника имён (manifest.json в папке, `--manifest`, `--name` или search API).
Без него - exit 2 (misuse) и `hint:` как источник.

## Контракт для скриптов

- `--json`: одна NDJSON-строка на событие, финальное событие всегда есть.
- Exit-коды: `0` успех, `1` данные (404, digest-мисс), `2` misuse (ошибка вызова), `3` транспорт.
- Каждая ошибка на stderr несёт `hint:` - подсказку следующего действия.
  Подсказка включает `server says:` - короткое тело 4xx/5xx-ответа (например, отказ до принятия EULA, CE 3.79+).
- Креды: `-u user:pass`, иначе `NXR_AUTH` (base64 `user:pass`), иначе `NXR_USERNAME` + `NXR_PASSWORD`.
  Без `-R` конфиг-файла в игре нет, по дизайну.
- Длинный URL? `-R <алиас>` из `~/.config/nxr/config.toml`: `nxr -R releases ls 1.4.0/`.
  Файл читается только когда флаг называет алиас, никакого автоподхвата.

## Странности, которые не баг

- `GET /service/rest/v1/status` отвечает 200 с пустым телом на Nexus 3.79.
- Коллекция `/v1/repositories` урезанная (`size`, пустые `attributes`) даже для админа.
  Полные настройки - только админский `GET /v1/repositories/{format}/{type}/{name}`.
- Аноним и пользователь видят разные списки репозиториев.
  403 на `status/check` значит «пользователь не админ», а не «сервер умер».
- CE 3.79+: любой PUT отвечает 403, пока не принята EULA.
  Лечение: `nxr service eula <url> --accept` или POST `/v1/system/eula` с эхом disclaimer.
- `-o` обязан быть реальным путём: nxr пишет соседний `<имя>.part` и переименовывает после докачки (ресюм-схема).
  Для выброса в stdout опускайте `-o` (или `-`).
- Сразу после `up`/`put` перечисление через search может быть пустым несколько секунд.
  Это лаг поискового индекса, а не потерянная запись.
- Репа, набитая внешним публикатором, может быть видна search-API лишь частично: часть поддеревьев проиндексирована, часть нет.
  Когда листинг выглядит коротким, проверяйте наличие по имени через `head`/`get`; `service assets` ходит в тот же search и видит тот же пробел.
- `/-/` в путях - сегмент npm-протокола (dist-tags, тарболы), не часть протокола nxr.
- В raw-репе лежат произвольные чужие файлы: пробелы, скобки, юникод.
  `ls`/`down`/`diff` их показывают и скачивают.
  Алфавит `[A-Za-z0-9._-]` ограничивает только то, что пишет сам nxr.
- npm-репы: обзор - через `nxr service assets`.
  Скачивание пакетов - npm-клиентом (`npm view`/`npm pack --registry ...`).

## Дальше

- Полная поверхность CLI: [reference/cli.md](../reference/cli.md).
- Таксономия ошибок и все подсказки: [reference/errors.md](../reference/errors.md).
- Хранилище и правила протокола: [reference/protocol.md](../reference/protocol.md).
