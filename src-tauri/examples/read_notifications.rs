//! Консольная проверка чтения уведомлений: `cargo run --example read_notifications`.

fn main() {
    let trusted = minibro_lib::notifications::ax_trusted(false);
    eprintln!("accessibility trusted: {trusted}");
    match minibro_lib::notifications::read_notifications() {
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
