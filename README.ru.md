<div align="center">

<img src="docs/assets/img/hero-terminal.png" alt="Записанная сессия nxr: публикация каталога версии против мок-сервера" width="760">

# nexus-raw

`nxr` — это curl для raw-репозитория Sonatype Nexus.

[![CI](https://github.com/Lipen/nexus-raw/actions/workflows/ci.yml/badge.svg)](https://github.com/Lipen/nexus-raw/actions/workflows/ci.yml)
[![Docs](https://github.com/Lipen/nexus-raw/actions/workflows/docs.yml/badge.svg)](https://github.com/Lipen/nexus-raw/actions/workflows/docs.yml)

[Документация](https://lipen.github.io/nexus-raw/) · [Установка](#установка) · [Быстрый старт](#быстрый-старт) · [Команды](#команды) · [README in English](README.md)

</div>

## Установка

Требуется Rust 1.85 или новее.
На crates.io и npm ничего не опубликовано, готовых бинарников нет, поэтому остаются два способа ниже.

Из git, без клона (ставится то, что на ветке по умолчанию, а не релиз):

```bash
cargo install --git https://github.com/Lipen/nexus-raw nexus-raw --locked
```

Из checkout:

```bash
cargo install --path crates/nexus-raw --locked
```

`nxr --version` печатает `nxr 0.1.0`.

## Быстрый старт

Опубликовать версию, назвать её каналом, забрать назад и проверить офлайн:

```bash
BASE=https://nexus.example.com/repository/raw-main

# down отказывает версии без manifest.json; в нём перечислены имена, которые могут забирать потребители
printf '{"artifacts": ["app-1.4.0.zip"]}\n' > dist/1.4.0/manifest.json
nxr up dist/1.4.0/ "$BASE/1.4.0/"
nxr channel set "$BASE/latest" 1.4.0 --if-forward
V=$(nxr channel get "$BASE/latest")
nxr down "$BASE/$V/" vendor/prebuilt
nxr verify vendor/prebuilt
```

Прерванная передача доезжает повтором той же команды: завершённое пропускается.

## Команды

| Команда | Делает |
|:--------|:-------|
| `nxr get <URL> [-o FILE] [--continue]` | GET в файл (через `.part`, докачка через Range) или stdout |
| `nxr put <URL> -f FILE [--sha]` | PUT байтов: `--sha` ещё и PUT `.sha256`-сиблинга |
| `nxr head <URL>` | статус, размер, content type |
| `nxr sha <FILE\|URL>` | потоковый sha256 файла или удалённого объекта |
| `nxr up <SRC_DIR> <DST_URL> [--manifest F] [--no-sha] [--dry-run] [--claim-first NAME]` | скан → дифф → PUT байтов + маркеров параллельными воркерами |
| `nxr down <SRC_URL> <DST_DIR> [--manifest F\|URL\|-] [--name N]... [--ls] [--fresh]` | перечисление → дифф → поток+hash → rename + локальный маркер |
| `nxr ls <URL> [--assets]` | листинг версий или объектов через search API (experimental) |
| `nxr channel get <URL>` | токен канала (`unset`, если пусто) |
| `nxr channel set <URL> <TOKEN> [--if-forward]` | запись токена; `--if-forward` допускает сдвиг только вперёд в dotted-numeric порядке |
| `nxr verify <DIR> [--manifest F\|-]` | локально байты + маркер + digest, без сети |
| `nxr doctor [URL]` | креды, TLS, настройки, достижимость |

`down` берёт перечисление явно: `manifest.json` в каталоге версии, `--manifest`, повторяемый `--name` или best-effort `--ls`; без ничего — отказ.

Exit-коды: 0 ок, 1 данные, 2 misuse, 3 транспорт.
Каждая ошибка печатает `hint:`.
`--json` печатает по одному JSON-объекту на строку в stdout.

## Креды

Источники в порядке проверки: `-u user:pass`, затем `NXR_AUTH` (base64 от `user:pass`), затем `NXR_USERNAME` + `NXR_PASSWORD`.

```bash
export NXR_USERNAME="my-login"
export NXR_PASSWORD="my-password"

# или одно значение для CI: base64 от "my-login:my-password"
export NXR_AUTH="$(printf '%s:%s' 'my-login' 'my-password' | base64)"
```

`nxr doctor [URL]` называет сработавший источник, не печатая значений.

## Документация

Гайды и справочник: <https://lipen.github.io/nexus-raw/>.
Исходники в [docs/](docs/); локально сайт поднимается `just docs`.

## Участие

Устройство репозитория и правила изменений: [AGENTS.md](AGENTS.md).
`just check` — полный гейт (fmt, clippy, prek, тесты); `just demo` — записанная сессия против локального мок-сервера.
