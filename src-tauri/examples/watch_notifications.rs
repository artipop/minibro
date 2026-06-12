//! Консольная проверка наблюдателя: `cargo run --example watch_notifications`.
//! Запускает AXObserver и 30 секунд печатает новые уведомления из лога.

use std::time::{Duration, Instant};

fn main() {
    minibro_lib::notifications::start_watcher(None).expect("watcher failed to start");
    eprintln!("watching for 30s, send a notification…");

    let started = Instant::now();
    let mut printed = 0;
    while started.elapsed() < Duration::from_secs(30) {
        let log = minibro_lib::notifications::notification_log();
        if log.len() > printed {
            for item in &log[printed..] {
                println!("{}", serde_json::to_string_pretty(item).unwrap());
            }
            printed = log.len();
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    eprintln!("done, total logged: {printed}");
}
