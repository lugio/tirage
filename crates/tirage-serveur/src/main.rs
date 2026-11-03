//! tirage — serveur.
//!
//! `--config FICHIER` : reçoit les événements des collecteurs (HTTP ; à placer derrière un mandataire HTTPS).
//! `--nouveau-jeton NOM` : fabrique le jeton d'un collecteur et la ligne à ajouter à la configuration.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tirage_serveur::api::{self, Etat};
use tirage_serveur::base::Base;
use tirage_serveur::config::{self, Config};

fn texte_version() -> String {
    format!(
        "tirage-serveur {} (format d'échange v{})",
        env!("CARGO_PKG_VERSION"),
        tirage_modele::VERSION_FORMAT
    )
}

const USAGE: &str = "usage :
  tirage-serveur --version
  tirage-serveur --config FICHIER
  tirage-serveur --nouveau-jeton NOM_DU_COLLECTEUR";

fn maintenant_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match (args.first().map(String::as_str), args.get(1)) {
        (Some("--version") | Some("-V"), None) => {
            println!("{}", texte_version());
            Ok(())
        }
        (Some("--nouveau-jeton"), Some(nom)) if args.len() == 2 => nouveau_jeton(nom),
        (Some("--config"), Some(f)) if args.len() == 2 => servir(PathBuf::from(f)),
        _ => {
            eprintln!("{}\n{USAGE}", texte_version());
            std::process::exit(2);
        }
    };
    if let Err(e) = r {
        eprintln!("erreur : {e}");
        std::process::exit(1);
    }
}

fn nouveau_jeton(nom: &str) -> Result<(), String> {
    let jeton = config::nouveau_jeton()?;
    eprintln!(
        "Jeton du collecteur « {nom} » (à remettre au collecteur, puis au coffre ; il n'est affiché qu'une fois) :"
    );
    println!("{jeton}");
    eprintln!(
        "\nÀ ajouter à la configuration du serveur :\n\n[[collecteurs]]\nnom = \"{nom}\"\njeton_sha256 = \"{}\"",
        config::empreinte(&jeton)
    );
    Ok(())
}

fn servir(chemin: PathBuf) -> Result<(), String> {
    let cfg = Config::charger(&chemin)?;
    if cfg.collecteurs.is_empty() {
        eprintln!("attention : aucun collecteur déclaré, tout envoi sera refusé");
    }
    let base = Base::ouvrir(&cfg.base).map_err(|e| format!("base {} : {e}", cfg.base.display()))?;
    let ecoute = cfg.ecoute.clone();
    let etat = Etat {
        config: Arc::new(cfg),
        base: Arc::new(Mutex::new(base)),
        horloge: maintenant_ms,
    };
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let ecouteur = tokio::net::TcpListener::bind(&ecoute)
            .await
            .map_err(|e| format!("écoute {ecoute} : {e}"))?;
        eprintln!("{} — écoute sur {ecoute}", texte_version());
        axum::serve(ecouteur, api::routeur(etat))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|e| e.to_string())
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_lisible() {
        assert!(super::texte_version().starts_with("tirage-serveur 0.1.0"));
    }
}
