//! Types partagés entre le collecteur et le serveur.
//!
//! Le format d'échange est versionné : un serveur refuse un événement d'une version qu'il ne connaît pas
//! plutôt que de l'interpréter de travers. Les dates sont en millisecondes depuis l'époque Unix, en UTC.

use serde::{Deserialize, Serialize};

mod document;

pub use document::{PolitiqueNomDocument, application_du_titre};

/// Millisecondes Unix → « AAAA-MM-JJTHH:MM:SSZ » (UTC), pour les journaux et l'affichage.
pub fn iso_utc(ms: i64) -> String {
    let s = ms.div_euclid(1000);
    let (jours, reste) = (s.div_euclid(86_400), s.rem_euclid(86_400));
    // « civil from days » algorithme calendaire grégorien.
    let z = jours + 719_468;
    let ere = z.div_euclid(146_097);
    let doe = z - ere * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let jour = doy - (153 * mp + 2) / 5 + 1;
    let mois = if mp < 10 { mp + 3 } else { mp - 9 };
    let an = yoe + ere * 400 + i64::from(mois <= 2);
    format!(
        "{an:04}-{mois:02}-{jour:02}T{:02}:{:02}:{:02}Z",
        reste / 3600,
        reste % 3600 / 60,
        reste % 60
    )
}

/// Version du format d'échange collecteur → serveur.
pub const VERSION_FORMAT: u32 = 1;

/// Mode couleur **demandé au pilote** pour le travail (pas l'analyse du contenu).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModeCouleur {
    Couleur,
    NoirEtBlanc,
    Inconnu,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RectoVerso {
    Recto,
    BordLong,
    BordCourt,
    Inconnu,
}

impl RectoVerso {
    pub fn est_recto_verso(self) -> bool {
        matches!(self, RectoVerso::BordLong | RectoVerso::BordCourt)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Orientation {
    Portrait,
    Paysage,
}

/// Issue d'un travail, vue du spouleur.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Statut {
    Imprime,
    Annule,
    Erreur,
    Inconnu,
}

/// Un travail d'impression, tel que le collecteur l'a lu auprès du spouleur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Travail {
    /// Identifiant du collecteur (configuré, ex. « impression-1 »).
    pub collecteur: String,
    /// Numéro du travail attribué par le spouleur (réutilisé après redémarrage : voir [`Travail::cle`]).
    pub numero: u32,
    /// Nom de la file d'impression (= le copieur, côté serveur d'impression).
    pub file: String,
    pub pilote: Option<String>,
    /// Compte de l'utilisateur, tel que donné par le spouleur (sans domaine si possible).
    pub utilisateur: String,
    /// Poste d'origine.
    pub poste: Option<String>,
    pub envoye_ms: i64,
    pub imprime_ms: Option<i64>,
    /// Pages d'un exemplaire.
    pub pages: u32,
    pub copies: u32,
    /// Feuilles physiques, si le spouleur ou le calcul les donnent.
    pub feuilles: Option<u32>,
    pub couleur: ModeCouleur,
    pub recto_verso: RectoVerso,
    /// Format du papier (nom donné par le pilote, ex. « A4 »).
    pub format: Option<String>,
    pub orientation: Option<Orientation>,
    pub resolution_ppp: Option<u32>,
    pub taille_octets: Option<u64>,
    /// Type de données du spool (ex. « RAW », « NT EMF 1.008 »).
    pub type_donnees: Option<String>,
    pub statut: Statut,
    /// Titre du document — présent seulement si la politique le permet ([`PolitiqueNomDocument`]).
    pub nom_document: Option<String>,
    /// Application d'origine déduite du titre.
    pub application: Option<String>,
}

impl Travail {
    /// Clé unique du travail : le numéro seul ne suffit pas (le spouleur le réutilise).
    pub fn cle(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.collecteur, self.file, self.numero, self.envoye_ms
        )
    }

    /// Pages imprimées, tous exemplaires confondus.
    pub fn pages_totales(&self) -> u64 {
        u64::from(self.pages) * u64::from(self.copies.max(1))
    }

    /// Feuilles : celles fournies, sinon estimées d'après le recto-verso.
    pub fn feuilles_estimees(&self) -> u64 {
        if let Some(f) = self.feuilles {
            return u64::from(f);
        }
        let par_exemplaire = if self.recto_verso.est_recto_verso() {
            u64::from(self.pages).div_ceil(2)
        } else {
            u64::from(self.pages)
        };
        par_exemplaire * u64::from(self.copies.max(1))
    }
}

/// Un consommable d'un copieur (Printer-MIB `prtMarkerSupplies`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Consommable {
    pub description: String,
    /// Niveau restant ; `None` si le copieur ne le donne pas.
    pub niveau: Option<i64>,
    pub capacite: Option<i64>,
}

impl Consommable {
    /// Pourcentage restant, si calculable.
    pub fn pourcentage(&self) -> Option<u8> {
        match (self.niveau, self.capacite) {
            (Some(n), Some(c)) if c > 0 && n >= 0 => Some((n.saturating_mul(100) / c).clamp(0, 100) as u8),
            _ => None,
        }
    }
}

/// Relevé SNMP d'un copieur à un instant donné.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleveCopieur {
    pub collecteur: String,
    /// Nom du copieur dans la configuration.
    pub copieur: String,
    pub releve_ms: i64,
    pub modele: Option<String>,
    pub numero_serie: Option<String>,
    pub compteur_total: Option<u64>,
    pub compteur_noir: Option<u64>,
    pub compteur_couleur: Option<u64>,
    pub consommables: Vec<Consommable>,
    /// État lisible (ex. « prêt », « bourrage »), selon la Printer-MIB.
    pub etat: Option<String>,
    pub alertes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Evenement {
    Travail(Travail),
    Releve(ReleveCopieur),
}

/// Ce qui circule du collecteur au serveur.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Enveloppe {
    pub version: u32,
    pub evenement: Evenement,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ErreurFormat {
    Illisible(String),
    VersionInconnue(u32),
}

impl std::fmt::Display for ErreurFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErreurFormat::Illisible(e) => write!(f, "événement illisible : {e}"),
            ErreurFormat::VersionInconnue(v) => write!(f, "version de format inconnue : {v}"),
        }
    }
}

impl std::error::Error for ErreurFormat {}

impl Enveloppe {
    pub fn nouvelle(evenement: Evenement) -> Self {
        Enveloppe {
            version: VERSION_FORMAT,
            evenement,
        }
    }

    pub fn encoder(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("un événement se sérialise toujours")
    }

    /// Décode en vérifiant la version AVANT d'interpréter le contenu.
    pub fn decoder(octets: &[u8]) -> Result<Self, ErreurFormat> {
        #[derive(Deserialize)]
        struct Tete {
            version: u32,
        }
        let tete: Tete =
            serde_json::from_slice(octets).map_err(|e| ErreurFormat::Illisible(e.to_string()))?;
        if tete.version != VERSION_FORMAT {
            return Err(ErreurFormat::VersionInconnue(tete.version));
        }
        serde_json::from_slice(octets).map_err(|e| ErreurFormat::Illisible(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn travail_exemple() -> Travail {
        Travail {
            collecteur: "impression-1".into(),
            numero: 42,
            file: "COPIEUR-EXEMPLE-1".into(),
            pilote: Some("Pilote universel EXEMPLE".into()),
            utilisateur: "a.martin".into(),
            poste: Some("POSTE-EXEMPLE-07".into()),
            envoye_ms: 1_767_225_600_000,
            imprime_ms: Some(1_767_225_605_000),
            pages: 5,
            copies: 2,
            feuilles: None,
            couleur: ModeCouleur::Couleur,
            recto_verso: RectoVerso::BordLong,
            format: Some("A4".into()),
            orientation: Some(Orientation::Portrait),
            resolution_ppp: Some(600),
            taille_octets: Some(123_456),
            type_donnees: Some("RAW".into()),
            statut: Statut::Imprime,
            nom_document: None,
            application: Some("Microsoft Word".into()),
        }
    }

    #[test]
    fn aller_retour_travail() {
        let e = Enveloppe::nouvelle(Evenement::Travail(travail_exemple()));
        assert_eq!(Enveloppe::decoder(&e.encoder()).unwrap(), e);
    }

    #[test]
    fn aller_retour_releve() {
        let r = ReleveCopieur {
            collecteur: "impression-1".into(),
            copieur: "COPIEUR-EXEMPLE-1".into(),
            releve_ms: 1_767_225_600_000,
            modele: Some("Modèle EXEMPLE".into()),
            numero_serie: Some("SN-000000".into()),
            compteur_total: Some(1000),
            compteur_noir: Some(800),
            compteur_couleur: Some(200),
            consommables: vec![Consommable {
                description: "Toner noir".into(),
                niveau: Some(30),
                capacite: Some(100),
            }],
            etat: Some("prêt".into()),
            alertes: vec![],
        };
        let e = Enveloppe::nouvelle(Evenement::Releve(r));
        assert_eq!(Enveloppe::decoder(&e.encoder()).unwrap(), e);
    }

    #[test]
    fn version_inconnue_refusee_avant_le_contenu() {
        // Contenu volontairement invalide : c'est la version qui doit être signalée.
        let octets = br#"{"version": 99, "evenement": {"type": "inconnu"}}"#;
        assert_eq!(Enveloppe::decoder(octets), Err(ErreurFormat::VersionInconnue(99)));
    }

    #[test]
    fn illisible() {
        assert!(matches!(
            Enveloppe::decoder(b"pas du json"),
            Err(ErreurFormat::Illisible(_))
        ));
    }

    #[test]
    fn pages_et_feuilles() {
        let t = travail_exemple(); // 5 pages, 2 copies, recto-verso
        assert_eq!(t.pages_totales(), 10);
        assert_eq!(t.feuilles_estimees(), 6); // 3 feuilles × 2
        let recto = Travail {
            recto_verso: RectoVerso::Recto,
            ..t.clone()
        };
        assert_eq!(recto.feuilles_estimees(), 10);
        let donne = Travail {
            feuilles: Some(4),
            ..t
        };
        assert_eq!(donne.feuilles_estimees(), 4);
    }

    #[test]
    fn zero_copie_compte_pour_une() {
        let t = Travail {
            copies: 0,
            recto_verso: RectoVerso::Recto,
            ..travail_exemple()
        };
        assert_eq!(t.pages_totales(), 5);
    }

    #[test]
    fn cle_distingue_un_numero_reutilise() {
        let a = travail_exemple();
        let b = Travail {
            envoye_ms: a.envoye_ms + 86_400_000,
            ..a.clone()
        };
        assert_ne!(a.cle(), b.cle());
    }

    #[test]
    fn pourcentage_consommable() {
        let c = |n, cap| Consommable {
            description: "x".into(),
            niveau: n,
            capacite: cap,
        };
        assert_eq!(c(Some(30), Some(100)).pourcentage(), Some(30));
        assert_eq!(c(Some(-3), Some(100)).pourcentage(), None); // -3 = « inconnu » dans la MIB
        assert_eq!(c(Some(5), Some(-2)).pourcentage(), None);
        assert_eq!(c(Some(150), Some(100)).pourcentage(), Some(100));
    }
}
