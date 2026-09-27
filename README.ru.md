# nexus-raw

<p align="center"><img src="docs/assets/img/hero-terminal.png" alt="Реальная сессия nxr: публикация, канал, потребление, верификация" width="720"></p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.85%2B-dea584?style=flat-square" alt="rust 1.85+">
</p>

> `nxr` — это curl для raw-репозитория Sonatype Nexus.

Один статический бинарь: примитивы уровня curl с ретраями, stall-детектом и TLS по умолчанию, затем проверенные переносы каталогов с маркерами-сиблингами, затем канальные рефы и манифесты.
Каждый вызов самодостаточен: URL в argv, креды через `-u` или окружение, никакого конфиг-файла, никаких профилей, ноль серверных компонент.
Крейт `nexus-raw-core` даёт те же операции как Rust-библиотека.

## Установка

```bash
cargo install --path crates/nexus-raw    # бинарь nxr
```

## Быстрый старт

Два сценария покрывают модель. Опубликовать версию и назвать её, забрать и проверить офлайн:

```bash
BASE=https://nexus.example.com/repository/raw-main

# перечисляем, что можно забирать: down без этого файла в каталоге версии откажется
printf '{"artifacts": ["app-1.4.0.zip"]}\n' > dist/1.4.0/manifest.json

# публикация каталога версии: маркеры генерируются, проверяются и загружаются сами
nxr up dist/1.4.0/ "$BASE/1.4.0/"

# назвать — канал это токен-файл с любым именем
nxr channel set "$BASE/latest" 1.4.0 --if-forward

# забрать в другое место: канал называет версию, манифест перечисляет файлы
V=$(nxr channel get "$BASE/latest")
nxr down "$BASE/$V/" vendor/prebuilt
nxr verify vendor/prebuilt
```

Прерванная передача докачивается повтором той же команды: `up` пропускает завершённое, `down` докачивает с part-файлов через `Range: bytes=N-` по умолчанию, а `--fresh` начинает с нуля.
Остальная модель (явное перечисление, dry-run планы, best-effort листинги) в [таблице команд](#команды) и [документации](docs/index.md).

## Креды

Три источника в порядке приоритета: `-u user:pass`, затем `NXR_AUTH`, затем `NXR_USERNAME` + `NXR_PASSWORD`.
Это весь список: алиасы и URL по умолчанию живут в вашем shell или CI, а не в конфиге.

```bash
# простой путь: две переменные, ничего кодировать не надо
export NXR_USERNAME="my-login"
export NXR_PASSWORD="my-password"

# путь для CI: одно значение вместо двух, удобно для маскируемой переменной
export NXR_AUTH="$(printf '%s:%s' 'my-login' 'my-password' | base64)"
# base64 от "my-login:my-password": bXktbG9naW46bXktcGFzc3dvcmQ=
```

`-u` быстрее всего для разового вызова и виден в `ps`.
Какой источник сработал, назовёт `nxr doctor`, не печатая значений.

## Возможности

- `get`, `put`, `head`, `sha`: примитивы уровня curl, digest считается на лету.
- `up`, `down`: перенос каталогов с симметричным диффом, параллельными воркерами и Range-resume по умолчанию (`.part`-файлы, 206), `--fresh` начинает с нуля.
- Маркеры-сиблинги: у каждого загруженного объекта появляется `<name>.sha256` в формате `sha256sum -c`, `up` пишет и генерирует их по умолчанию, `--no-sha` означает осознанный отказ.
- `down` перечисление берёт явно: `manifest.json` в каталоге версии, `--manifest`, повторяемый `--name` или best-effort `--ls`.
- `channel get|set`: токен-файлы с любым именем, с guard'ом `--if-forward` (dotted-numeric).
- `verify`: офлайн-проверка байтов, маркеров и digest'ов.
- `doctor`: креды, TLS, достижимость.
- TLS проверяется по умолчанию, и `--tls-insecure` остаётся единственным выключателем.
- `--json`: один JSON-объект для простых команд, NDJSON-события для переносов.
- Exit-коды 0/1/2/3, каждая ошибка печатает `hint:` на stderr.

## Команды

| Команда | Делает |
|:--------|:-------|
| `nxr get <URL> [-o FILE] [--continue]` | GET в файл (через `.part`, докачка через Range) или stdout |
| `nxr put <URL> -f FILE [--sha]` | PUT байтов: `--sha` ещё и PUT `.sha256`-сиблинга |
| `nxr head <URL>` | статус, размер, content type |
| `nxr sha <FILE\|URL>` | потоковый sha256 файла или удалённого объекта |
| `nxr up <SRC_DIR> <DST_URL> [--manifest F] [--no-sha] [--dry-run] [--claim-first NAME]` | скан → дифф → PUT байтов + маркеров параллельно, `--manifest F` ограничивает прогон именами из F |
| `nxr down <SRC_URL> <DST_DIR> [--manifest F\|URL\|-] [--name N]... [--ls] [--fresh]` | перечисление → дифф → поток+hash → rename + локальный маркер |
| `nxr ls <URL> [--assets]` | листинг версий или объектов через search API (experimental) |
| `nxr channel get <URL>` | токен канала (`unset`, если пусто) |
| `nxr channel set <URL> <TOKEN> [--if-forward]` | запись токена, `--if-forward` допускает сдвиг только вперёд в dotted-numeric порядке |
| `nxr verify <DIR> [--manifest F\|-]` | локально байты + маркер + digest, без сети |
| `nxr doctor [URL]` | креды, TLS, настройки, достижимость |

Exit-коды: 0 ок, 1 данные (`mismatch`, `incomplete`, `missing`, `cannot enumerate`), 2 misuse, 3 транспорт (сеть, auth, TLS, 5xx).
Расходящийся завершённый артефакт отвергается, а не перезаписывается.

## Устройство

| Путь | Для |
|:-----|:----|
| `crates/nexus-raw-core/` | Rust-библиотека из четырёх слоёв: `transport` + `primitive`, `sync`, `layout` и фасад `Nxr` |
| `crates/nexus-raw/` | бинарь `nxr`: только флаги, рендер и exit-коды, без протокольной логики |
| `crates/mock-nexus/` | мок-сервер с таблицей отказов, конформанс-фикстура |
| `docs/` + `mkdocs.yml` | сайт документации (zensical), который поднимает `just docs` |
| `node/` | будущая npm-упаковка, которой пока не существует |

## Разработка

```bash
just check        # fmt + clippy + prek + тесты
just test         # юнит- и конформанс-наборы
just mock atomic --port 8080
just nxr -- up dist/1.4.0/ http://127.0.0.1:8080/1.4.0/
```

Пользовательская документация живёт в [docs/](docs/index.md) и собирается в сайт:

```bash
just docs     # http://localhost:8000, live reload
```

Английский readme: [README.md](README.md).
