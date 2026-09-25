# nexus-raw

Надёжная доставка артефактов через плохой канал: claim, sha-маркеры, симметричный дифф, resume.
Один статический бинарь `nxr` для всех и Rust-библиотека `nexus-raw-core`; ноль серверных компонент.
Говорит на раскладке claim_version 1 из [SPEC.md](SPEC.md) (harbor, panda-sdk §6) и заменяет десяток самодельных curl-скриптов: одна политика auth, одна политика ретраев, TLS проверяется по умолчанию, креды никогда не в argv.

## Установка

```bash
cargo install --path crates/nexus-raw    # бинарь nxr
```

## Настройка

В конфиге только URL, `~/.config/nxr/config.toml` (переопределяется `$NXR_CONFIG`):

```toml
default_profile = "panda"

[panda]
url = "https://nexus.example/repository/koala-raw/panda/"

[dev]
url = "http://localhost:8080/raw/dev/"
tls_insecure = true
```

Креды берутся из env по порядку: `NXR_<PROFILE>_AUTH` (base64 `user:pass`), `NXR_AUTH`, `NXR_USERNAME` + `NXR_PASSWORD`, `OPENLAB_USERNAME` + `OPENLAB_PASSWORD`.
Пароли в TOML-конфиге отвергаются.

## Быстрый старт

```bash
# продюсер
nxr up --profile panda --dir dist/1.14.0          # обрыв не страшен: повтори ту же команду
nxr point latest 1.14.0 --if-newer --profile panda

# потребитель
nxr down --profile panda --pointer latest --dir third-party/panda/prebuilt

# проверить локальную сборку без сети
nxr verify --dir dist/1.14.0

# план против сервера, ничего не пишется
nxr diff --profile panda --dir dist/1.14.0
nxr up --profile panda --dir dist/1.14.0 --dry-run
```

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
| [SPEC.md](SPEC.md) | канонический протокол: раскладки, маркеры, таблица диффа, ошибки |
| `crates/nexus-raw-core/` | Rust-библиотека: фасад `Nxr`, типизированные ошибки, поток событий |
| `crates/nexus-raw/` | бинарь `nxr` |
| `mock/mock-nexus/` | мок-сервер с таблицей отказов (`just mock atomic`) |

## Разработка

```bash
just check        # fmt + clippy + prek + тесты
just test
just mock atomic --port 8080
just nxr -- up --base http://127.0.0.1:8080/ --dir dist/1.14.0
```

Полная инструкция: [AGENTS.md](AGENTS.md).
Английский readme: [README.md](README.md).
