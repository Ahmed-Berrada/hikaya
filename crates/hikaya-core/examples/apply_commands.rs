//! Reads text on stdin and prints it with spoken commands applied, for
//! `eval/commands/run.sh`.

use std::io::Read;

fn main() {
    let mut text = String::new();
    if std::io::stdin().read_to_string(&mut text).is_err() {
        std::process::exit(2);
    }
    print!("{}", hikaya_core::commands::apply_commands(&text, &["en", "fr"]));
}
