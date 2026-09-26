# nexus-raw

<p align="center"><img src="docs/assets/img/hero-terminal.png" alt="Реальная сессия nxr: публикация, канал, потребление, верификация" width="720"></p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.85%2B-dea584?style=flat-square" alt="rust 1.85+">
  <img src="https://img.shields.io/badge/protocol-claim__version%201-3f7e6e?style=flat-square" alt="протокол claim_version 1">
  <img src="https://img.shields.io/badge/config%20files-none-2ea44f?style=flat-square" alt="без конфиг-файлов">
  <img src="https://img.shields.io/badge/exits-0%20%7C%201%20%7C%202%20%7C%203-blue?style=flat-square" alt="коды выхода 0/1/2/3">
</p>

`nxr` — это curl для raw-репозитория Sonatype Nexus.
Примитивы уровня HTTP с ретраями, stall-детектом и TLS по умолчанию, поверх — проверенные переносы каталогов, ещё выше — канальные рефы и манифесты.
Каждый вызов самодостаточен: URL в argv, креды через `-u` или окружение.
Никакого конфиг-файла, никаких профилей.
Крейт `nexus-raw-core` даёт те же операции как Rust-библиотека.
Ноль серверных компонент.

Возможности:

- `get`, `put`, `head`, `sha` — примитивы уровня curl, digest считается на лету.
- `up`, `down` — перенос каталогов с симметричным диффом, параллельными воркерами и Range-resume (`.part`-файлы, 206).
- Маркеры-сиблинги в формате `sha256sum -c`: `up` пишет и генерирует их по умолчанию, `--no-sha` — осознанный отказ.
- `down` перечисление берёт явно: `manifest.json` в каталоге версии, `--manifest`, повторяемый `--name` или best-effort `--ls`.
- `channel get|set` — токен-файлы с любым именем, с guard'ом `--if-forward` (dotted-numeric).
- `verify` — офлайн-проверка байтов, маркеров и digest'ов.
- `doctor` — креды, TLS, достижимость.
- TLS проверяется по умолчанию, `--tls-insecure` — единственный выключатель.
- `--json`: один JSON-объект для простых команд, NDJSON-события для переносов.
- Exit-коды 0/1/2/3, каждая ошибка печатает `hint:` на stderr.

## Установка

```bash
cargo install --path crates/nexus-raw    # бинарь nxr
```

## Креды

`-u user:pass` главный, затем окружение: `NXR_AUTH` (base64 `user:pass`) или `NXR_USERNAME` + `NXR_PASSWORD` (только вместе).
Это весь список — алиасы и URL по умолчанию живут в вашем shell или CI, а не в конфиге.

```bash
printf 'ci-bot:%s' "$TOKEN" | base64
export NXR_AUTH="Y2ktYm90OnRva2Vu"
```

`-u` виден в `ps`, env-пути — выбор для CI.
`nxr doctor` сообщает, какой источник сработал, не печатая значений.

## Быстрый старт

```bash
BASE=https://nexus.example.com/repository/raw-main

# публикация каталога версии: маркеры генерируются, проверяются и загружаются сами
nxr up dist/1.4.0/ "$BASE/1.4.0/"

# назвать — канал это токен-файл с любым именем
nxr channel set "$BASE/latest" 1.4.0 --if-forward

# забрать в другое место; manifest.json в каталоге версии задаёт перечисление
nxr down "$BASE/1.4.0/" vendor/prebuilt --continue

# без манифеста — назвать нужное
nxr down "$BASE/1.4.0/" vendor/prebuilt --name app.zip

# проверить локальный каталог без сети; план — до переноса
nxr verify vendor/prebuilt
nxr up --dry-run dist/1.5.0/ "$BASE/1.5.0/"
```

Прерванная передача докачивается повтором той же команды: `up` пропускает завершённое, `down --continue` докачивает с part-файлов через `Range: bytes=N-`.

## Команды

| Команда | Делает |
|:--------|:-------|
| `nxr get <URL> [-o FILE] [--continue]` | GET в файл (через `.part`, докачка через Range) или stdout |
| `nxr put <URL> -f FILE [--sha]` | PUT байтов — `--sha` ещё и PUT `.sha256`-сиблинга |
| `nxr head <URL>` | статус, размер, content type |
| `nxr sha <FILE\|URL>` | потоковый sha256 файла или удалённого объекта |
| `nxr up <SRC_DIR> <DST_URL> [--manifest F] [--no-sha] [--dry-run]` | скан → дифф → PUT байтов + маркеров параллельно |
| `nxr down <SRC_URL> <DST_DIR> [--manifest F\|URL\|-] [--name N]... [--ls] [--continue]` | перечисление → дифф → поток+hash → rename + локальный маркер |
| `nxr ls <URL> [--assets]` | листинг версий или объектов через search API (experimental) |
| `nxr channel get <URL>` | токен канала (`unset`, если пусто) |
| `nxr channel set <URL> <TOKEN> [--if-forward]` | запись токена, `--if-forward` — только вперёд в dotted-numeric порядке |
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
