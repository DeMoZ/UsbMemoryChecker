# UsbMemoryChecker

Проверка USB-флешек и карт памяти: скорость чтения/записи (100 МБ / 1 ГБ / 5 ГБ, средняя / макс / мин), реальный
объём (поиск поддельных «2 ТБ» флешек) и исправление — раздел на реальный объём. macOS (Apple Silicon) и Windows.
Отвечай пользователю по-русски. Описание для пользователя — [README.md](README.md) (английский) и
[README.ru.md](README.ru.md): меняешь одно — поправь второе. Комментарии в коде — по-английски (`update.rs`, `donations.rs`, `net.rs` взяты из 360Cleaner как есть, по-русски).

## Устройство

Rust (edition 2024) + tao 0.37 / wry 0.57 (системный WebView: WKWebView на Mac, WebView2 в Windows). Интерфейс —
`src/ui/index.html` (HTML/CSS/JS, `include_str!`), тексты — `src/ui/i18n.js` (Rust подставляет вместо `/*I18N*/`).

- `main.rs` — окно, IPC, состояние (`State` под `Mutex`), одна задача за раз (`start_task`), наблюдатель дисков
  (`disk_watcher`, раз в 2 с / 4 с в Windows), донаты и обновления. `--helper <папка>` и `--cli …` — без окна.
- `disks.rs` — список съёмных носителей: macOS — `diskutil list/info -plist` (только не Internal / Removable /
  SD, без загрузочного), Windows — PowerShell `Get-Disk` (только USB/SD/MMC/1394, без IsBoot/IsSystem).
  `UMC_SHOW_VIRTUAL=1` показывает и образы дисков / RAM-диски (для проверки без флешки).
- `dio.rs` — прямой ввод-вывод мимо кэша: `F_NOCACHE` + `F_FULLFSYNC` на Mac, `FILE_FLAG_NO_BUFFERING |
  WRITE_THROUGH` в Windows (буферы, смещения и размеры кратны 4 КиБ — `AlignedBuf`).
- `pattern.rs` — тестовые данные: каждый блок 4 КиБ зависит от адреса и соли запуска.
- `bench.rs` — тест скорости (файл `.UsbMemoryChecker-speed.bin`, окна по 16 МиБ для макс/мин).
- `fullcheck.rs` — полная проверка без стирания: папка `UsbMemoryChecker-test` из файлов по 1 ГиБ на всё свободное
  место, потом чтение и сверка.
- `probe.rs` — быстрая проверка (как f3probe) поверх `trait Device`; тесты на имитациях подделок (обёртка по 2^k,
  потерянные записи, кэш, сбои в середине, отмена).
- `rawdev.rs` — весь носитель: `/dev/rdiskN` / `\\.\PhysicalDriveN`, размер из ioctl; в Windows тома на время
  проверки блокируются и отключаются (`VolumeLocks`).
- `helper.rs` — работа с правами администратора во второй копии программы: macOS — `osascript … with administrator
  privileges`, Windows — `ShellExecuteExW` `runas`. Обмен через файлы в папке задания (`job.json`,
  `progress.json`, `result.json`, `cancel`). Исправление: `diskutil partitionDisk … MBR <fs> <имя> <байты> FREE gap R`
  (или `eraseDisk` на весь объём) / `diskpart` (`clean`, `convert mbr`, `create partition primary size=…`).
  `UMC_NO_ELEVATE=1` — без запроса прав (для RAM-диска, который принадлежит пользователю).
- `update.rs`, `donations.rs`, `net.rs`, `settings.rs` — как в 360Cleaner / AdbToolsGUI.

## Важно

- **WebView только в главном потоке**: из потоков — `Ui::send` → `UserEvent::Js`.
- **Быстрая проверка ничего не стирает**: перед каждой записью читает старое содержимое, в конце (и при отмене /
  ошибке) пишет всё обратно в обратном порядке. Не ломай этот порядок — он восстанавливает ячейки, даже когда на
  одну ячейку приходится несколько адресов.
- Помощник с правами перед работой заново находит носитель среди съёмных и сверяет размер (`disks::find`); id
  проверяется `disks::valid_id` (в root-команды попадает только `diskN` / номер).
- Ошибки доступа (`dio::is_access_error`) прерывают проверку, а не считаются «плохими точками»: иначе занятый том
  выглядел бы поддельным.
- Размер раздела при исправлении — `helper::partition_size` (реальный − 2 МиБ, кратно МиБ); в окне та же формула
  (`repairSize`). FAT32 — только до 32 ГБ.
- Проверка на RAM-диске: `hdiutil attach -nomount ram://4194304`, `diskutil eraseDisk FAT32 UMCTEST MBR diskN`,
  затем `UMC_SHOW_VIRTUAL=1 UMC_NO_ELEVATE=1 target/release/usbmemorychecker --cli probe diskN`.
  `UMC_DEV_SCRIPT='js'` выполняет скрипт в окне через 2,5 с после запуска (для снимков экрана).
- До v0.1.0 включительно программа называлась UsbMemoryCheker (репозиторий переименован 2026-11-26). Ради
  обновления старых копий: в Windows .zip кладётся тот же .exe ещё и как `UsbMemoryCheker.exe` (старый апдейтер ищет
  его), настройки читаются из старой папки `UsbMemoryCheker`, если новой нет. На Mac старая копия обновится в
  прежнюю папку `UsbMemoryCheker.app`. Убирать это можно, когда v0.1.0 уже не встречается.
- На настоящей Windows программа не проверялась; CI собирает и гоняет там тесты и clippy.

## Проверки и релиз

- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `node scripts/check-i18n.mjs`;
  Windows с Mac: `cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings`.
- Языки: en, ru, es, de, zh (`i18n.rs` `LANGUAGES` = ключи `I18N`). Новый ключ — сразу во все 5 языков. Тексты
  доната — в `usbmemorychecker.json` репозитория DeMoZ/donations и в `assets/donations.json` (держать одинаковыми).
- Релиз: `version` в `Cargo.toml` + `docs/releases/v<версия>.md` (en + ru, их видно в окне обновления) + тег
  `v<версия>` на main. CI выкладывает .dmg, Mac .zip и Windows .zip — **Windows в этом проекте выпускается всегда**
  (так попросил Роман, 2026-11-26).
- Значок: `scripts/make-icon.sh` → `assets/`.
