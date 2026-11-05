//! Relevés périodiques des copieurs déclarés : un fil dédié dépose chaque relevé dans la file d'envoi.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tirage_modele::{Enveloppe, Evenement, ReleveCopieur};
use tirage_snmp::{Client, Udp};

use super::{Profils, relever};
use crate::config::{Copieur, Snmp};
use crate::spouleur::temps::maintenant_ms;

const PAS_ATTENTE: Duration = Duration::from_millis(500);

/// Relève tous les copieurs jusqu'à `arret`. Sans profil lisible, journalise et s'arrête : les travaux continuent.
pub fn boucle(
    collecteur: &str,
    snmp: &Snmp,
    copieurs: &[Copieur],
    arret: &AtomicBool,
    deposer: impl FnMut(&[u8]),
    mut journal: impl FnMut(&str),
) {
    let profils = match Profils::charger(snmp.dossier_profils.as_deref()) {
        Ok(p) => p,
        Err(e) => {
            journal(&format!("profils illisibles, relevés arrêtés : {e}"));
            return;
        }
    };
    tourner(
        copieurs,
        snmp.intervalle(),
        arret,
        |c| {
            let udp = Udp::ouvrir(&c.adresse, snmp.delai(), snmp.essais).map_err(|e| e.to_string())?;
            let mut client = Client::nouveau(udp, c.communaute.as_bytes());
            relever(
                &mut client,
                collecteur,
                &c.nom,
                &profils,
                c.profil.as_deref(),
                maintenant_ms(),
            )
            .map(|(r, _)| r)
            .map_err(|e| e.to_string())
        },
        deposer,
        &mut journal,
    );
}

/// Un copieur éteint ne remplit pas le journal : une ligne quand il cesse de répondre, une quand il revient.
fn tourner(
    copieurs: &[Copieur],
    intervalle: Duration,
    arret: &AtomicBool,
    mut lire: impl FnMut(&Copieur) -> Result<ReleveCopieur, String>,
    mut deposer: impl FnMut(&[u8]),
    journal: &mut impl FnMut(&str),
) {
    let mut muets: HashSet<&str> = HashSet::new();
    while !arret.load(Ordering::Relaxed) {
        for c in copieurs {
            if arret.load(Ordering::Relaxed) {
                return;
            }
            match lire(c) {
                Ok(r) => {
                    if muets.remove(c.nom.as_str()) {
                        journal(&format!("{} répond de nouveau", c.nom));
                    }
                    deposer(&Enveloppe::nouvelle(Evenement::Releve(r)).encoder());
                }
                Err(e) => {
                    if muets.insert(c.nom.as_str()) {
                        journal(&format!("{} ne répond pas : {e}", c.nom));
                    }
                }
            }
        }
        let debut = Instant::now();
        while !arret.load(Ordering::Relaxed) && debut.elapsed() < intervalle {
            std::thread::sleep(PAS_ATTENTE.min(intervalle));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    fn copieur(nom: &str) -> Copieur {
        Copieur {
            nom: nom.into(),
            adresse: "192.0.2.1".into(),
            communaute: "secret-exemple".into(),
            profil: None,
        }
    }

    fn releve(nom: &str) -> ReleveCopieur {
        ReleveCopieur {
            collecteur: "c".into(),
            copieur: nom.into(),
            releve_ms: 1,
            modele: None,
            numero_serie: None,
            compteur_total: Some(1),
            compteur_noir: None,
            compteur_couleur: None,
            consommables: Vec::new(),
            etat: None,
            alertes: Vec::new(),
        }
    }

    #[test]
    fn un_copieur_eteint_ne_bloque_pas_les_autres_et_n_inonde_pas_le_journal() {
        let copieurs = [copieur("A"), copieur("ETEINT")];
        let arret = AtomicBool::new(false);
        let tours = Cell::new(0);
        let depots = Cell::new(0);
        let lignes = RefCell::new(Vec::<String>::new());
        tourner(
            &copieurs,
            Duration::from_millis(1),
            &arret,
            |c| {
                if c.nom == "A" {
                    tours.set(tours.get() + 1);
                    return Ok(releve("A"));
                }
                match tours.get() {
                    3 => {
                        arret.store(true, Ordering::Relaxed);
                        Ok(releve("ETEINT"))
                    }
                    _ => Err("pas de réponse".into()),
                }
            },
            |_| depots.set(depots.get() + 1),
            &mut |m| lignes.borrow_mut().push(m.to_string()),
        );
        assert_eq!(tours.get(), 3);
        assert_eq!(depots.get(), 4, "3 relevés de A et celui du retour");
        assert_eq!(
            *lignes.borrow(),
            [
                "ETEINT ne répond pas : pas de réponse",
                "ETEINT répond de nouveau"
            ]
        );
    }

    #[test]
    fn arret_immediat() {
        let arret = AtomicBool::new(true);
        tourner(
            &[copieur("A")],
            Duration::from_secs(3600),
            &arret,
            |_| panic!("aucun relevé après l'arrêt"),
            |_| panic!("aucun dépôt après l'arrêt"),
            &mut |_| {},
        );
    }

    #[test]
    fn un_releve_arrive_encode_pour_la_file() {
        let arret = AtomicBool::new(false);
        let contenu = RefCell::new(Vec::new());
        tourner(
            &[copieur("A")],
            Duration::from_millis(1),
            &arret,
            |_| {
                arret.store(true, Ordering::Relaxed);
                Ok(releve("A"))
            },
            |o| contenu.borrow_mut().extend_from_slice(o),
            &mut |_| {},
        );
        let texte = String::from_utf8(contenu.into_inner()).unwrap();
        assert!(texte.contains("\"type\":\"releve\"") && texte.contains("\"copieur\":\"A\""));
        assert!(!texte.contains("secret-exemple"));
    }
}
