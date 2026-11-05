//! Configuration du collecteur (fichier TOML sur le serveur d'impression).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use tirage_modele::PolitiqueNomDocument;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub collecteur: String,
    /// Dossier de la file d'attente (sous-dossiers `a-envoyer` et `rebut`).
    pub donnees: Option<PathBuf>,
    /// Ce qui est gardé du titre des documents : `non`, `application` (défaut) ou `complet`.
    #[serde(default)]
    pub nom_document: PolitiqueNomDocument,
    pub serveur: Option<Serveur>,
    #[serde(default)]
    pub snmp: Snmp,
    #[serde(default)]
    pub copieurs: Vec<Copieur>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Snmp {
    pub delai_ms: u64,
    pub essais: u32,
    pub dossier_profils: Option<PathBuf>,
    /// Temps entre deux relevés des copieurs déclarés, par le service ou la console.
    pub intervalle_minutes: u64,
}

impl Default for Snmp {
    fn default() -> Self {
        Snmp {
            delai_ms: 2000,
            essais: 2,
            dossier_profils: None,
            intervalle_minutes: 60,
        }
    }
}

impl Snmp {
    pub fn delai(&self) -> Duration {
        Duration::from_millis(self.delai_ms.clamp(100, 30_000))
    }

    pub fn intervalle(&self) -> Duration {
        Duration::from_secs(self.intervalle_minutes.clamp(5, 1440) * 60)
    }
}

#[derive(Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Serveur {
    /// Adresse du serveur, ex. « https://tirage.example.org ».
    pub url: String,
    /// Jeton du collecteur (fabriqué par `tirage-serveur --nouveau-jeton`).
    pub jeton: String,
    /// Certificat de l'autorité (PEM) si le serveur n'est pas reconnu par le magasin du système.
    pub ca: Option<PathBuf>,
    /// Autorise `http://` (essai seulement : les travaux sont des données personnelles).
    #[serde(default)]
    pub http_en_clair: bool,
}

// Le jeton ne doit jamais apparaître dans un journal ou une trace.
impl std::fmt::Debug for Serveur {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Serveur")
            .field("url", &self.url)
            .field("jeton", &"‹masqué›")
            .field("ca", &self.ca)
            .field("http_en_clair", &self.http_en_clair)
            .finish()
    }
}

#[derive(Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Copieur {
    pub nom: String,
    /// « hôte » ou « hôte:port ».
    pub adresse: String,
    pub communaute: String,
    pub profil: Option<String>,
}

// La communauté SNMP ne doit jamais apparaître dans un journal ou une trace.
impl std::fmt::Debug for Copieur {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Copieur")
            .field("nom", &self.nom)
            .field("adresse", &self.adresse)
            .field("communaute", &"‹masquée›")
            .field("profil", &self.profil)
            .finish()
    }
}

impl Config {
    pub fn lire(texte: &str) -> Result<Self, String> {
        let c: Config = toml::from_str(texte).map_err(|e| e.to_string())?;
        if c.collecteur.trim().is_empty() {
            return Err("« collecteur » est vide".into());
        }
        let mut noms = std::collections::BTreeSet::new();
        for cp in &c.copieurs {
            if cp.nom.trim().is_empty() || cp.adresse.trim().is_empty() {
                return Err("un copieur sans nom ou sans adresse".into());
            }
            if !noms.insert(cp.nom.as_str()) {
                return Err(format!("copieur en double : {}", cp.nom));
            }
        }
        Ok(c)
    }

    pub fn charger(chemin: &Path) -> Result<Self, String> {
        let texte = std::fs::read_to_string(chemin).map_err(|e| format!("{} : {e}", chemin.display()))?;
        Config::lire(&texte).map_err(|e| format!("{} : {e}", chemin.display()))
    }

    pub fn copieur(&self, nom: &str) -> Option<&Copieur> {
        self.copieurs.iter().find(|c| c.nom == nom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exemple_fourni_valide() {
        let c = Config::lire(include_str!("../collecteur.exemple.toml")).unwrap();
        assert_eq!(c.collecteur, "impression-1");
        assert_eq!(c.copieurs.len(), 2);
        assert_eq!(c.snmp.delai(), Duration::from_millis(2000));
        assert!(c.copieur("COPIEUR-EXEMPLE-2").is_some());
    }

    #[test]
    fn communaute_jamais_affichee() {
        let c = Config::lire(
            "collecteur = \"c\"\n[[copieurs]]\nnom = \"A\"\nadresse = \"192.0.2.1\"\ncommunaute = \"secret-exemple\"\n",
        )
        .unwrap();
        assert!(!format!("{c:?}").contains("secret-exemple"));
        let s =
            Config::lire("collecteur = \"c\"\n[serveur]\nurl = \"https://x\"\njeton = \"jeton-exemple\"\n")
                .unwrap();
        assert!(!format!("{s:?}").contains("jeton-exemple"));
        assert_eq!(s.nom_document, PolitiqueNomDocument::Application);
    }

    #[test]
    fn erreurs_de_configuration() {
        assert!(Config::lire("collecteur = \"\"\n").is_err());
        assert!(Config::lire("collecteur = \"c\"\ninconnu = 1\n").is_err());
        let double = "collecteur = \"c\"\n[[copieurs]]\nnom = \"A\"\nadresse = \"x\"\ncommunaute = \"y\"\n\
                      [[copieurs]]\nnom = \"A\"\nadresse = \"z\"\ncommunaute = \"y\"\n";
        assert!(Config::lire(double).unwrap_err().contains("double"));
    }
}
