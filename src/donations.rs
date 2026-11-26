//! Окно «Поддержать»: ссылки и тексты из репозитория DeMoZ/donations (файл `usbmemorychecker.json`, формат schema 1),
//! как в ADB Toolkit, 360Cleaner, EnhanceCapCut, LocalAIBar и UniversalKM. Ссылки меняются там без нового релиза программы.
//!
//! Порядок: raw.githubusercontent.com, зеркало jsDelivr, последняя сохранённая копия, копия внутри программы.

use serde_json::Value;

use crate::net;

pub const FILE: &str = "usbmemorychecker.json";
const BUILT_IN: &str = include_str!("../assets/donations.json");

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Method {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Info {
    pub message: Option<String>,
    pub methods: Vec<Method>,
}

pub fn sources() -> [String; 2] {
    [
        format!("https://raw.githubusercontent.com/DeMoZ/donations/main/{FILE}"),
        format!("https://cdn.jsdelivr.net/gh/DeMoZ/donations@main/{FILE}"),
    ]
}

/// Текст на языке интерфейса: точный тег, базовый язык («pt-BR» → «pt»), затем английский.
/// Строка вместо объекта считается английским текстом.
fn text(v: Option<&Value>, languages: &[&str]) -> Option<String> {
    let v = v?;
    if let Some(s) = v.as_str() {
        return (!s.is_empty()).then(|| s.to_string());
    }
    let obj = v.as_object()?;
    let by_tag: Vec<(String, &str)> = obj
        .iter()
        .filter_map(|(k, v)| v.as_str().filter(|s| !s.is_empty()).map(|s| (k.replace('_', "-").to_lowercase(), s)))
        .collect();
    let find = |tag: &str| by_tag.iter().find(|(k, _)| k == tag).map(|(_, s)| s.to_string());
    for lang in languages {
        let tag = lang.replace('_', "-").to_lowercase();
        if let Some(s) = find(&tag) {
            return Some(s);
        }
        if let Some(s) = tag.split('-').next().and_then(find) {
            return Some(s);
        }
    }
    find("en")
}

/// Разбор нестрогий: неизвестные поля пропускаются, способ без названия или без ссылки https:// отбрасывается.
/// None — не JSON или нет ни одного подходящего способа.
pub fn parse(data: &[u8], lang: &str) -> Option<Info> {
    let languages = [lang];
    let v: Value = serde_json::from_slice(data).ok()?;
    let methods: Vec<Method> = v
        .get("methods")?
        .as_array()?
        .iter()
        .filter_map(|m| {
            let url = m.get("url")?.as_str()?.trim().to_string();
            if !net::is_https(&url) {
                return None;
            }
            Some(Method {
                id: m.get("id").and_then(Value::as_str).unwrap_or(&url).to_string(),
                title: text(m.get("title"), &languages)?,
                description: text(m.get("description"), &languages),
                url,
            })
        })
        .collect();
    if methods.is_empty() {
        return None;
    }
    Some(Info { message: text(v.get("message"), &languages), methods })
}

fn cache_file() -> std::path::PathBuf {
    net::cache_dir().join("donations.json")
}

/// Файл, который показать сразу: последняя загруженная копия или копия внутри программы.
/// Хранится целиком, а не разобранным: язык можно сменить, пока программа открыта.
pub fn cached() -> Vec<u8> {
    std::fs::read(cache_file())
        .ok()
        .filter(|d| parse(d, "en").is_some())
        .unwrap_or_else(|| BUILT_IN.as_bytes().to_vec())
}

/// Разбор на языке интерфейса; негодный файл заменяется копией внутри программы.
pub fn info(data: &[u8], lang: &str) -> Info {
    parse(data, lang).unwrap_or_else(|| parse(BUILT_IN.as_bytes(), lang).expect("built-in"))
}

/// Загружает свежий файл и сохраняет в кэш; None — ни один источник не ответил подходящим файлом.
pub fn download() -> Option<Vec<u8>> {
    for url in sources() {
        let Ok(data) = net::get(&url, 10) else { continue };
        if parse(&data, "en").is_none() {
            continue;
        }
        let file = cache_file();
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, &data);
        return Some(data);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_file_is_valid() {
        for lang in crate::i18n::LANGUAGES {
            let info = parse(BUILT_IN.as_bytes(), lang).unwrap();
            assert!(info.message.unwrap().contains("UsbMemoryChecker"));
        }
        let info = parse(BUILT_IN.as_bytes(), "ru").unwrap();
        assert!(info.methods[0].title.contains("Карта"));
        // в копии внутри программы есть тексты на всех языках интерфейса
        let v: Value = serde_json::from_str(BUILT_IN).unwrap();
        for lang in crate::i18n::LANGUAGES {
            assert!(v["message"].get(lang).is_some(), "message: no {lang}");
            for m in v["methods"].as_array().unwrap() {
                assert!(
                    m["title"].get(lang).is_some() && m["description"].get(lang).is_some(),
                    "{}: no {lang}",
                    m["id"]
                );
            }
        }
        assert!(info.methods.iter().all(|m| m.url.starts_with("https://")));
    }

    #[test]
    fn languages_and_unsafe_links() {
        let json = r#"{"schema":1,"message":"Hello","methods":[
            {"id":"a","title":{"en":"Card","ru":"Карта"},"url":"https://pay.example/a","extra":1},
            {"id":"b","title":{"en":"Bad"},"url":"http://pay.example/b"},
            {"id":"c","title":{"en":"Js"},"url":"javascript:alert(1)"},
            {"id":"d","title":{"pt-BR":"x","en":"Only en"},"description":{"en":"desc"},"url":"https://pay.example/d"},
            {"id":"e","url":"https://pay.example/e"}
        ]}"#;
        let info = parse(json.as_bytes(), "ru").unwrap();
        assert_eq!(info.message.as_deref(), Some("Hello"));
        assert_eq!(info.methods.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["a", "d"]);
        assert_eq!(info.methods[0].title, "Карта");
        assert_eq!(info.methods[1].title, "Only en");
        assert_eq!(info.methods[1].description.as_deref(), Some("desc"));
        assert!(parse(br#"{"methods":[{"title":"x","url":"http://a"}]}"#, "en").is_none());
        assert!(parse(b"not json", "en").is_none());
    }

    #[test]
    fn base_language() {
        let v: Value = serde_json::from_str(r#"{"en":"E","pt":"P"}"#).unwrap();
        assert_eq!(text(Some(&v), &["pt-BR"]).as_deref(), Some("P"));
        assert_eq!(text(Some(&v), &["de"]).as_deref(), Some("E"));
    }
}
