//! Exécution du collecteur, commune au mode console et au service Windows :
//! surveillance du spouleur → file d'attente → envoi au serveur (fil dédié).

use std::path::PathBuf;

use tirage_modele::PolitiqueNomDocument;

use crate::config::{Config, Copieur, Serveur, Snmp};

/// Événements gardés au plus dans la file (serveur injoignable longtemps).
pub const CAPACITE: usize = 100_000;

/// Réglages effectifs d'une exécution.
#[derive(Debug, Clone)]
pub struct Reglages {
    pub donnees: PathBuf,
    pub collecteur: String,
    pub politique: PolitiqueNomDocument,
    pub serveur: Option<Serveur>,
    pub snmp: Snmp,
    pub copieurs: Vec<Copieur>,
}

impl Reglages {
    pub fn de_config(c: Config) -> Self {
        Reglages {
            donnees: c.donnees.unwrap_or_else(|| PathBuf::from("donnees")),
            collecteur: c.collecteur,
            politique: c.nom_document,
            serveur: c.serveur,
            snmp: c.snmp,
            copieurs: c.copieurs,
        }
    }

    /// Fichier du journal de fonctionnement.
    pub fn chemin_journal(&self) -> PathBuf {
        self.donnees.join("journal").join("collecteur.log")
    }
}

#[cfg(windows)]
pub use windows::executer;

#[cfg(windows)]
mod windows {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use tirage_file::{ErreurFile, FileAttente};
    use tirage_modele::{Enveloppe, Evenement};

    use super::{CAPACITE, Reglages};
    use crate::copieurs::periodique;
    use crate::envoi;
    use crate::journal::Journal;
    use crate::spouleur;

    /// Tourne jusqu'à `arret`. `afficher` : imprime chaque travail en JSON (mode console seulement).
    pub fn executer(
        r: &Reglages,
        arret: &Arc<AtomicBool>,
        journal: &Arc<Journal>,
        afficher: bool,
    ) -> Result<(), String> {
        let file = FileAttente::ouvrir(&r.donnees, CAPACITE).map_err(|e| e.to_string())?;
        journal.ecrire(&format!(
            "démarrage {} — collecteur « {} », file : {} élément(s) en attente",
            env!("CARGO_PKG_VERSION"),
            r.collecteur,
            file.longueur()
        ));
        // La surveillance du spouleur et les relevés déposent dans la même file.
        let file = Arc::new(Mutex::new(file));
        let envoi = match &r.serveur {
            Some(s) => {
                let mut http = envoi::Http::nouveau(s)?;
                journal.ecrire(&format!("envoi au serveur : {}", s.url));
                let (donnees, arret, j) = (r.donnees.clone(), Arc::clone(arret), Arc::clone(journal));
                Some(std::thread::spawn(move || {
                    let res = envoi::boucle(
                        &donnees,
                        CAPACITE,
                        &mut http,
                        envoi::Rythme::default(),
                        &arret,
                        |m| j.ecrire(&format!("envoi : {m}")),
                    );
                    if let Err(e) = res {
                        j.ecrire(&format!("envoi arrêté : {e}"));
                    }
                }))
            }
            None => {
                journal.ecrire("pas de serveur configuré : les travaux restent dans la file");
                None
            }
        };
        let releves = (!r.copieurs.is_empty()).then(|| {
            journal.ecrire(&format!(
                "relevés SNMP : {} copieur(s), toutes les {} min",
                r.copieurs.len(),
                r.snmp.intervalle().as_secs() / 60
            ));
            let (reglages, arret, j, f) = (
                r.clone(),
                Arc::clone(arret),
                Arc::clone(journal),
                Arc::clone(&file),
            );
            std::thread::spawn(move || {
                periodique::boucle(
                    &reglages.collecteur,
                    &reglages.snmp,
                    &reglages.copieurs,
                    &arret,
                    |octets| {
                        let mut file = f.lock().unwrap_or_else(|e| e.into_inner());
                        if let Err(e) = file.deposer(octets) {
                            j.ecrire(&format!("relevé non déposé : {e}"));
                        }
                    },
                    |m| j.ecrire(&format!("relevés : {m}")),
                );
            })
        });
        let mut pleine_signalee = false;
        let res = spouleur::windows::surveiller(arret, |suivi| {
            for brut in suivi.prendre_termines() {
                let Some(t) = spouleur::suivi::convertir(&brut, &r.collecteur, r.politique) else {
                    journal.ecrire(&format!(
                        "travail n° {} inexploitable (sans file ni utilisateur), ignoré",
                        brut.numero
                    ));
                    continue;
                };
                let octets = Enveloppe::nouvelle(Evenement::Travail(t)).encoder();
                if afficher {
                    println!("{}", String::from_utf8_lossy(&octets));
                }
                match file.lock().unwrap_or_else(|e| e.into_inner()).deposer(&octets) {
                    Ok(_) => pleine_signalee = false,
                    Err(ErreurFile::Pleine { capacite }) => {
                        // Une seule ligne par épisode : la collecte continue, le travail est perdu.
                        if !pleine_signalee {
                            journal.ecrire(&format!("file pleine ({capacite}) : travaux perdus tant que le serveur ne vide pas la file"));
                            pleine_signalee = true;
                        }
                    }
                    Err(e) => journal.ecrire(&format!("dépôt impossible : {e}")),
                }
            }
        });
        arret.store(true, Ordering::Relaxed);
        for fil in [envoi, releves].into_iter().flatten() {
            let _ = fil.join();
        }
        match res {
            Ok(()) => {
                journal.ecrire("arrêt");
                Ok(())
            }
            Err(e) => {
                journal.ecrire(&format!("arrêt sur erreur du spouleur : {e}"));
                Err(e.to_string())
            }
        }
    }
}
