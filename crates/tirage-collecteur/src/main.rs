//! tirage — collecteur. Squelette (lot 0) : seule l'option `--version` existe.

fn texte_version() -> String {
    format!(
        "tirage-collecteur {} (format d'échange v{})",
        env!("CARGO_PKG_VERSION"),
        tirage_modele::VERSION_FORMAT
    )
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("--version") | Some("-V") => println!("{}", texte_version()),
        _ => {
            eprintln!("{}\nusage : tirage-collecteur --version", texte_version());
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_lisible() {
        assert!(super::texte_version().starts_with("tirage-collecteur 0.1.0"));
    }
}
