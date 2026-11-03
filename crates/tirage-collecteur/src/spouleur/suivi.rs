//! Suivi des travaux à partir des notifications du spouleur — partie PORTABLE, testée sur toute plateforme.
//!
//! Le spouleur envoie des mises à jour champ par champ ; un travail n'est complet qu'à sa sortie de la file.
//! On accumule donc les champs par numéro de travail et on « termine » le travail quand il quitte la file
//! (statut supprimé, absent d'un rafraîchissement, ou inactif trop longtemps après impression).

use std::collections::HashMap;

use tirage_modele::{ModeCouleur, Orientation, PolitiqueNomDocument, RectoVerso, Statut, Travail};

/// Bits de statut d'un travail (API du spouleur, `JOB_STATUS_*`).
pub mod bits {
    pub const ERROR: u32 = 0x0000_0002;
    pub const DELETING: u32 = 0x0000_0004;
    pub const PRINTED: u32 = 0x0000_0080;
    pub const DELETED: u32 = 0x0000_0100;
    pub const COMPLETE: u32 = 0x0000_1000;
}

/// Réglages du travail lus dans la DEVMODE (valeurs brutes de l'API).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reglages {
    pub copies: Option<u32>,
    /// `dmColor` : 1 = monochrome, 2 = couleur.
    pub couleur: Option<i16>,
    /// `dmDuplex` : 1 = recto, 2 = bord long (vertical), 3 = bord court (horizontal).
    pub duplex: Option<i16>,
    /// `dmFormName`, sinon déduit de `dmPaperSize`.
    pub format: Option<String>,
    pub format_code: Option<i16>,
    /// `dmOrientation` : 1 = portrait, 2 = paysage.
    pub orientation: Option<i16>,
    /// `dmPrintQuality` si positif (ppp), sinon `dmYResolution`.
    pub resolution: Option<u32>,
}

/// Un champ reçu du spouleur pour un travail.
#[derive(Debug, Clone, PartialEq)]
pub enum Champ {
    File(String),
    Poste(String),
    Utilisateur(String),
    Document(String),
    TypeDonnees(String),
    Pilote(String),
    Statut(u32),
    PagesTotales(u32),
    PagesImprimees(u32),
    Octets(u64),
    Envoye(i64),
    Reglages(Reglages),
}

/// Ce que l'on sait d'un travail encore suivi.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TravailBrut {
    pub numero: u32,
    pub file: Option<String>,
    pub poste: Option<String>,
    pub utilisateur: Option<String>,
    pub document: Option<String>,
    pub type_donnees: Option<String>,
    pub pilote: Option<String>,
    /// Union des bits de statut vus (un bit « imprimé » vu une fois reste acquis).
    pub statut: u32,
    pub pages_totales: u32,
    pub pages_imprimees: u32,
    pub octets: Option<u64>,
    pub envoye_ms: Option<i64>,
    pub reglages: Reglages,
    pub vu_ms: i64,
}

/// Accumulateur des travaux en cours.
#[derive(Debug, Default)]
pub struct Suivi {
    en_cours: HashMap<u32, TravailBrut>,
    termines: Vec<TravailBrut>,
}

/// Délai au-delà duquel un travail imprimé mais jamais vu sortir est considéré terminé.
pub const INACTIVITE_MS: i64 = 10 * 60 * 1000;

impl Suivi {
    pub fn appliquer(&mut self, numero: u32, champ: Champ, maintenant_ms: i64) {
        let t = self.en_cours.entry(numero).or_insert_with(|| TravailBrut {
            numero,
            ..Default::default()
        });
        t.vu_ms = maintenant_ms;
        match champ {
            Champ::File(v) => t.file = non_vide(v),
            Champ::Poste(v) => t.poste = non_vide(v),
            Champ::Utilisateur(v) => t.utilisateur = non_vide(v),
            Champ::Document(v) => t.document = non_vide(v),
            Champ::TypeDonnees(v) => t.type_donnees = non_vide(v),
            Champ::Pilote(v) => t.pilote = non_vide(v),
            Champ::Statut(s) => t.statut |= s,
            Champ::PagesTotales(p) => t.pages_totales = t.pages_totales.max(p),
            Champ::PagesImprimees(p) => t.pages_imprimees = t.pages_imprimees.max(p),
            Champ::Octets(o) => t.octets = Some(o),
            Champ::Envoye(ms) => t.envoye_ms = Some(ms),
            Champ::Reglages(r) => t.reglages = r,
        }
        if t.statut & bits::DELETED != 0 {
            self.terminer(numero);
        }
    }

    /// Le spouleur signale qu'un travail a quitté la file.
    pub fn retire(&mut self, numero: u32) {
        self.terminer(numero);
    }

    /// Après un rafraîchissement complet : tout travail absent de la liste a quitté la file.
    pub fn rafraichi(&mut self, presents: &[u32]) {
        let partis: Vec<u32> = self
            .en_cours
            .keys()
            .filter(|n| !presents.contains(n))
            .copied()
            .collect();
        for n in partis {
            self.terminer(n);
        }
    }

    /// Termine les travaux imprimés restés sans nouvelles (sortie de file manquée).
    pub fn balayer(&mut self, maintenant_ms: i64) {
        let vieux: Vec<u32> = self
            .en_cours
            .values()
            .filter(|t| {
                maintenant_ms - t.vu_ms > INACTIVITE_MS && t.statut & (bits::PRINTED | bits::COMPLETE) != 0
            })
            .map(|t| t.numero)
            .collect();
        for n in vieux {
            self.terminer(n);
        }
    }

    pub fn en_cours(&self) -> usize {
        self.en_cours.len()
    }

    /// Récupère les travaux terminés depuis le dernier appel.
    pub fn prendre_termines(&mut self) -> Vec<TravailBrut> {
        std::mem::take(&mut self.termines)
    }

    fn terminer(&mut self, numero: u32) {
        if let Some(t) = self.en_cours.remove(&numero) {
            self.termines.push(t);
        }
    }
}

fn non_vide(s: String) -> Option<String> {
    let s = s.trim().to_owned();
    (!s.is_empty()).then_some(s)
}

/// Retire le préfixe de domaine éventuel (« DOMAINE\compte » → « compte »).
pub fn compte_sans_domaine(u: &str) -> &str {
    u.rsplit_once('\\').map_or(u, |(_, c)| c)
}

/// Retire les barres obliques inverses de tête d'un nom de poste (« \\POSTE » → « POSTE »).
pub fn poste_propre(p: &str) -> &str {
    p.trim_start_matches('\\')
}

/// Nom lisible d'un format papier à partir de `dmPaperSize` (codes les plus courants).
pub fn format_du_code(code: i16) -> Option<&'static str> {
    Some(match code {
        1 => "Letter",
        5 => "Legal",
        8 => "A3",
        9 => "A4",
        11 => "A5",
        12 => "B4",
        13 => "B5",
        70 => "A6",
        _ => return None,
    })
}

/// Convertit un travail terminé vers le format d'échange. `None` si le travail est inexploitable
/// (sans file ni utilisateur — ne devrait pas arriver avec un spouleur sain).
pub fn convertir(b: &TravailBrut, collecteur: &str, politique: PolitiqueNomDocument) -> Option<Travail> {
    let file = b.file.clone()?;
    let utilisateur = compte_sans_domaine(b.utilisateur.as_deref()?).to_owned();
    let r = &b.reglages;
    let statut = if b.statut & (bits::PRINTED | bits::COMPLETE) != 0 {
        Statut::Imprime
    } else if b.statut & bits::ERROR != 0 {
        Statut::Erreur
    } else if b.statut & (bits::DELETED | bits::DELETING) != 0 {
        Statut::Annule
    } else {
        Statut::Inconnu
    };
    let (nom_document, application) = politique.appliquer(b.document.as_deref());
    Some(Travail {
        collecteur: collecteur.to_owned(),
        numero: b.numero,
        file,
        pilote: b.pilote.clone(),
        utilisateur,
        poste: b
            .poste
            .as_deref()
            .map(poste_propre)
            .map(str::to_owned)
            .filter(|p| !p.is_empty()),
        envoye_ms: b.envoye_ms.unwrap_or(b.vu_ms),
        imprime_ms: (statut == Statut::Imprime).then_some(b.vu_ms),
        pages: b.pages_totales.max(b.pages_imprimees),
        copies: r.copies.unwrap_or(1).max(1),
        feuilles: None,
        couleur: match r.couleur {
            Some(2) => ModeCouleur::Couleur,
            Some(1) => ModeCouleur::NoirEtBlanc,
            _ => ModeCouleur::Inconnu,
        },
        recto_verso: match r.duplex {
            Some(1) => RectoVerso::Recto,
            Some(2) => RectoVerso::BordLong,
            Some(3) => RectoVerso::BordCourt,
            _ => RectoVerso::Inconnu,
        },
        format: r
            .format
            .clone()
            .or_else(|| r.format_code.and_then(format_du_code).map(str::to_owned)),
        orientation: match r.orientation {
            Some(1) => Some(Orientation::Portrait),
            Some(2) => Some(Orientation::Paysage),
            _ => None,
        },
        resolution_ppp: r.resolution.filter(|&d| d > 0),
        taille_octets: b.octets,
        type_donnees: b.type_donnees.clone(),
        statut,
        nom_document,
        application,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alimenter(s: &mut Suivi, n: u32, t: i64) {
        s.appliquer(n, Champ::File("COPIEUR-EXEMPLE-1".into()), t);
        s.appliquer(n, Champ::Utilisateur("EXEMPLE\\a.martin".into()), t);
        s.appliquer(n, Champ::Poste("\\\\POSTE-EXEMPLE-07".into()), t);
        s.appliquer(n, Champ::Document("Microsoft Word - Note EXEMPLE.docx".into()), t);
        s.appliquer(n, Champ::Envoye(1_000), t);
        s.appliquer(
            n,
            Champ::Reglages(Reglages {
                copies: Some(2),
                couleur: Some(2),
                duplex: Some(2),
                format: None,
                format_code: Some(9),
                orientation: Some(1),
                resolution: Some(600),
            }),
            t,
        );
        s.appliquer(n, Champ::PagesTotales(3), t);
    }

    #[test]
    fn travail_complet_termine_a_la_suppression() {
        let mut s = Suivi::default();
        alimenter(&mut s, 7, 10);
        s.appliquer(7, Champ::Statut(bits::PRINTED), 20);
        assert!(s.prendre_termines().is_empty(), "pas encore sorti de la file");
        s.appliquer(7, Champ::Statut(bits::DELETED), 30);
        let fini = s.prendre_termines();
        assert_eq!(fini.len(), 1);
        assert_eq!(s.en_cours(), 0);
        let t = convertir(&fini[0], "impression-1", PolitiqueNomDocument::Complet).unwrap();
        assert_eq!(t.utilisateur, "a.martin");
        assert_eq!(t.poste.as_deref(), Some("POSTE-EXEMPLE-07"));
        assert_eq!(
            t.statut,
            Statut::Imprime,
            "le bit « imprimé » vu avant la suppression reste acquis"
        );
        assert_eq!((t.pages, t.copies, t.pages_totales()), (3, 2, 6));
        assert_eq!(t.couleur, ModeCouleur::Couleur);
        assert_eq!(t.recto_verso, RectoVerso::BordLong);
        assert_eq!(t.format.as_deref(), Some("A4"));
        assert_eq!(t.orientation, Some(Orientation::Portrait));
        assert_eq!(
            t.nom_document.as_deref(),
            Some("Microsoft Word - Note EXEMPLE.docx")
        );
        assert_eq!(t.application.as_deref(), Some("Microsoft Word"));
        assert_eq!(t.envoye_ms, 1_000);
    }

    #[test]
    fn annule_sans_impression() {
        let mut s = Suivi::default();
        alimenter(&mut s, 8, 10);
        s.appliquer(8, Champ::Statut(bits::DELETING), 20);
        s.retire(8);
        let t = convertir(&s.prendre_termines()[0], "c", PolitiqueNomDocument::Application).unwrap();
        assert_eq!(t.statut, Statut::Annule);
        assert_eq!(t.nom_document, None, "politique « application » : pas de titre");
        assert_eq!(t.application.as_deref(), Some("Microsoft Word"));
    }

    #[test]
    fn politique_non_ne_garde_rien() {
        let mut s = Suivi::default();
        alimenter(&mut s, 9, 10);
        s.retire(9);
        let t = convertir(&s.prendre_termines()[0], "c", PolitiqueNomDocument::Non).unwrap();
        assert_eq!((t.nom_document, t.application), (None, None));
    }

    #[test]
    fn rafraichissement_termine_les_absents() {
        let mut s = Suivi::default();
        alimenter(&mut s, 1, 10);
        alimenter(&mut s, 2, 10);
        s.rafraichi(&[2]);
        let fini = s.prendre_termines();
        assert_eq!(fini.iter().map(|t| t.numero).collect::<Vec<_>>(), vec![1]);
        assert_eq!(s.en_cours(), 1);
    }

    #[test]
    fn balayage_seulement_des_imprimes_inactifs() {
        let mut s = Suivi::default();
        alimenter(&mut s, 1, 0);
        s.appliquer(1, Champ::Statut(bits::PRINTED), 0);
        alimenter(&mut s, 2, 0); // en attente (copieur hors ligne par ex.) : on le garde
        s.balayer(INACTIVITE_MS + 1);
        assert_eq!(
            s.prendre_termines().iter().map(|t| t.numero).collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(s.en_cours(), 1);
    }

    #[test]
    fn pages_ne_reculent_pas() {
        let mut s = Suivi::default();
        alimenter(&mut s, 3, 0);
        s.appliquer(3, Champ::PagesImprimees(5), 0);
        s.appliquer(3, Champ::PagesTotales(0), 0); // certains pilotes remettent 0 en fin de travail
        s.retire(3);
        let t = convertir(&s.prendre_termines()[0], "c", PolitiqueNomDocument::Non).unwrap();
        assert_eq!(t.pages, 5);
    }

    #[test]
    fn inexploitable_sans_file_ou_utilisateur() {
        let b = TravailBrut {
            numero: 1,
            ..Default::default()
        };
        assert!(convertir(&b, "c", PolitiqueNomDocument::Non).is_none());
    }

    #[test]
    fn valeurs_devmode_inconnues() {
        let b = TravailBrut {
            numero: 1,
            file: Some("F".into()),
            utilisateur: Some("u".into()),
            reglages: Reglages {
                couleur: Some(0),
                duplex: Some(9),
                format_code: Some(999),
                ..Default::default()
            },
            ..Default::default()
        };
        let t = convertir(&b, "c", PolitiqueNomDocument::Non).unwrap();
        assert_eq!(
            (t.couleur, t.recto_verso, t.format, t.copies),
            (ModeCouleur::Inconnu, RectoVerso::Inconnu, None, 1)
        );
    }
}
