# nexus-raw

Клиент общего назначения для raw-репозитория Sonatype Nexus.
`nxr` закачивает и скачивает каталоги версий, описанные `claim.json`, проверяет sha256-маркеры-сиблинги и докачивает прерванные передачи через дифф локального и удалённого состояния.
Крейт `nexus-raw-core` даёт те же операции как Rust-библиотека.
Ноль серверных компонент.

Возможности:

- `up`, `down`, локальный `verify`, план передачи `diff`, `ls`, обновление указателей (`point`).
- Завершённость — байты плюс маркер `<name>.sha256` в формате `sha256sum -c`.
- Параллельные передачи (8 воркеров по умолчанию), ретраи с backoff, детект остановки.
- TLS проверяется по умолчанию; креды только из env.
- `--json` NDJSON-вывод; стабильные exit-коды 0/1/2/3.

## Установка

```bash
cargo install --path crates/nexus-raw    # бинарь nxr
```

## Настройка

В конфиге только URL, `~/.config/nxr/config.toml` (переопределяется `$NXR_CONFIG`):

```toml
default_profile = "release"

[release]
url = "https://nexus.example.com/repository/raw-main/"

[dev]
url = "http://localhost:8080/repository/raw-dev/"
tls_insecure = true
```

Креды берутся из env по порядку:

1. `NXR_<PROFILE>_AUTH` — base64 `user:pass`, имя профиля в верхнем регистре, `-` превращается в `_`.
2. `NXR_AUTH` — то же, для всех профилей сразу.
3. `NXR_USERNAME` + `NXR_PASSWORD`.

Пароли в TOML-конфиге отвергаются.
`--base <url>` вместо `--profile` работает вовсе без конфига.

## Быстрый старт

```bash
# публикация каталога версии с claim.json + артефактами + .sha256-маркерами
nxr up --profile release --dir dist/1.4.0
nxr point latest 1.4.0 --if-newer --profile release

# скачивание версии через указатель
nxr down --profile release --pointer latest --dir vendor/prebuilt

# проверить локальную сборку без сети
nxr verify --dir dist/1.4.0

# план против сервера, ничего не пишется
nxr diff --profile release --dir dist/1.4.0
nxr up --profile release --dir dist/1.4.0 --dry-run
```

Прерванная передача докачивается повтором той же команды.

## Команды

| Команда | Делает |
|:--------|:-------|
| `nxr up --dir <dir> [--names <файл>] [--dry-run]` | claim (drift-проверка) → дифф → PUT байтов + маркеров |
| `nxr down --dir <dir> (--version <v> \| --pointer <latest\|nightly>) [--only <имя>]...` | GET claim → дифф → tmp+hash → rename → маркер; докачка всегда включена |
| `nxr verify --dir <dir>` | локально байты + маркер + digest, без сети |
| `nxr diff --dir <dir>` | план против сервера, симметричен up и down |
| `nxr ls [--version <v>]` | состояния имён на сервере, либо список версий (REST search, experimental) |
| `nxr point <latest\|nightly> <version> [--if-newer]` | атомарный PUT указателя; `--if-newer` — только вперёд |

Exit-коды: 0 ок, 1 данные (mismatch, incomplete, claim drift, missing), 2 misuse, 3 транспорт (сеть, auth, TLS, 5xx).
`--json` отдаёт NDJSON-события тех же структур, что ядро; piping в `jq` работает.

## Устройство

| Путь | Для |
|:-----|:----|
| `crates/nexus-raw-core/` | Rust-библиотека: фасад `Nxr`, типизированные ошибки, поток событий |
| `crates/nexus-raw/` | бинарь `nxr`: только флаги и рендер, без протокольной логики |
| `mock/mock-nexus/` | мок-сервер с таблицей отказов, конформанс-фикстура |
| `docs/` + `mkdocs.yml` | сайт документации (zensical); `just docs` поднимает его |
| `node/` | будущая npm-упаковка; пока не существует |

## Разработка

```bash
just check        # fmt + clippy + prek + тесты
just test         # юнит- и конформанс-наборы
just mock atomic --port 8080
just nxr -- up --base http://127.0.0.1:8080/ --dir dist/1.4.0
```

Пользовательская документация живёт в [docs/](docs/index.md) и собирается в сайт:

```bash
just docs     # http://localhost:8000, live reload
```

Английский readme: [README.md](README.md).
