//! tirage — collecteur du serveur d'impression.
//!
//! Lot 2 : mode console (`--console`) pour l'essai ; le service Windows et l'envoi au serveur viennent ensuite.

#[cfg(windows)]
use tirage_collecteur::spouleur;

use std::path::PathBuf;

use tirage_modele::PolitiqueNomDocument;

fn texte_version() -> String {
    format!(
        "tirage-collecteur {} (format d'échange v{})",
        env!("CARGO_PKG_VERSION"),
        tirage_modele::VERSION_FORMAT
    )
}

const USAGE: &str = "usage :
  tirage-collecteur --version
  tirage-collecteur --console [--donnees DOSSIER] [--collecteur NOM] [--nom-document non|application|complet]

--console : surveille le spouleur local, affiche chaque travail terminé (une ligne JSON) et le dépose
            dans la file d'attente (DOSSIER/a-envoyer). Ctrl+C pour arrêter : rien de déposé n'est perdu.";

#[derive(Debug, PartialEq)]
struct OptionsConsole {
    donnees: PathBuf,
    collecteur: String,
    politique: PolitiqueNomDocument,
}

fn lire_options(args: &[String]) -> Result<OptionsConsole, String> {
    let mut o = OptionsConsole {
        donnees: PathBuf::from("donnees"),
        collecteur: "collecteur".into(),
        politique: PolitiqueNomDocument::default(),
    };
    let mut i = args.iter();
    while let Some(a) = i.next() {
        let mut valeur = || {
            i.next()
                .cloned()
                .ok_or_else(|| format!("valeur manquante après {a}"))
        };
        match a.as_str() {
            "--donnees" => o.donnees = PathBuf::from(valeur()?),
            "--collecteur" => o.collecteur = valeur()?,
            "--nom-document" => {
                o.politique = match valeur()?.as_str() {
                    "non" => PolitiqueNomDocument::Non,
                    "application" => PolitiqueNomDocument::Application,
                    "complet" => PolitiqueNomDocument::Complet,
                    v => return Err(format!("--nom-document : valeur inconnue « {v} »")),
                }
            }
            autre => return Err(format!("option inconnue : {autre}")),
        }
    }
    Ok(o)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => println!("{}", texte_version()),
        Some("--console") => match lire_options(&args[1..]) {
            Ok(o) => {
                if let Err(e) = console(&o) {
                    eprintln!("erreur : {e}");
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("{e}\n{USAGE}");
                std::process::exit(2);
            }
        },
        _ => {
            eprintln!("{}\n{USAGE}", texte_version());
            std::process::exit(2);
        }
    }
}

#[cfg(windows)]
fn console(o: &OptionsConsole) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::AtomicBool;
    use tirage_modele::{Enveloppe, Evenement};

    let mut file = tirage_file::FileAttente::ouvrir(&o.donnees, 100_000)?;
    eprintln!(
        "{} — surveillance du spouleur ; file : {} élément(s) en attente",
        texte_version(),
        file.longueur()
    );
    let arret = AtomicBool::new(false);
    let mut erreur: Option<String> = None;
    spouleur::windows::surveiller(&arret, |suivi| {
        for brut in suivi.prendre_termines() {
            let Some(t) = spouleur::suivi::convertir(&brut, &o.collecteur, o.politique) else {
                eprintln!(
                    "travail n° {} inexploitable (sans file ni utilisateur), ignoré",
                    brut.numero
                );
                continue;
            };
            let e = Enveloppe::nouvelle(Evenement::Travail(t));
            println!("{}", String::from_utf8_lossy(&e.encoder()));
            if let Err(err) = file.deposer(&e.encoder()) {
                erreur.get_or_insert_with(|| err.to_string());
            }
        }
    })?;
    match erreur {
        Some(e) => Err(e.into()),
        None => Ok(()),
    }
}

#[cfg(not(windows))]
fn console(_o: &OptionsConsole) -> Result<(), Box<dyn std::error::Error>> {
    Err("le mode console lit le spouleur Windows : il ne fonctionne que sous Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn version_lisible() {
        assert!(texte_version().starts_with("tirage-collecteur 0.1.0"));
    }

    #[test]
    fn options_par_defaut() {
        let o = lire_options(&[]).unwrap();
        assert_eq!(o.politique, PolitiqueNomDocument::Application);
        assert_eq!(o.donnees, PathBuf::from("donnees"));
    }

    #[test]
    fn options_lues() {
        let o = lire_options(&v(&[
            "--donnees",
            "D",
            "--collecteur",
            "impression-1",
            "--nom-document",
            "complet",
        ]))
        .unwrap();
        assert_eq!(
            o,
            OptionsConsole {
                donnees: PathBuf::from("D"),
                collecteur: "impression-1".into(),
                politique: PolitiqueNomDocument::Complet
            }
        );
    }

    #[test]
    fn options_refusees() {
        assert!(lire_options(&v(&["--nom-document", "tout"])).is_err());
        assert!(lire_options(&v(&["--donnees"])).is_err());
        assert!(lire_options(&v(&["--inconnue"])).is_err());
    }
}
