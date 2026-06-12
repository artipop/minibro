//! Клик по уведомлению по его AXIdentifier:
//! `cargo run --example press_notification -- <identifier>`.
//! Раскрывает стопки в шторке, если уведомление спрятано внутри.

fn main() {
    let Some(id) = std::env::args().nth(1) else {
        eprintln!("usage: press_notification <AXIdentifier>");
        std::process::exit(2);
    };
    match minibro_lib::notifications::press_notification(&id) {
        Ok(()) => eprintln!("pressed {id}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
