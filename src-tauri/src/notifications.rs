//! Чтение уведомлений macOS из Notification Center через Accessibility API.
//!
//! Работает так: находим процесс `NotificationCenter`, создаём для него
//! AXUIElement приложения и обходим дерево элементов. Уведомления — это
//! элементы с subrole `AXNotificationCenterAlert` (шторка открыта) или
//! `AXNotificationCenterBanner` (всплывающий баннер).
//!
//! Требуется разрешение Accessibility (System Settings → Privacy & Security →
//! Accessibility). В dev-режиме разрешение нужно выдать терминалу/IDE,
//! из которого запущен `tauri dev`.

use std::collections::HashSet;
use std::os::raw::c_void;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use core_foundation::array::CFArrayRef;
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::runloop::{kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopSource};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation_sys::array::{CFArrayGetCount, CFArrayGetTypeID, CFArrayGetValueAtIndex};
use core_foundation_sys::runloop::CFRunLoopSourceRef;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

type AXUIElementRef = *const c_void;
type AXObserverRef = *mut c_void;
type AXError = i32;
type AXObserverCallback =
    extern "C" fn(AXObserverRef, AXUIElementRef, CFStringRef, *mut c_void);

const K_AX_ERROR_SUCCESS: AXError = 0;

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    static kAXTrustedCheckOptionPrompt: CFStringRef;

    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementCopyActionNames(element: AXUIElementRef, names: *mut CFArrayRef) -> AXError;

    fn AXObserverCreate(
        pid: i32,
        callback: AXObserverCallback,
        observer: *mut AXObserverRef,
    ) -> AXError;
    fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        refcon: *mut c_void,
    ) -> AXError;
    fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFRunLoopSourceRef;
}

#[derive(Serialize, Clone)]
pub struct NotificationItem {
    /// AXNotificationCenterAlert (шторка) или AXNotificationCenterBanner (баннер).
    pub subrole: String,
    /// AXDescription — обычно "Приложение, время, заголовок, текст" одной строкой.
    pub description: Option<String>,
    /// Тексты дочерних AXStaticText: как правило [заголовок, подзаголовок?, тело].
    pub texts: Vec<String>,
    /// Структурированные поля — у AXStaticText внутри уведомления есть
    /// AXIdentifier "title"/"subtitle"/"body".
    pub title: Option<String>,
    pub subtitle: Option<String>,
    pub body: Option<String>,
    /// true для свёрнутой стопки (*Stack): видно только верхнее уведомление,
    /// id — идентификатор этого верхнего.
    pub stacked: bool,
    pub identifier: Option<String>,
    /// Bundle id приложения-отправителя (если система его отдаёт).
    pub stacking_identifier: Option<String>,
    /// Доступные AX-действия (Close, Show Details, …) — пригодятся для клика/закрытия.
    pub actions: Vec<String>,
}

/// Проверка (и опционально запрос) разрешения Accessibility.
pub fn ax_trusted(prompt: bool) -> bool {
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let value = if prompt {
            CFBoolean::true_value()
        } else {
            CFBoolean::false_value()
        };
        let options = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
        AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}

fn notification_center_pid() -> Option<i32> {
    let output = Command::new("/usr/bin/pgrep")
        .args(["-x", "NotificationCenter"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .trim()
        .parse()
        .ok()
}

fn copy_attr(element: AXUIElementRef, name: &str) -> Option<CFType> {
    let attr = CFString::new(name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = unsafe {
        AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value)
    };
    if err == K_AX_ERROR_SUCCESS && !value.is_null() {
        Some(unsafe { CFType::wrap_under_create_rule(value) })
    } else {
        None
    }
}

fn attr_string(element: AXUIElementRef, name: &str) -> Option<String> {
    copy_attr(element, name)?
        .downcast::<CFString>()
        .map(|s| s.to_string())
}

/// Обходит детей элемента, вызывая `f` для каждого. Массив-владелец жив на
/// время обхода, поэтому сырые указатели на детей валидны внутри `f`.
fn for_each_child(element: AXUIElementRef, mut f: impl FnMut(AXUIElementRef)) {
    let Some(children) = copy_attr(element, "AXChildren") else {
        return;
    };
    if children.type_of() != unsafe { CFArrayGetTypeID() } {
        return;
    }
    let array = children.as_CFTypeRef() as CFArrayRef;
    let count = unsafe { CFArrayGetCount(array) };
    for i in 0..count {
        let child = unsafe { CFArrayGetValueAtIndex(array, i) } as AXUIElementRef;
        if !child.is_null() {
            f(child);
        }
    }
}

fn action_names(element: AXUIElementRef) -> Vec<String> {
    let mut names: CFArrayRef = std::ptr::null();
    let err = unsafe { AXUIElementCopyActionNames(element, &mut names) };
    if err != K_AX_ERROR_SUCCESS || names.is_null() {
        return Vec::new();
    }
    let owned = unsafe { CFType::wrap_under_create_rule(names as CFTypeRef) };
    let array = owned.as_CFTypeRef() as CFArrayRef;
    let count = unsafe { CFArrayGetCount(array) };
    (0..count)
        .filter_map(|i| {
            let item = unsafe { CFArrayGetValueAtIndex(array, i) };
            if item.is_null() {
                return None;
            }
            let s = unsafe { CFString::wrap_under_get_rule(item as CFStringRef) };
            let s = s.to_string();
            // Кастомные действия приходят как "Name:Close\nTarget:0x0\nSelector:(null)" —
            // оставляем только имя.
            let s = match s.strip_prefix("Name:") {
                Some(rest) => rest.lines().next().unwrap_or(rest).to_string(),
                None => s,
            };
            Some(s)
        })
        .collect()
}

/// (значение, AXIdentifier текста) для всех AXStaticText внутри уведомления.
fn collect_static_texts(element: AXUIElementRef, depth: usize, out: &mut Vec<(String, Option<String>)>) {
    if depth > 10 {
        return;
    }
    if attr_string(element, "AXRole").as_deref() == Some("AXStaticText") {
        if let Some(value) = attr_string(element, "AXValue") {
            if !value.is_empty() {
                out.push((value, attr_string(element, "AXIdentifier")));
            }
        }
    }
    for_each_child(element, |child| collect_static_texts(child, depth + 1, out));
}

fn extract_notification(element: AXUIElementRef, subrole: String) -> NotificationItem {
    let mut labelled = Vec::new();
    collect_static_texts(element, 0, &mut labelled);
    let field = |name: &str| {
        labelled
            .iter()
            .find(|(_, id)| id.as_deref() == Some(name))
            .map(|(v, _)| v.clone())
    };
    NotificationItem {
        stacked: subrole.ends_with("Stack"),
        description: attr_string(element, "AXDescription"),
        title: field("title"),
        subtitle: field("subtitle"),
        body: field("body"),
        texts: labelled.into_iter().map(|(v, _)| v).collect(),
        identifier: attr_string(element, "AXIdentifier"),
        stacking_identifier: attr_string(element, "AXStackingIdentifier"),
        actions: action_names(element),
        subrole,
    }
}

fn walk(element: AXUIElementRef, depth: usize, budget: &mut usize, out: &mut Vec<NotificationItem>) {
    if depth > 40 || *budget == 0 {
        return;
    }
    *budget -= 1;

    if let Some(subrole) = attr_string(element, "AXSubrole") {
        // Banner — всплывший/одиночный, Alert — вариант с кнопками, *Stack —
        // свёрнутая стопка уведомлений одного приложения (видна только верхняя).
        if matches!(
            subrole.as_str(),
            "AXNotificationCenterAlert"
                | "AXNotificationCenterBanner"
                | "AXNotificationCenterAlertStack"
                | "AXNotificationCenterBannerStack"
        ) {
            out.push(extract_notification(element, subrole));
            return;
        }
    }
    for_each_child(element, |child| walk(child, depth + 1, budget, out));
}

/// Читает видимые уведомления: баннеры всегда, полный список — когда шторка
/// Notification Center открыта (окно процесса существует только в эти моменты).
pub fn read_notifications() -> Result<Vec<NotificationItem>, String> {
    if !ax_trusted(false) {
        return Err(
            "Нет разрешения Accessibility. Выдайте его в System Settings → Privacy & Security → Accessibility (в dev-режиме — терминалу, из которого запущено приложение).".into(),
        );
    }
    let pid = notification_center_pid().ok_or("Процесс NotificationCenter не найден")?;
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Err("AXUIElementCreateApplication вернул null".into());
    }
    let app = unsafe { CFType::wrap_under_create_rule(app as CFTypeRef) };

    let mut items = Vec::new();
    let mut budget = 20_000usize;
    if let Some(windows) = copy_attr(app.as_CFTypeRef() as AXUIElementRef, "AXWindows") {
        if windows.type_of() == unsafe { CFArrayGetTypeID() } {
            let array = windows.as_CFTypeRef() as CFArrayRef;
            let count = unsafe { CFArrayGetCount(array) };
            for i in 0..count {
                let window = unsafe { CFArrayGetValueAtIndex(array, i) } as AXUIElementRef;
                if !window.is_null() {
                    walk(window, 0, &mut budget, &mut items);
                }
            }
        }
    }
    Ok(items)
}

// ---------------------------------------------------------------------------
// Наблюдатель: AXObserver на процессе NotificationCenter собирает уведомления
// в момент появления баннера, не требуя открытой шторки. Лог живёт с момента
// запуска наблюдателя; историю «до» этим способом не достать.
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct LoggedNotification {
    /// Unix-время появления в логе, миллисекунды.
    pub seen_at_ms: u64,
    #[serde(flatten)]
    pub item: NotificationItem,
}

struct LogState {
    items: Vec<LoggedNotification>,
    seen: HashSet<String>,
}

static LOG: LazyLock<Mutex<LogState>> = LazyLock::new(|| {
    Mutex::new(LogState {
        items: Vec::new(),
        seen: HashSet::new(),
    })
});
static WATCHER_STARTED: AtomicBool = AtomicBool::new(false);
static APP_HANDLE: OnceLock<AppHandle<tauri::Cef>> = OnceLock::new();
static TICK: OnceLock<Sender<()>> = OnceLock::new();

/// Есть ли сейчас у NotificationCenter хоть одно окно (баннер или шторка).
fn nc_has_windows() -> bool {
    let Some(pid) = notification_center_pid() else {
        return false;
    };
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return false;
    }
    let app = unsafe { CFType::wrap_under_create_rule(app as CFTypeRef) };
    let Some(windows) = copy_attr(app.as_CFTypeRef() as AXUIElementRef, "AXWindows") else {
        return false;
    };
    windows.type_of() == unsafe { CFArrayGetTypeID() }
        && unsafe { CFArrayGetCount(windows.as_CFTypeRef() as CFArrayRef) } > 0
}

fn dedup_key(item: &NotificationItem) -> String {
    item.identifier
        .clone()
        .unwrap_or_else(|| format!("{:?}|{:?}", item.description, item.texts))
}

/// Сканирует текущее AX-дерево и дописывает в лог уведомления, которых там
/// ещё не было. Новые рассылает фронту событием `notifications://new`.
fn scan_and_log() {
    let Ok(items) = read_notifications() else {
        return;
    };
    let mut fresh = Vec::new();
    {
        let mut log = LOG.lock().unwrap();
        for item in items {
            let key = dedup_key(&item);
            if !log.seen.insert(key) {
                continue;
            }
            let logged = LoggedNotification {
                seen_at_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
                item,
            };
            log.items.push(logged.clone());
            fresh.push(logged);
        }
    }
    if let Some(app) = APP_HANDLE.get() {
        for n in &fresh {
            let _ = app.emit("notifications://new", n);
        }
    }
}

extern "C" fn observer_callback(
    _observer: AXObserverRef,
    _element: AXUIElementRef,
    _notification: CFStringRef,
    _refcon: *mut c_void,
) {
    // Колбэк дёргается на ранлуп-потоке наблюдателя; само чтение делает
    // воркер с дебаунсом — к моменту скана баннер успевает дорисоваться.
    if let Some(tx) = TICK.get() {
        let _ = tx.send(());
    }
}

/// Запускает наблюдатель (идемпотентно). `app` нужен только для эмита
/// событий во фронтенд — из консольных утилит можно передать `None`.
pub fn start_watcher(app: Option<AppHandle<tauri::Cef>>) -> Result<(), String> {
    if !ax_trusted(false) {
        return Err("Нет разрешения Accessibility".into());
    }
    let pid = notification_center_pid().ok_or("Процесс NotificationCenter не найден")?;
    if let Some(app) = app {
        let _ = APP_HANDLE.set(app);
    }
    if WATCHER_STARTED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let (tx, rx) = channel::<()>();
    let _ = TICK.set(tx);

    std::thread::spawn(move || {
        scan_and_log(); // снимок того, что видно прямо сейчас
        while rx.recv().is_ok() {
            // AXCreated не приходит для новых баннеров в уже открытом окне,
            // поэтому после события пересканируем, пока окна не исчезнут.
            loop {
                std::thread::sleep(Duration::from_millis(400));
                while rx.try_recv().is_ok() {}
                scan_and_log();
                if !nc_has_windows() {
                    break;
                }
            }
        }
    });

    std::thread::spawn(move || unsafe {
        let mut observer: AXObserverRef = std::ptr::null_mut();
        if AXObserverCreate(pid, observer_callback, &mut observer) != K_AX_ERROR_SUCCESS
            || observer.is_null()
        {
            eprintln!("notifications: AXObserverCreate failed");
            WATCHER_STARTED.store(false, Ordering::SeqCst);
            return;
        }
        let app_element = AXUIElementCreateApplication(pid);
        // AXWindowCreated — появление окна баннера/шторки; AXCreated — новые
        // элементы в уже открытом окне (стопка баннеров, список в шторке).
        for name in ["AXWindowCreated", "AXCreated"] {
            let s = CFString::new(name);
            AXObserverAddNotification(
                observer,
                app_element,
                s.as_concrete_TypeRef(),
                std::ptr::null_mut(),
            );
        }
        let source = CFRunLoopSource::wrap_under_get_rule(AXObserverGetRunLoopSource(observer));
        CFRunLoop::get_current().add_source(&source, kCFRunLoopDefaultMode);
        // Поток живёт до конца процесса; observer и app_element не освобождаем.
        CFRunLoop::run_current();
    });
    Ok(())
}

pub fn notification_log() -> Vec<LoggedNotification> {
    LOG.lock().unwrap().items.clone()
}

/// Отладочный дамп AX-дерева NotificationCenter: роль/subrole/описание/значение
/// с отступами по глубине.
pub fn dump_tree() -> Result<String, String> {
    fn dump(element: AXUIElementRef, depth: usize, budget: &mut usize, out: &mut String) {
        if depth > 40 || *budget == 0 {
            return;
        }
        *budget -= 1;
        let role = attr_string(element, "AXRole").unwrap_or_default();
        let subrole = attr_string(element, "AXSubrole").unwrap_or_default();
        let desc = attr_string(element, "AXDescription").unwrap_or_default();
        let value = attr_string(element, "AXValue").unwrap_or_default();
        let id = attr_string(element, "AXIdentifier").unwrap_or_default();
        out.push_str(&format!(
            "{}{role} [{subrole}] id={id} desc={desc:?} value={value:?}\n",
            "  ".repeat(depth)
        ));
        for_each_child(element, |child| dump(child, depth + 1, budget, out));
    }

    let pid = notification_center_pid().ok_or("NotificationCenter не найден")?;
    let app = unsafe { AXUIElementCreateApplication(pid) };
    if app.is_null() {
        return Err("AXUIElementCreateApplication вернул null".into());
    }
    let app = unsafe { CFType::wrap_under_create_rule(app as CFTypeRef) };
    let mut out = String::new();
    let mut budget = 20_000usize;
    dump(app.as_CFTypeRef() as AXUIElementRef, 0, &mut budget, &mut out);
    Ok(out)
}

#[tauri::command]
pub fn ax_check_permission(prompt: bool) -> bool {
    ax_trusted(prompt)
}

#[tauri::command]
pub fn get_notifications() -> Result<Vec<NotificationItem>, String> {
    read_notifications()
}

#[tauri::command]
pub fn start_notification_watcher(app: AppHandle<tauri::Cef>) -> Result<(), String> {
    start_watcher(Some(app))
}

#[tauri::command]
pub fn get_notification_log() -> Vec<LoggedNotification> {
    notification_log()
}
