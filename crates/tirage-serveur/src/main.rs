//! tirage — serveur.
//!
//! `--config FICHIER` : reçoit les événements des collecteurs et, si un annuaire est configuré, sert l'interface
//! (HTTP ; à placer derrière un mandataire HTTPS).
//! `--config FICHIER --sauvegarder COPIE` : copie cohérente de la base dans un fichier neuf, serveur en marche ou non.
//! `--nouveau-jeton NOM` : fabrique le jeton d'un collecteur et la ligne à ajouter à la configuration.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tirage_serveur::annuaire::{self, Annuaire, Ldap};
use tirage_serveur::api::{self, Etat};
use tirage_serveur::base::Base;
use tirage_serveur::config::{self, Config};
use tirage_serveur::conservation;
use tirage_serveur::interface::Interface;

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
  tirage-serveur --config FICHIER --sauvegarder COPIE
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
        (Some("--config"), Some(f)) if args.len() == 4 && args[2] == "--sauvegarder" => {
            sauvegarder(&PathBuf::from(f), &PathBuf::from(&args[3]))
        }
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

fn sauvegarder(config: &std::path::Path, copie: &std::path::Path) -> Result<(), String> {
    let cfg = Config::charger(config)?;
    conservation::sauvegarder(&cfg.base, copie)?;
    eprintln!(
        "copie écrite : {} (contient des données personnelles)",
        copie.display()
    );
    Ok(())
}

fn servir(chemin: PathBuf) -> Result<(), String> {
    let cfg = Config::charger(&chemin)?;
    if cfg.collecteurs.is_empty() {
        eprintln!("attention : aucun collecteur déclaré, tout envoi sera refusé");
    }
    let base = Base::ouvrir(&cfg.base).map_err(|e| format!("base {} : {e}", cfg.base.display()))?;
    let tarifs = base.tarifs(&cfg.couts).map_err(|e| format!("tarifs : {e}"))?;
    let ecoute = cfg.ecoute.clone();
    let base = Arc::new(Mutex::new(base));
    let annuaire: Option<(Arc<dyn Annuaire>, u32)> = match &cfg.annuaire {
        Some(a) => Some((Arc::new(Ldap::nouveau(a)?), a.rafraichissement_minutes)),
        None => {
            eprintln!("pas d'annuaire configuré : aucune interface web, seule la réception tourne");
            None
        }
    };
    let regles = cfg.conservation.clone();
    let fuseau = jiff::tz::TimeZone::get(&cfg.interface.fuseau).map_err(|e| e.to_string())?;
    let etat = Etat {
        interface: annuaire.as_ref().map(|(a, _)| {
            Arc::new(Interface::nouvelle(
                Arc::clone(a),
                &cfg.interface,
                tarifs.clone(),
                cfg.conservation.detail_mois,
            ))
        }),
        config: Arc::new(cfg),
        base: Arc::clone(&base),
        horloge: maintenant_ms,
    };
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    rt.block_on(async move {
        let ecouteur = tokio::net::TcpListener::bind(&ecoute)
            .await
            .map_err(|e| format!("écoute {ecoute} : {e}"))?;
        eprintln!("{} — écoute sur {ecoute}", texte_version());
        tokio::spawn(appliquer_la_conservation(Arc::clone(&base), regles, fuseau));
        if let Some((a, minutes)) = annuaire {
            tokio::spawn(rafraichir_les_fiches(a, base, minutes));
        }
        axum::serve(ecouteur, api::routeur(etat))
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|e| e.to_string())
    })
}

/// Au démarrage puis chaque jour. Le journal ne donne que des nombres.
async fn appliquer_la_conservation(
    base: Arc<Mutex<Base>>,
    regles: config::ConfigConservation,
    fuseau: jiff::tz::TimeZone,
) {
    loop {
        let (b, r, f) = (Arc::clone(&base), regles.clone(), fuseau.clone());
        let res =
            tokio::task::spawn_blocking(move || conservation::appliquer(&b, &r, &f, maintenant_ms())).await;
        match res {
            Ok(Ok(bilan)) if bilan.travaux + bilan.personnes + bilan.consultations > 0 => eprintln!(
                "conservation : {} travaux passés en totaux, {} fiches et {} lignes de journal effacées",
                bilan.travaux, bilan.personnes, bilan.consultations
            ),
            Ok(Ok(_)) => {}
            Ok(Err(e)) => eprintln!("conservation : échec : {e}"),
            Err(e) => eprintln!("conservation : interrompue : {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(86_400)).await;
    }
}

/// Relit régulièrement les fiches de l'annuaire. Le journal ne cite aucune personne.
async fn rafraichir_les_fiches(a: Arc<dyn Annuaire>, base: Arc<Mutex<Base>>, minutes: u32) {
    let periode = std::time::Duration::from_secs(u64::from(minutes.min(5)) * 60);
    let age_max = i64::from(minutes) * 60_000;
    loop {
        let (a2, b2) = (Arc::clone(&a), Arc::clone(&base));
        let r = tokio::task::spawn_blocking(move || {
            annuaire::synchroniser(&b2, a2.as_ref(), maintenant_ms(), age_max)
        })
        .await;
        match r {
            Ok(Ok(bilan)) if bilan.verifies > 0 => {
                eprintln!(
                    "annuaire : {} fiche(s) relue(s), {} absente(s)",
                    bilan.verifies, bilan.absents
                );
            }
            Ok(Ok(_)) => {}
            Ok(Err(e)) => eprintln!("annuaire : relecture impossible : {e}"),
            Err(e) => eprintln!("annuaire : relecture interrompue : {e}"),
        }
        tokio::time::sleep(periode).await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_lisible() {
        assert!(super::texte_version().starts_with("tirage-serveur 0.1.0"));
    }
}
