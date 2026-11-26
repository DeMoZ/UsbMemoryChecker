# UsbMemoryCheker

**Check USB flash drives and memory cards: real read/write speed, real capacity, and repair of fake drives.**

macOS (Apple Silicon) and Windows · English, Русский, Español, Deutsch, 中文 · free

[Русская версия](README.ru.md)

![Real capacity check: a fake "2 TB" drive that holds only 31.9 GB](docs/screenshot-capacity.png)

## What it does

- **Speed test.** Writes and reads 100 MB, 1 GB or 5 GB past the system cache, so you see the speed of the drive
  and not of your computer's memory. Shows average, maximum and minimum write and read speed, a live chart and a
  table of all runs. Every block read back is checked, so a drive that corrupts data is caught too.
- **Real capacity.** Cheap "1 TB / 2 TB" flash drives and microSD cards often hold only 8–64 GB: the controller
  lies about the size, and everything written past the real size is silently lost. Two ways to find the truth:
  - **Quick check** — about a minute for any size, even 2 TB. Tests ~1000 points across the whole device directly
    (like `f3probe`), finds the exact boundary to 1 MB. Asks for administrator rights. Everything it overwrites is
    read first and written back at the end, so your files survive.
  - **Full check** — fills all free space with test files and reads them back (like H2testw). No rights needed,
    your files stay, but it takes as long as writing the whole drive.
- **Repair.** Rewrites the partition table so the drive has a single partition of its *real* size (exFAT or
  FAT32), with the fake part left unused. After that the drive honestly shows, say, 31.9 GB and stops losing files.
- **Shows free and total space** of the volume, the size the drive declares, and the verified real size.
- **Safe by design.** Only removable and external drives are listed; internal and system disks never appear. The
  elevated part checks the drive again (same id, same size) right before touching it.
- **Updates itself** from GitHub releases (checked once a day, can be turned off) and has a **Support** button.

![Speed test with the live chart and results](docs/screenshot-speed.png)

*The screenshots show example drives.*

## Install

Download the latest release from [Releases](https://github.com/DeMoZ/UsbMemoryCheker/releases/latest):

- **macOS (Apple Silicon):** `UsbMemoryCheker-macOS-v<version>.dmg` — open it and drag the app to Applications.
  The app is not notarized: on the first start right-click it → **Open**, or run
  `xattr -dr com.apple.quarantine /Applications/UsbMemoryCheker.app`.
- **Windows 10/11 (x64):** `UsbMemoryCheker-<version>-windows-x64.zip` — unpack and run `UsbMemoryCheker.exe`.
  The WebView2 runtime is built into Windows 11 and current Windows 10.

## How to use

1. Insert the flash drive or card. It appears in the list on the left by itself.
2. **Speed** tab: choose 100 MB, 1 GB or 5 GB and press **Start test**.
3. **Real capacity** tab: choose **Quick check** (recommended) or **Full check** and press **Check capacity**.
   For the quick check macOS asks for your password, Windows asks for permission.
4. If the drive turns out fake, press **Repair to …**: choose exFAT or FAT32 and a name, confirm, and the drive gets
   a partition of its real size. **This erases the drive** — copy your files off it first.

## How the quick check works

A fake drive maps the addresses past its real size either back onto the real part (usually by ignoring the high
address bits) or nowhere at all (writes are lost, reads give zeros or garbage); some also keep recent writes in a
small cache. The probe writes a unique pattern, computed from the address, to points on a power-of-two grid over the
whole device plus their likely wrap-around targets, from the highest address down; then writes 16 MB at the start to
flush any cache, and reads every point back. A point whose data came back different is not real. Between the last
good and the first bad point it repeats with a finer grid until the boundary is known to 1 MB. Before every write the
old content is read, and at the end everything is written back in reverse order.

Very unusual controllers that wrap by a non-power-of-two size may slip past the quick check — the full check catches
everything, because it really writes and reads back every byte of free space.

## Build from source

Needs [Rust](https://rustup.rs) (edition 2024).

```bash
cargo run --release           # run the app
cargo test                    # tests (the probe is tested against simulated fake drives)
scripts/build-macos.sh        # macOS: dist/UsbMemoryCheker.app, .dmg and the update .zip
```

Command line without the window (JSON output):

```bash
usbmemorycheker --cli list
usbmemorycheker --cli speed disk4 1000
usbmemorycheker --cli probe disk4
usbmemorycheker --cli full disk4
```

Built with [tao](https://github.com/tauri-apps/tao) + [wry](https://github.com/tauri-apps/wry) (system WebView),
no runtime besides the OS.

## Support

UsbMemoryCheker is free. If it saved you from a fake drive or lost files, the **Support** button in the app shows
ways to support its development.

## License

[MIT](LICENSE)
