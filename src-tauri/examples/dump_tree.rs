//! Отладочный дамп AX-дерева процесса: `cargo run --example dump_tree [процесс]`
//! (по умолчанию NotificationCenter).

fn main() {
    let process = std::env::args().nth(1).unwrap_or("NotificationCenter".into());
    match minibro_lib::notifications::dump_tree_for(&process) {
        Ok(tree) => println!("{tree}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
