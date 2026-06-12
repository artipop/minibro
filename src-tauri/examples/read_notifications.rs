//! Консольная проверка чтения уведомлений: `cargo run --example read_notifications`.
//! С флагом `--expand` предварительно раскрывает свёрнутые стопки в шторке.

fn main() {
    let expand = std::env::args().any(|a| a == "--expand");
    let trusted = minibro_lib::notifications::ax_trusted(false);
    eprintln!("accessibility trusted: {trusted}");
    match minibro_lib::notifications::read_notifications_opts(expand) {
        Ok(items) => {
            eprintln!("found {} notification(s)", items.len());
            println!("{}", serde_json::to_string_pretty(&items).unwrap());
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
