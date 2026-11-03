//! tirage — collecteur du serveur d'impression.
//!
//! Modes d'essai : `--console` (travaux du spouleur), `--releve` et `--parcourir` (copieurs, SNMP en lecture).
//! Le service Windows et l'envoi au serveur viennent ensuite.

#[cfg(windows)]
use tirage_collecteur::spouleur;

use std::path::PathBuf;

use tirage_collecteur::config::{self, Config};
use tirage_collecteur::copieurs::{self, Profils};
use tirage_collecteur::spouleur::temps::maintenant_ms;
use tirage_modele::{Enveloppe, Evenement, PolitiqueNomDocument};
use tirage_snmp::{Client, Oid, Udp};

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
  tirage-collecteur --releve --config FICHIER [--copieur NOM]
  tirage-collecteur --parcourir OID --config FICHIER --copieur NOM [--max N]

--console : surveille le spouleur local, affiche chaque travail terminé (une ligne JSON) et le dépose
            dans la file d'attente (DOSSIER/a-envoyer). Ctrl+C pour arrêter : rien de déposé n'est perdu.
--releve    : relève une fois chaque copieur de la configuration (ou celui nommé) et affiche le relevé (JSON).
--parcourir : liste les valeurs SNMP d'un sous-arbre d'un copieur (ex. 1.3.6.1.4.1.<entreprise>), pour écrire
              un profil de marque. Lecture seule ; la communauté vient de la configuration, jamais de la ligne.";

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
        Some("--releve") => {
            terminer(lire_snmp(&args[1..], None).and_then(|o| releve(&o).map_err(Into::into)))
        }
        Some("--parcourir") => match args.get(1) {
            Some(base) => {
                terminer(lire_snmp(&args[2..], Some(base)).and_then(|o| parcourir(&o).map_err(Into::into)))
            }
            None => terminer(Err(Erreur::Usage("--parcourir : OID manquant".into()))),
        },
        _ => {
            eprintln!("{}\n{USAGE}", texte_version());
            std::process::exit(2);
        }
    }
}

enum Erreur {
    Usage(String),
    Echec(String),
}

impl From<String> for Erreur {
    fn from(e: String) -> Self {
        Erreur::Echec(e)
    }
}

fn terminer(r: Result<(), Erreur>) {
    match r {
        Ok(()) => {}
        Err(Erreur::Usage(e)) => {
            eprintln!("{e}\n{USAGE}");
            std::process::exit(2);
        }
        Err(Erreur::Echec(e)) => {
            eprintln!("erreur : {e}");
            std::process::exit(1);
        }
    }
}

#[derive(Debug, PartialEq)]
struct OptionsSnmp {
    config: PathBuf,
    copieur: Option<String>,
    base: Option<Oid>,
    max: usize,
}

fn lire_snmp(args: &[String], base: Option<&String>) -> Result<OptionsSnmp, Erreur> {
    let u = |m: String| Erreur::Usage(m);
    let mut o = OptionsSnmp {
        config: PathBuf::new(),
        copieur: None,
        base: base.map(|b| b.parse::<Oid>().map_err(|e| u(e.0))).transpose()?,
        max: 5000,
    };
    let mut i = args.iter();
    while let Some(a) = i.next() {
        let mut valeur = || {
            i.next()
                .cloned()
                .ok_or_else(|| u(format!("valeur manquante après {a}")))
        };
        match a.as_str() {
            "--config" => o.config = PathBuf::from(valeur()?),
            "--copieur" => o.copieur = Some(valeur()?),
            "--max" if o.base.is_some() => {
                o.max = valeur()?
                    .parse()
                    .map_err(|_| u("--max : nombre attendu".into()))?;
            }
            autre => return Err(u(format!("option inconnue : {autre}"))),
        }
    }
    if o.config.as_os_str().is_empty() {
        return Err(u("--config FICHIER est obligatoire".into()));
    }
    if o.base.is_some() && o.copieur.is_none() {
        return Err(u("--parcourir : --copieur NOM est obligatoire".into()));
    }
    Ok(o)
}

fn client(cfg: &Config, c: &config::Copieur) -> Result<Client<Udp>, String> {
    let udp =
        Udp::ouvrir(&c.adresse, cfg.snmp.delai(), cfg.snmp.essais).map_err(|e| format!("{} : {e}", c.nom))?;
    Ok(Client::nouveau(udp, c.communaute.as_bytes()))
}

fn releve(o: &OptionsSnmp) -> Result<(), String> {
    let cfg = Config::charger(&o.config)?;
    let profils = Profils::charger(cfg.snmp.dossier_profils.as_deref())?;
    let cibles: Vec<&config::Copieur> = match &o.copieur {
        Some(n) => vec![
            cfg.copieur(n)
                .ok_or_else(|| format!("copieur absent de la configuration : {n}"))?,
        ],
        None => cfg.copieurs.iter().collect(),
    };
    if cibles.is_empty() {
        return Err("aucun copieur dans la configuration".into());
    }
    let mut echecs = 0;
    for c in cibles {
        let r = client(&cfg, c).and_then(|mut cl| {
            copieurs::relever(
                &mut cl,
                &cfg.collecteur,
                &c.nom,
                &profils,
                c.profil.as_deref(),
                maintenant_ms(),
            )
            .map_err(|e| e.to_string())
        });
        match r {
            Ok((rel, ctx)) => {
                let id = ctx
                    .sys_object_id
                    .map(|o| o.to_string())
                    .unwrap_or_else(|| "?".into());
                eprintln!("{} : profil {} (identifiant d'appareil {id})", c.nom, ctx.profil);
                let e = Enveloppe::nouvelle(Evenement::Releve(rel));
                println!("{}", String::from_utf8_lossy(&e.encoder()));
            }
            Err(e) => {
                echecs += 1;
                eprintln!("{} : {e}", c.nom);
            }
        }
    }
    match echecs {
        0 => Ok(()),
        n => Err(format!("{n} copieur(s) sans relevé")),
    }
}

fn parcourir(o: &OptionsSnmp) -> Result<(), String> {
    let cfg = Config::charger(&o.config)?;
    let nom = o.copieur.as_deref().unwrap_or_default();
    let c = cfg
        .copieur(nom)
        .ok_or_else(|| format!("copieur absent de la configuration : {nom}"))?;
    let base = o.base.clone().unwrap_or_else(|| Oid(vec![1, 3]));
    let lignes = client(&cfg, c)?
        .parcourir(&base, o.max)
        .map_err(|e| e.to_string())?;
    for (oid, v) in &lignes {
        println!("{oid} = {v}");
    }
    eprintln!(
        "{} valeur(s) sous {base}{}",
        lignes.len(),
        if lignes.len() >= o.max {
            " (limite --max atteinte)"
        } else {
            ""
        }
    );
    Ok(())
}

#[cfg(windows)]
fn console(o: &OptionsConsole) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::atomic::AtomicBool;

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
    fn options_snmp() {
        let base = "1.3.6.1.4.1.99999".to_owned();
        let o = lire_snmp(
            &v(&["--config", "c.toml", "--copieur", "A", "--max", "10"]),
            Some(&base),
        )
        .ok()
        .unwrap();
        assert_eq!((o.copieur.as_deref(), o.max), (Some("A"), 10));
        assert!(
            lire_snmp(&v(&["--config", "c.toml"]), Some(&base)).is_err(),
            "copieur obligatoire"
        );
        assert!(
            lire_snmp(&v(&["--copieur", "A"]), None).is_err(),
            "config obligatoire"
        );
        assert!(
            lire_snmp(&v(&["--config", "c", "--max", "3"]), None).is_err(),
            "--max réservé au parcours"
        );
        assert!(lire_snmp(&v(&["--config", "c", "--copieur", "A"]), Some(&"x.y".to_owned())).is_err());
        assert!(lire_snmp(&v(&["--config", "c"]), None).is_ok());
    }

    #[test]
    fn options_refusees() {
        assert!(lire_options(&v(&["--nom-document", "tout"])).is_err());
        assert!(lire_options(&v(&["--donnees"])).is_err());
        assert!(lire_options(&v(&["--inconnue"])).is_err());
    }
}
