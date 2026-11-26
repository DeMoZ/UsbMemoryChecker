//! Interface language. All window texts are in `ui/i18n.js` (the `I18N` dictionary); here only the choice of
//! the language and the text of the system password prompt, which Rust shows.

/// Supported languages in menu order. The first one is the fallback.
pub const LANGUAGES: [&str; 5] = ["en", "ru", "es", "de", "zh"];

/// Language from a tag like `pt-BR`, `zh-Hans-CN`, `ru_RU`; None — not supported.
pub fn supported(tag: &str) -> Option<&'static str> {
    let base = tag.split(['-', '_', '.', '@']).next()?.to_ascii_lowercase();
    LANGUAGES.iter().copied().find(|l| *l == base)
}

/// First supported language from the system preferences, otherwise English.
pub fn system_language() -> &'static str {
    sys_locale::get_locales().find_map(|l| supported(&l)).unwrap_or("en")
}

/// Language from the setting: empty or unknown code — the system language.
pub fn resolve(setting: &str) -> &'static str {
    supported(setting).unwrap_or_else(system_language)
}

/// Text of the macOS administrator password window.
pub fn admin_prompt(lang: &str) -> &'static str {
    match lang {
        "ru" => "UsbMemoryChecker нужен доступ администратора, чтобы напрямую проверить или переразметить носитель.",
        "es" => {
            "UsbMemoryChecker necesita permisos de administrador para comprobar o reparticionar la unidad directamente."
        }
        "de" => {
            "UsbMemoryChecker benötigt Administratorrechte, um das Laufwerk direkt zu prüfen oder neu zu partitionieren."
        }
        "zh" => "UsbMemoryChecker 需要管理员权限才能直接检测或重新分区该存储设备。",
        _ => "UsbMemoryChecker needs administrator rights to test or repartition the drive directly.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_tags() {
        assert_eq!(supported("zh-Hans-CN"), Some("zh"));
        assert_eq!(supported("ru_RU.UTF-8"), Some("ru"));
        assert_eq!(supported("EN"), Some("en"));
        assert_eq!(supported("ko-KR"), None);
        assert_eq!(supported(""), None);
        assert_eq!(resolve("de"), "de");
        assert!(LANGUAGES.contains(&resolve("xx")));
        for l in LANGUAGES {
            assert!(!admin_prompt(l).is_empty());
        }
    }
}
