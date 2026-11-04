//! Sessions de l'interface (en mémoire : un redémarrage demande de se reconnecter) et limite des essais de connexion.
//!
//! Le jeton n'existe que dans le cookie du navigateur : le serveur n'en garde que l'empreinte.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::config::{empreinte, nouveau_jeton};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub login: String,
    pub nom: Option<String>,
    cree_ms: i64,
    vu_ms: i64,
}

pub struct Sessions {
    actives: Mutex<HashMap<String, Session>>,
    duree_ms: i64,
    inactivite_ms: i64,
}

impl Sessions {
    pub fn nouvelles(duree_minutes: u32, inactivite_minutes: u32) -> Self {
        Sessions {
            actives: Mutex::new(HashMap::new()),
            duree_ms: i64::from(duree_minutes) * 60_000,
            inactivite_ms: i64::from(inactivite_minutes) * 60_000,
        }
    }

    pub fn duree_secondes(&self) -> i64 {
        self.duree_ms / 1000
    }

    fn valide(&self, s: &Session, maintenant: i64) -> bool {
        maintenant - s.cree_ms < self.duree_ms && maintenant - s.vu_ms < self.inactivite_ms
    }

    /// Ouvre une session ; rend le jeton à poser dans le cookie.
    pub fn ouvrir(&self, login: &str, nom: Option<&str>, maintenant: i64) -> Result<String, String> {
        let jeton = nouveau_jeton()?;
        let mut a = self
            .actives
            .lock()
            .map_err(|_| "sessions indisponibles".to_owned())?;
        a.retain(|_, s| self.valide(s, maintenant));
        a.insert(
            empreinte(&jeton),
            Session {
                login: login.to_owned(),
                nom: nom.map(str::to_owned),
                cree_ms: maintenant,
                vu_ms: maintenant,
            },
        );
        Ok(jeton)
    }

    /// Session de ce jeton si elle est encore valide ; compte comme une activité.
    pub fn lire(&self, jeton: &str, maintenant: i64) -> Option<Session> {
        let mut a = self.actives.lock().ok()?;
        let cle = empreinte(jeton);
        if !self.valide(a.get(&cle)?, maintenant) {
            a.remove(&cle);
            return None;
        }
        let s = a.get_mut(&cle)?;
        s.vu_ms = maintenant;
        Some(s.clone())
    }

    pub fn fermer(&self, jeton: &str) {
        if let Ok(mut a) = self.actives.lock() {
            a.remove(&empreinte(jeton));
        }
    }
}

/// Essais de connexion par identifiant, pour ne pas verrouiller le compte dans l'annuaire à force d'échecs.
pub struct Limiteur {
    essais: Mutex<HashMap<String, (u32, i64)>>,
}

const ESSAIS_MAX: u32 = 5;
const FENETRE_MS: i64 = 15 * 60_000;
const IDENTIFIANTS_SUIVIS_MAX: usize = 10_000;

impl Default for Limiteur {
    fn default() -> Self {
        Limiteur {
            essais: Mutex::new(HashMap::new()),
        }
    }
}

impl Limiteur {
    /// Réserve un essai. `false` : trop d'essais récents, ne pas interroger l'annuaire.
    pub fn reserver(&self, login: &str, maintenant: i64) -> bool {
        let Ok(mut e) = self.essais.lock() else {
            return false;
        };
        if e.len() >= IDENTIFIANTS_SUIVIS_MAX {
            e.retain(|_, (_, debut)| maintenant - *debut < FENETRE_MS);
            if e.len() >= IDENTIFIANTS_SUIVIS_MAX {
                return false;
            }
        }
        let n = e.entry(login.to_lowercase()).or_insert((0, maintenant));
        if maintenant - n.1 >= FENETRE_MS {
            *n = (0, maintenant);
        }
        if n.0 >= ESSAIS_MAX {
            return false;
        }
        n.0 += 1;
        true
    }

    /// Connexion réussie : le compteur repart de zéro.
    pub fn effacer(&self, login: &str) {
        if let Ok(mut e) = self.essais.lock() {
            e.remove(&login.to_lowercase());
        }
    }

    /// Annuaire en panne : cet essai ne compte pas contre la personne.
    pub fn rendre(&self, login: &str) {
        if let Ok(mut e) = self.essais.lock()
            && let Some(n) = e.get_mut(&login.to_lowercase())
        {
            n.0 = n.0.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    #[test]
    fn session_ouverte_puis_fermee() {
        let s = Sessions::nouvelles(240, 30);
        let j = s.ouvrir("a.exemple", Some("A"), 0).unwrap();
        assert_eq!(j.len(), 64);
        assert_eq!(s.lire(&j, 1).unwrap().login, "a.exemple");
        assert_eq!(s.lire("autre", 1), None);
        s.fermer(&j);
        assert_eq!(s.lire(&j, 2), None);
    }

    #[test]
    fn expiration_par_inactivite() {
        let s = Sessions::nouvelles(240, 30);
        let j = s.ouvrir("a", None, 0).unwrap();
        assert!(s.lire(&j, 29 * MIN).is_some());
        // L'activité repousse l'inactivité…
        assert!(s.lire(&j, 58 * MIN).is_some());
        // … jusqu'à 30 minutes sans rien.
        assert!(s.lire(&j, 88 * MIN).is_none());
        // Une session expirée ne revient pas.
        assert!(s.lire(&j, 89 * MIN).is_none());
    }

    #[test]
    fn expiration_absolue_malgre_l_activite() {
        let s = Sessions::nouvelles(60, 30);
        let j = s.ouvrir("a", None, 0).unwrap();
        for m in [20, 40, 59] {
            assert!(s.lire(&j, m * MIN).is_some());
        }
        assert!(s.lire(&j, 60 * MIN).is_none());
    }

    #[test]
    fn le_jeton_n_est_pas_garde() {
        let s = Sessions::nouvelles(60, 30);
        let j = s.ouvrir("a", None, 0).unwrap();
        assert!(!s.actives.lock().unwrap().contains_key(&j));
    }

    #[test]
    fn limite_les_essais_sans_tenir_compte_de_la_casse() {
        let l = Limiteur::default();
        for _ in 0..ESSAIS_MAX {
            assert!(l.reserver("a.exemple", 0));
        }
        assert!(!l.reserver("A.EXEMPLE", 1));
        assert!(l.reserver("b.exemple", 1));
        // La fenêtre passée, on peut réessayer.
        assert!(l.reserver("a.exemple", FENETRE_MS));
    }

    #[test]
    fn panne_et_reussite_ne_comptent_pas() {
        let l = Limiteur::default();
        for _ in 0..ESSAIS_MAX {
            assert!(l.reserver("a", 0));
            l.rendre("a");
        }
        for _ in 0..ESSAIS_MAX {
            assert!(l.reserver("a", 0));
        }
        l.effacer("a");
        assert!(l.reserver("a", 0));
    }

    #[test]
    fn nombre_d_identifiants_borne() {
        let l = Limiteur::default();
        for i in 0..IDENTIFIANTS_SUIVIS_MAX {
            assert!(l.reserver(&format!("u{i}"), 0));
        }
        assert!(!l.reserver("nouveau", 1));
        assert!(l.reserver("nouveau", FENETRE_MS));
    }
}
