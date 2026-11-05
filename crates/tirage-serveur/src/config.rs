//! Configuration du serveur (fichier TOML).
//!
//! Les collecteurs s'authentifient par un jeton ; le fichier ne garde que son **empreinte SHA-256**, jamais le jeton.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Adresse d'écoute, ex. « 127.0.0.1:8080 ».
    pub ecoute: String,
    /// Fichier de la base SQLite.
    pub base: PathBuf,
    #[serde(default)]
    pub collecteurs: Vec<Collecteur>,
    /// Annuaire LDAP(S) : sans lui, pas d'interface web (seule la réception des collecteurs tourne).
    pub annuaire: Option<ConfigAnnuaire>,
    #[serde(default)]
    pub interface: ConfigInterface,
    #[serde(default)]
    pub couts: ConfigCouts,
    #[serde(default)]
    pub conservation: ConfigConservation,
}

/// Durées de conservation, en mois glissants.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ConfigConservation {
    /// Détail par utilisateur ; au-delà, totaux mensuels sans nom.
    pub detail_mois: u32,
    /// Journal des consultations.
    pub journal_mois: u32,
}

impl Default for ConfigConservation {
    fn default() -> Self {
        ConfigConservation {
            detail_mois: 12,
            journal_mois: 12,
        }
    }
}

/// Prix par page. Une file sans tarif propre prend le tarif général ; sans tarif du tout, ses pages sont signalées.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ConfigCouts {
    pub devise: String,
    pub noir: Option<f64>,
    pub couleur: Option<f64>,
    pub files: Vec<TarifFile>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TarifFile {
    pub file: String,
    /// Sans prix propre, le tarif général s'applique.
    pub noir: Option<f64>,
    pub couleur: Option<f64>,
}

impl Default for ConfigCouts {
    fn default() -> Self {
        ConfigCouts {
            devise: "€".into(),
            noir: None,
            couleur: None,
            files: Vec::new(),
        }
    }
}

impl ConfigCouts {
    /// Prix d'une page, en cent-millièmes d'unité monétaire. Le mode inconnu suit le tarif noir.
    pub fn prix(&self, file: &str, couleur: bool) -> Option<i64> {
        let propre = self.files.iter().find(|t| t.file.eq_ignore_ascii_case(file));
        let p = if couleur {
            propre.and_then(|t| t.couleur).or(self.couleur)
        } else {
            propre.and_then(|t| t.noir).or(self.noir)
        };
        p.map(|x| (x * 100_000.0).round() as i64)
    }

    /// Vrai pour un prix par page recevable.
    pub fn prix_correct(x: f64) -> bool {
        x.is_finite() && (0.0..=1000.0).contains(&x)
    }

    fn valider(&self) -> Result<(), String> {
        let generaux = [self.noir, self.couleur].into_iter().flatten();
        let par_file = self.files.iter().flat_map(|t| [t.noir, t.couleur]).flatten();
        if !generaux.chain(par_file).all(ConfigCouts::prix_correct) {
            return Err("couts : un prix doit être un nombre entre 0 et 1000".into());
        }
        Ok(())
    }
}

/// Lecture seule de l'annuaire : rattachement des personnes et connexion réservée à un groupe.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigAnnuaire {
    /// `ldaps://…` ; `ldap://` est refusé sauf `ldap_en_clair = true` (essai seulement).
    pub url: String,
    #[serde(default)]
    pub ldap_en_clair: bool,
    /// Autorité de certification (PEM) ; à défaut, le magasin du système.
    pub ca: Option<PathBuf>,
    /// Racine des recherches.
    pub base: String,
    /// Compte de lecture (DN) et fichier contenant son mot de passe (droits 600 ; jamais dans ce fichier-ci).
    pub compte: String,
    pub mot_de_passe_fichier: PathBuf,
    /// DN du groupe autorisé à se connecter (membres directs et groupes imbriqués).
    pub groupe_autorise: String,
    #[serde(default = "defaut_filtre")]
    pub filtre_personnes: String,
    #[serde(default = "defaut_identifiant")]
    pub attribut_identifiant: String,
    #[serde(default = "defaut_nom")]
    pub attribut_nom: String,
    /// Attribut LDAP, ou `dn:ou` pour la première unité d'organisation du DN.
    #[serde(default = "defaut_service")]
    pub attribut_service: String,
    /// Attribut LDAP, ou `dn:ou`.
    #[serde(default = "defaut_cabinet")]
    pub attribut_cabinet: String,
    /// Âge maximal d'une fiche avant de la relire dans l'annuaire.
    #[serde(default = "defaut_rafraichissement")]
    pub rafraichissement_minutes: u32,
}

fn defaut_filtre() -> String {
    "(&(objectCategory=person)(objectClass=user))".into()
}
fn defaut_identifiant() -> String {
    "sAMAccountName".into()
}
fn defaut_nom() -> String {
    "displayName".into()
}
fn defaut_service() -> String {
    "department".into()
}
fn defaut_cabinet() -> String {
    "company".into()
}
fn defaut_rafraichissement() -> u32 {
    60
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ConfigInterface {
    /// Durée maximale d'une session, puis durée d'inactivité qui la ferme.
    pub session_minutes: u32,
    pub inactivite_minutes: u32,
    /// Retire l'attribut `Secure` du cookie : essai sans HTTPS seulement.
    pub http_en_clair: bool,
    /// Fuseau des dates affichées et des bornes de période (nom IANA, ex. « Europe/Paris »).
    pub fuseau: String,
    /// Un travail en couleur est signalé à partir de ce nombre de pages (copies comprises).
    pub gros_travail_couleur_pages: u32,
    /// Temps ouvré sans aucun travail reçu au bout duquel la collecte est signalée comme muette.
    pub collecte_silence_minutes: u32,
    /// Heures ouvrées, du lundi au vendredi, dans le fuseau : début inclus, fin exclue.
    pub heure_ouverture: u8,
    pub heure_fermeture: u8,
}

impl Default for ConfigInterface {
    fn default() -> Self {
        ConfigInterface {
            session_minutes: 240,
            inactivite_minutes: 30,
            http_en_clair: false,
            fuseau: "UTC".into(),
            gros_travail_couleur_pages: 100,
            collecte_silence_minutes: 120,
            heure_ouverture: 8,
            heure_fermeture: 18,
        }
    }
}

/// Nom d'attribut LDAP valide, ou la forme `dn:ou`.
fn attribut_valide(a: &str, dn_permis: bool) -> bool {
    (dn_permis && a == "dn:ou")
        || (!a.is_empty()
            && a.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b';'))
}

impl ConfigAnnuaire {
    fn valider(&self) -> Result<(), String> {
        if !(self.url.starts_with("ldaps://") || self.ldap_en_clair && self.url.starts_with("ldap://")) {
            return Err(
                "annuaire : l'url doit commencer par ldaps:// (ldap:// exige ldap_en_clair = true)".into(),
            );
        }
        for (nom, v) in [
            ("base", &self.base),
            ("compte", &self.compte),
            ("groupe_autorise", &self.groupe_autorise),
        ] {
            if v.trim().is_empty() {
                return Err(format!("annuaire : {nom} est vide"));
            }
        }
        if !(self.filtre_personnes.starts_with('(') && self.filtre_personnes.ends_with(')')) {
            return Err("annuaire : filtre_personnes doit être un filtre LDAP entre parenthèses".into());
        }
        let attributs = [
            ("attribut_identifiant", &self.attribut_identifiant, false),
            ("attribut_nom", &self.attribut_nom, false),
            ("attribut_service", &self.attribut_service, true),
            ("attribut_cabinet", &self.attribut_cabinet, true),
        ];
        for (nom, v, dn_permis) in attributs {
            if !attribut_valide(v, dn_permis) {
                return Err(format!("annuaire : {nom} invalide"));
            }
        }
        if self.rafraichissement_minutes == 0 {
            return Err("annuaire : rafraichissement_minutes doit être positif".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collecteur {
    pub nom: String,
    /// Empreinte SHA-256 du jeton, en hexadécimal (64 caractères).
    pub jeton_sha256: String,
}

pub fn empreinte(jeton: &str) -> String {
    hex(&Sha256::digest(jeton.as_bytes()))
}

fn hex(octets: &[u8]) -> String {
    octets.iter().map(|b| format!("{b:02x}")).collect()
}

/// Nouveau jeton aléatoire (256 bits), en hexadécimal.
pub fn nouveau_jeton() -> Result<String, String> {
    let mut o = [0u8; 32];
    getrandom::fill(&mut o).map_err(|e| format!("générateur aléatoire : {e}"))?;
    Ok(hex(&o))
}

/// Comparaison en temps constant (les empreintes ont la même longueur).
fn egales(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Config {
    pub fn lire(texte: &str) -> Result<Self, String> {
        let c: Config = toml::from_str(texte).map_err(|e| e.to_string())?;
        let mut noms = std::collections::BTreeSet::new();
        for col in &c.collecteurs {
            if col.nom.trim().is_empty() {
                return Err("un collecteur sans nom".into());
            }
            if col.jeton_sha256.len() != 64 || !col.jeton_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!(
                    "collecteur {} : jeton_sha256 doit faire 64 caractères hexadécimaux",
                    col.nom
                ));
            }
            if !noms.insert(col.nom.as_str()) {
                return Err(format!("collecteur en double : {}", col.nom));
            }
        }
        if let Some(a) = &c.annuaire {
            a.valider()?;
        }
        if c.interface.session_minutes == 0 || c.interface.inactivite_minutes == 0 {
            return Err("interface : les durées de session doivent être positives".into());
        }
        if c.interface.gros_travail_couleur_pages == 0 || c.interface.collecte_silence_minutes == 0 {
            return Err(
                "interface : gros_travail_couleur_pages et collecte_silence_minutes doivent être positifs"
                    .into(),
            );
        }
        if c.interface.heure_ouverture >= c.interface.heure_fermeture || c.interface.heure_fermeture > 24 {
            return Err("interface : heures ouvrées invalides".into());
        }
        if jiff::tz::TimeZone::get(&c.interface.fuseau).is_err() {
            return Err(format!("interface : fuseau inconnu : {}", c.interface.fuseau));
        }
        c.couts.valider()?;
        let durees = [c.conservation.detail_mois, c.conservation.journal_mois];
        if durees.iter().any(|m| !(1..=120).contains(m)) {
            return Err("conservation : une durée doit être comprise entre 1 et 120 mois".into());
        }
        Ok(c)
    }

    pub fn charger(chemin: &Path) -> Result<Self, String> {
        let texte = std::fs::read_to_string(chemin).map_err(|e| format!("{} : {e}", chemin.display()))?;
        Config::lire(&texte).map_err(|e| format!("{} : {e}", chemin.display()))
    }

    /// Nom du collecteur dont c'est le jeton.
    pub fn authentifier(&self, jeton: &str) -> Option<&str> {
        let e = empreinte(jeton);
        // On parcourt tout, sans s'arrêter au premier : même durée quel que soit le résultat.
        let mut trouve = None;
        for c in &self.collecteurs {
            if egales(&c.jeton_sha256.to_ascii_lowercase(), &e) {
                trouve = Some(c.nom.as_str());
            }
        }
        trouve
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exemple_fourni_valide() {
        let c = Config::lire(include_str!("../serveur.exemple.toml")).unwrap();
        assert_eq!(c.collecteurs.len(), 1);
        assert!(c.annuaire.as_ref().unwrap().url.starts_with("ldaps://"));
        assert!(!c.interface.http_en_clair);
        assert_eq!(c.authentifier("jeton-exemple-a-remplacer"), Some("impression-1"));
        assert_eq!(c.authentifier("autre"), None);
    }

    #[test]
    fn jetons() {
        let j = nouveau_jeton().unwrap();
        assert_eq!(j.len(), 64);
        assert_ne!(j, nouveau_jeton().unwrap());
        assert_eq!(
            empreinte("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn configuration_refusee() {
        let base = "ecoute = \"127.0.0.1:0\"\nbase = \"b.sqlite\"\n";
        assert!(
            Config::lire(&format!(
                "{base}[[collecteurs]]\nnom = \"a\"\njeton_sha256 = \"court\"\n"
            ))
            .is_err()
        );
        let e = empreinte("x");
        let double = format!(
            "{base}[[collecteurs]]\nnom = \"a\"\njeton_sha256 = \"{e}\"\n[[collecteurs]]\nnom = \"a\"\njeton_sha256 = \"{e}\"\n"
        );
        assert!(Config::lire(&double).is_err());
        assert!(Config::lire(&format!("{base}inconnu = 1\n")).is_err());
    }

    #[test]
    fn annuaire_refuse() {
        let avec = |url: &str, plus: &str| {
            Config::lire(&format!(
                "ecoute = \"x\"\nbase = \"b\"\n[annuaire]\nurl = \"{url}\"\nbase = \"DC=example,DC=org\"\ncompte = \"CN=lecture\"\nmot_de_passe_fichier = \"f\"\ngroupe_autorise = \"CN=g\"\n{plus}"
            ))
        };
        assert!(avec("ldaps://dc.example.org", "").is_ok());
        assert!(avec("ldap://dc.example.org", "").is_err());
        assert!(avec("ldap://dc.example.org", "ldap_en_clair = true\n").is_ok());
        assert!(avec("http://dc.example.org", "ldap_en_clair = true\n").is_err());
        assert!(avec("ldaps://d", "attribut_nom = \"a)(b\"\n").is_err());
        assert!(avec("ldaps://d", "attribut_nom = \"dn:ou\"\n").is_err());
        assert!(avec("ldaps://d", "attribut_service = \"dn:ou\"\n").is_ok());
        assert!(avec("ldaps://d", "filtre_personnes = \"x=y\"\n").is_err());
    }

    #[test]
    fn couts_et_fuseau() {
        let base = "ecoute = \"x\"\nbase = \"b\"\n";
        assert!(Config::lire(&format!("{base}[couts]\nnoir = -1.0\n")).is_err());
        assert!(Config::lire(&format!("{base}[conservation]\ndetail_mois = 0\n")).is_err());
        assert!(Config::lire(&format!("{base}[interface]\ngros_travail_couleur_pages = 0\n")).is_err());
        assert!(Config::lire(&format!("{base}[interface]\ncollecte_silence_minutes = 0\n")).is_err());
        assert!(
            Config::lire(&format!(
                "{base}[interface]\nheure_ouverture = 18\nheure_fermeture = 8\n"
            ))
            .is_err()
        );
        assert!(Config::lire(&format!("{base}[interface]\nheure_fermeture = 25\n")).is_err());
        assert!(Config::lire(&format!("{base}[interface]\nfuseau = \"Lune/Base\"\n")).is_err());
        let c = Config::lire(&format!(
            "{base}[interface]\nfuseau = \"Europe/Paris\"\n[couts]\nnoir = 0.01\n[[couts.files]]\nfile = \"F\"\nnoir = 0.02\ncouleur = 0.2\n"
        ))
        .unwrap();
        assert_eq!(c.couts.prix("f", true), Some(20_000));
        assert_eq!(c.couts.prix("autre", false), Some(1000));
        assert_eq!(c.couts.prix("autre", true), None);
    }
}
