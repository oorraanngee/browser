//! Хранилище atmo: настройки, закладки, сессия, загрузки. Всё лежит в %APPDATA%\atmo.

use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const VERSION: &str = "0.2.0";
pub type Shared = Arc<Mutex<App>>;

/// Эти ключи страница настроек может менять через API. Остальное только из самого браузера.
const SETTING_KEYS: &[&str] = &[
    "mode",
    "show_ip",
    "newtab_page",
    "restore_session",
    "keep_open",
    "remember_adapter",
    "accel_keys",
    "disabled_keys",
];

fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let d = base.join("atmo");
    let _ = fs::create_dir_all(&d);
    d
}

fn read_json(name: &str) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(dir().join(name)).ok()?).ok()
}

fn write_json(name: &str, v: &Value) {
    if let Ok(s) = serde_json::to_string_pretty(v) {
        let _ = fs::write(dir().join(name), s);
    }
}

pub struct Download {
    pub id: u32,
    pub url: String,
    pub path: String,
    pub state: &'static str, // "running" | "done" | "failed"
}

pub struct App {
    pub settings: Map<String, Value>,
    pub bookmarks: Vec<(String, String)>, // (заголовок, url)
    pub downloads: Vec<Download>,
    pub next_dl: u32,
    pub session: (Vec<String>, usize),
}

impl App {
    pub fn load() -> App {
        let settings: Map<String, Value> = read_json("settings.json")
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        let bookmarks: Vec<(String, String)> = read_json("bookmarks.json")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .filter_map(|b| Some((b["title"].as_str()?.to_string(), b["url"].as_str()?.to_string())))
            .collect();
        let session: (Vec<String>, usize) = match read_json("session.json") {
            Some(v) => {
                let urls: Vec<String> = v["urls"]
                    .as_array()
                    .map(|a| a.iter().filter_map(|u| u.as_str().map(String::from)).collect())
                    .unwrap_or_default();
                (urls, v["active"].as_u64().unwrap_or(0) as usize)
            }
            None => (Vec::new(), 0),
        };
        App { settings, bookmarks, downloads: Vec::new(), next_dl: 1, session }
    }

    fn default_of(k: &str) -> bool {
        matches!(k, "newtab_page" | "accel_keys")
    }

    pub fn b(&self, k: &str) -> bool {
        self.settings
            .get(k)
            .and_then(Value::as_bool)
            .unwrap_or_else(|| Self::default_of(k))
    }

    pub fn s(&self, k: &str) -> String {
        self.settings.get(k).and_then(Value::as_str).unwrap_or("").to_string()
    }

    pub fn disabled_keys(&self) -> Vec<String> {
        self.settings
            .get("disabled_keys")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default()
    }

    pub fn set(&mut self, k: &str, v: Value) {
        self.settings.insert(k.to_string(), v);
        self.save_settings();
    }

    /// Установка из страницы настроек: только разрешённые ключи.
    pub fn set_user(&mut self, k: &str, v: Value) -> bool {
        if SETTING_KEYS.contains(&k) {
            self.set(k, v);
            true
        } else {
            false
        }
    }

    fn save_settings(&self) {
        write_json("settings.json", &Value::Object(self.settings.clone()));
    }

    fn save_bookmarks(&self) {
        let arr: Vec<Value> = self
            .bookmarks
            .iter()
            .map(|(t, u)| json!({ "title": t, "url": u }))
            .collect();
        write_json("bookmarks.json", &Value::Array(arr));
    }

    pub fn save_session(&mut self, urls: Vec<String>, active: usize) {
        write_json("session.json", &json!({ "urls": urls, "active": active }));
        self.session = (urls, active);
    }

    // --- закладки ---
    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.bookmarks.iter().any(|(_, u)| u == url)
    }

    pub fn toggle_bookmark(&mut self, title: &str, url: &str) {
        if let Some(i) = self.bookmarks.iter().position(|(_, u)| u == url) {
            self.bookmarks.remove(i);
        } else {
            self.bookmarks.push((title.to_string(), url.to_string()));
        }
        self.save_bookmarks();
    }

    pub fn remove_bookmark(&mut self, url: &str) {
        self.bookmarks.retain(|(_, u)| u != url);
        self.save_bookmarks();
    }

    // --- масштаб по сайтам ---
    pub fn zoom_for(&self, host: &str) -> f64 {
        self.settings
            .get("zoom")
            .and_then(|z| z.get(host))
            .and_then(Value::as_f64)
            .unwrap_or(1.0)
    }

    pub fn set_zoom(&mut self, host: &str, z: f64) {
        let m = self.settings.entry("zoom".to_string()).or_insert_with(|| json!({}));
        if let Some(o) = m.as_object_mut() {
            if (z - 1.0).abs() < 0.001 {
                o.remove(host);
            } else {
                o.insert(host.to_string(), json!(z));
            }
        }
        self.save_settings();
    }

    // --- загрузки ---
    pub fn dl_path(&self, id: u32) -> Option<String> {
        self.downloads
            .iter()
            .find(|d| d.id == id && d.state == "done")
            .map(|d| d.path.clone())
    }

    pub fn state_json(&self) -> Value {
        let bookmarks: Vec<Value> = self
            .bookmarks
            .iter()
            .map(|(t, u)| json!({ "title": t, "url": u }))
            .collect();
        let downloads: Vec<Value> = self
            .downloads
            .iter()
            .map(|d| {
                let name = Path::new(&d.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                json!({ "id": d.id, "url": d.url, "name": name, "state": d.state })
            })
            .collect();
        json!({
            "settings": Value::Object(self.settings.clone()),
            "bookmarks": bookmarks,
            "downloads": downloads,
            "keymap": keymap(),
            "version": VERSION,
            "defaults": { "newtab_page": true, "accel_keys": true },
        })
    }
}

fn e(c: &str, l: &str, a: &str, d: &str) -> Value {
    json!({ "c": c, "l": l, "a": a, "d": d })
}

/// Каталог сочетаний. `c` = модификаторы + e.code в нижнем регистре (не зависит от раскладки),
/// `a` = действие браузера (пусто = встроенное сочетание Chromium, его можно только отключить).
pub fn keymap() -> Value {
    let mut v = vec![
        e("ctrl+keyt", "Ctrl+T", "newtab", "Новая вкладка"),
        e("ctrl+shift+keyt", "Ctrl+Shift+T", "reopen", "Вернуть закрытую вкладку"),
        e("ctrl+keyw", "Ctrl+W", "close", "Закрыть вкладку"),
        e("ctrl+shift+keyn", "Ctrl+Shift+N", "incognito", "Инкогнито-вкладка"),
        e("ctrl+tab", "Ctrl+Tab", "next", "Следующая вкладка"),
        e("ctrl+pagedown", "Ctrl+PgDn", "next", "Следующая вкладка"),
        e("ctrl+shift+tab", "Ctrl+Shift+Tab", "prev", "Предыдущая вкладка"),
        e("ctrl+pageup", "Ctrl+PgUp", "prev", "Предыдущая вкладка"),
        e("ctrl+keyl", "Ctrl+L", "focusurl", "Строка адреса"),
        e("ctrl+keyr", "Ctrl+R", "reload", "Обновить страницу"),
        e("f5", "F5", "reload", "Обновить страницу"),
        e("alt+arrowleft", "Alt+←", "back", "Назад"),
        e("alt+arrowright", "Alt+→", "fwd", "Вперёд"),
        e("ctrl+keyd", "Ctrl+D", "bookmark", "Добавить в закладки"),
        e("ctrl+shift+keyo", "Ctrl+Shift+O", "bookmarks", "Закладки"),
        e("ctrl+keyj", "Ctrl+J", "downloads", "Загрузки"),
        e("ctrl+comma", "Ctrl+,", "settings", "Настройки"),
        e("ctrl+equal", "Ctrl+=", "zoomin", "Увеличить масштаб"),
        e("ctrl+numpadadd", "Ctrl+Num+", "zoomin", "Увеличить масштаб"),
        e("ctrl+minus", "Ctrl+−", "zoomout", "Уменьшить масштаб"),
        e("ctrl+numpadsubtract", "Ctrl+Num−", "zoomout", "Уменьшить масштаб"),
        e("ctrl+digit0", "Ctrl+0", "zoomreset", "Масштаб 100%"),
        e("ctrl+numpad0", "Ctrl+Num0", "zoomreset", "Масштаб 100%"),
        e("f11", "F11", "fullscreen", "Полный экран"),
        e("ctrl+wheel", "Ctrl+колесо", "", "Масштаб колесом мыши"),
        // встроенные сочетания Chromium: их можно только отключить
        e("f12", "F12", "", "Инструменты разработчика"),
        e("ctrl+shift+keyi", "Ctrl+Shift+I", "", "Инструменты разработчика"),
        e("ctrl+shift+keyj", "Ctrl+Shift+J", "", "Консоль"),
        e("ctrl+shift+keyc", "Ctrl+Shift+C", "", "Выбор элемента"),
        e("ctrl+keyu", "Ctrl+U", "", "Исходный код страницы"),
        e("ctrl+keyp", "Ctrl+P", "", "Печать"),
        e("ctrl+keys", "Ctrl+S", "", "Сохранить страницу"),
        e("ctrl+keyf", "Ctrl+F", "", "Поиск по странице"),
        e("f7", "F7", "", "Режим курсора (caret browsing)"),
    ];
    for d in 1..=9 {
        v.push(e(
            &format!("ctrl+digit{d}"),
            &format!("Ctrl+{d}"),
            &format!("tab{d}"),
            &format!("Перейти на вкладку {d}"),
        ));
    }
    Value::Array(v)
}
