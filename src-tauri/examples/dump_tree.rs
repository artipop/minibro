//! Отладочный дамп AX-дерева NotificationCenter: `cargo run --example dump_tree`.

fn main() {
    match minibro_lib::notifications::dump_tree() {
        Ok(tree) => println!("{tree}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
