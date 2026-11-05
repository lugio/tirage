//! tirage — collecteur du serveur d'impression.
//!
//! Modes : `--console` (travaux du spouleur, et envoi au serveur si la configuration en déclare un),
//! `--installer` / `--desinstaller` / `--service` (service Windows), `--releve` et `--parcourir` (copieurs, SNMP).

use std::path::PathBuf;

use tirage_collecteur::config::{self, Config};
use tirage_collecteur::copieurs::{self, Profils};
use tirage_collecteur::execution::Reglages;
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
  tirage-collecteur --console [--config FICHIER] [--donnees DOSSIER] [--collecteur NOM]
                    [--nom-document non|application|complet]
  tirage-collecteur --installer --config FICHIER      (administrateur)
  tirage-collecteur --desinstaller                    (administrateur)
  tirage-collecteur --releve --config FICHIER [--copieur NOM]
  tirage-collecteur --parcourir OID --config FICHIER --copieur NOM [--max N]

--console : surveille le spouleur local, affiche chaque travail terminé (une ligne JSON) et le dépose
            dans la file d'attente (DOSSIER/a-envoyer). Avec --config et une section [serveur], la file est
            envoyée au serveur en continu. Ctrl+C pour arrêter : rien de déposé n'est perdu.
            Les options de la ligne de commande l'emportent sur la configuration.
--installer : enregistre le service Windows « tirage-collecteur » (LocalSystem, démarrage automatique différé,
              redémarrage après échec) avec cette configuration ; --desinstaller l'arrête et le supprime.
--releve    : relève une fois chaque copieur de la configuration (ou celui nommé) et affiche le relevé (JSON).
--parcourir : liste les valeurs SNMP d'un sous-arbre d'un copieur (ex. 1.3.6.1.4.1.<entreprise>), pour écrire
              un profil de marque. Lecture seule ; la communauté vient de la configuration, jamais de la ligne.";

#[derive(Debug, PartialEq, Default)]
struct OptionsConsole {
    config: Option<PathBuf>,
    donnees: Option<PathBuf>,
    collecteur: Option<String>,
    politique: Option<PolitiqueNomDocument>,
}

fn lire_options(args: &[String]) -> Result<OptionsConsole, String> {
    let mut o = OptionsConsole::default();
    let mut i = args.iter();
    while let Some(a) = i.next() {
        let mut valeur = || {
            i.next()
                .cloned()
                .ok_or_else(|| format!("valeur manquante après {a}"))
        };
        match a.as_str() {
            "--config" => o.config = Some(PathBuf::from(valeur()?)),
            "--donnees" => o.donnees = Some(PathBuf::from(valeur()?)),
            "--collecteur" => o.collecteur = Some(valeur()?),
            "--nom-document" => {
                o.politique = Some(match valeur()?.as_str() {
                    "non" => PolitiqueNomDocument::Non,
                    "application" => PolitiqueNomDocument::Application,
                    "complet" => PolitiqueNomDocument::Complet,
                    v => return Err(format!("--nom-document : valeur inconnue « {v} »")),
                })
            }
            autre => return Err(format!("option inconnue : {autre}")),
        }
    }
    Ok(o)
}

fn resoudre(o: OptionsConsole) -> Result<Reglages, String> {
    // Ligne de commande, puis configuration, puis valeurs par défaut.
    let cfg = o.config.as_deref().map(Config::charger).transpose()?;
    let (c_donnees, c_collecteur, c_politique, serveur, snmp, copieurs) = match cfg {
        Some(c) => (
            c.donnees,
            Some(c.collecteur),
            Some(c.nom_document),
            c.serveur,
            c.snmp,
            c.copieurs,
        ),
        None => (None, None, None, None, Default::default(), Vec::new()),
    };
    let collecteur = o
        .collecteur
        .or(c_collecteur)
        .unwrap_or_else(|| "collecteur".into());
    Ok(Reglages {
        donnees: o
            .donnees
            .or(c_donnees)
            .unwrap_or_else(|| PathBuf::from("donnees")),
        collecteur,
        politique: o.politique.or(c_politique).unwrap_or_default(),
        serveur,
        snmp,
        copieurs,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") | Some("-V") => println!("{}", texte_version()),
        Some("--console") => match lire_options(&args[1..]).and_then(resoudre) {
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
        Some("--service") | Some("--installer") => match (args.get(1).map(String::as_str), args.get(2)) {
            (Some("--config"), Some(f)) if args.len() == 3 => terminer(service(&args[0], PathBuf::from(f))),
            _ => terminer(Err(Erreur::Usage(format!("{} --config FICHIER", args[0])))),
        },
        Some("--desinstaller") if args.len() == 1 => terminer(service("--desinstaller", PathBuf::new())),
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
fn console(o: &Reglages) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tirage_collecteur::journal::Journal;

    // Le mode console journalise à l'écran ET dans le fichier du service, pour retrouver un essai.
    let journal = Arc::new(Journal::fichier(&o.chemin_journal(), true)?);
    tirage_collecteur::execution::executer(o, &Arc::new(AtomicBool::new(false)), &journal, true)?;
    Ok(())
}

#[cfg(windows)]
fn service(mode: &str, config: PathBuf) -> Result<(), Erreur> {
    use tirage_collecteur::service;
    match mode {
        "--service" => service::lancer(config),
        "--installer" => service::installer(&config),
        _ => service::desinstaller(),
    }
    .map_err(Erreur::Echec)
}

#[cfg(not(windows))]
fn service(_mode: &str, _config: PathBuf) -> Result<(), Erreur> {
    Err(Erreur::Echec("le service n'existe que sous Windows".into()))
}

#[cfg(not(windows))]
fn console(_o: &Reglages) -> Result<(), Box<dyn std::error::Error>> {
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
        let r = resoudre(lire_options(&[]).unwrap()).unwrap();
        assert_eq!(r.politique, PolitiqueNomDocument::Application);
        assert_eq!(r.donnees, PathBuf::from("donnees"));
        assert!(r.serveur.is_none());
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
        let r = resoudre(o).unwrap();
        assert_eq!(
            (r.donnees, r.collecteur.as_str(), r.politique),
            (PathBuf::from("D"), "impression-1", PolitiqueNomDocument::Complet)
        );
    }

    #[test]
    fn configuration_puis_ligne_de_commande() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("c.toml");
        std::fs::write(
            &f,
            "collecteur = \"impression-1\"\ndonnees = \"D1\"\nnom_document = \"non\"\n[serveur]\nurl = \"https://x\"\njeton = \"j\"\n",
        )
        .unwrap();
        let chemin = f.to_string_lossy().into_owned();
        let r = resoudre(lire_options(&v(&["--config", &chemin])).unwrap()).unwrap();
        assert_eq!(
            (r.collecteur.as_str(), r.politique),
            ("impression-1", PolitiqueNomDocument::Non)
        );
        assert_eq!(r.donnees, PathBuf::from("D1"));
        assert!(r.serveur.is_some());
        let r = resoudre(lire_options(&v(&["--config", &chemin, "--donnees", "D2"])).unwrap()).unwrap();
        assert_eq!(r.donnees, PathBuf::from("D2"), "la ligne de commande l'emporte");
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
