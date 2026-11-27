//! Сеть через системный `curl` (есть в macOS и в Windows 10+): без своей реализации TLS в программе.
//! Нужна только для донатов и обновлений; проверка носителей работает без сети.

use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Команда без окна консоли в Windows.
pub fn command(program: &str) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Runs a command like `Command::output`, but kills it and returns `ErrorKind::TimedOut` when it runs longer
/// than `limit`. Stdout and stderr are drained in two threads, so a chatty child cannot block on a full pipe.
pub fn output_timeout(cmd: &mut Command, limit: Duration) -> io::Result<Output> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    }
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        }
        if start.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            // the reader threads end by themselves when the pipes close; a grandchild may keep them open, so no join
            return Err(io::Error::new(io::ErrorKind::TimedOut, format!("the command did not finish in {limit:?}")));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(Output { status, stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() })
}

fn curl() -> Command {
    let mut c = command(if cfg!(windows) { "curl.exe" } else { "/usr/bin/curl" });
    // GitHub API без User-Agent отвечает отказом.
    c.args(["-fsSL", "--proto", "=https", "-A", "UsbMemoryChecker"]).stdin(Stdio::null());
    c
}

/// Скачивает небольшой ответ (JSON) целиком.
pub fn get(url: &str, timeout_secs: u32) -> Result<Vec<u8>, String> {
    let out = curl()
        .args(["--max-time", &timeout_secs.to_string(), url])
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { format!("curl exited with code {:?}", out.status.code()) } else { err })
    }
}

/// Скачивает файл на диск.
pub fn download(url: &str, to: &Path) -> Result<(), String> {
    let out = curl()
        .args(["--max-time", "900", "-o"])
        .arg(to)
        .arg(url)
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// Открывает страницу в браузере. Только https://.
pub fn open_url(url: &str) -> std::io::Result<()> {
    if !is_https(url) {
        return Err(std::io::Error::other("only https:// links are allowed"));
    }
    if cfg!(windows) {
        command("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).spawn().map(|_| ())
    } else if cfg!(target_os = "macos") {
        command("/usr/bin/open").arg(url).spawn().map(|_| ())
    } else {
        command("xdg-open").arg(url).spawn().map(|_| ())
    }
}

pub fn is_https(url: &str) -> bool {
    url.len() > "https://".len()
        && url[..8].eq_ignore_ascii_case("https://")
        && !url[8..].starts_with('/')
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn home_dir() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default()
}

fn base_dir(var_win: &str, mac: &str, linux: &str) -> PathBuf {
    let home = home_dir();
    let dir = if cfg!(windows) {
        std::env::var_os(var_win).map(PathBuf::from).unwrap_or_else(|| home.join("AppData"))
    } else if cfg!(target_os = "macos") {
        home.join(mac)
    } else {
        home.join(linux)
    };
    dir.join("UsbMemoryChecker")
}

/// Настройки: `~/Library/Application Support/UsbMemoryChecker`, `%APPDATA%\UsbMemoryChecker`.
pub fn config_dir() -> PathBuf {
    base_dir("APPDATA", "Library/Application Support", ".config")
}

/// Кэш: `~/Library/Caches/UsbMemoryChecker`, `%LOCALAPPDATA%\UsbMemoryChecker`.
pub fn cache_dir() -> PathBuf {
    base_dir("LOCALAPPDATA", "Library/Caches", ".cache")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_links() {
        assert!(is_https("https://pay.example/a"));
        assert!(is_https("HTTPS://pay.example/a"));
        assert!(!is_https("http://pay.example/a"));
        assert!(!is_https("https:///etc/passwd"));
        assert!(!is_https("javascript:alert(1)"));
        assert!(!is_https("https://a b"));
        assert!(!is_https("https://"));
    }

    #[cfg(unix)]
    #[test]
    fn output_timeout_returns_output() {
        let out = output_timeout(Command::new("/bin/echo").arg("hi"), Duration::from_secs(5)).unwrap();
        assert!(out.status.success());
        assert_eq!(out.stdout, b"hi\n");
    }

    #[cfg(unix)]
    #[test]
    fn output_timeout_kills_a_slow_child() {
        let start = Instant::now();
        let err = output_timeout(Command::new("/bin/sleep").arg("5"), Duration::from_millis(200)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        // the point: it returns long before `sleep 5` would (the bound allows a loaded machine)
        assert!(start.elapsed() < Duration::from_millis(4500));
    }
}
