//! Configuration du collecteur (fichier TOML sur le serveur d'impression).

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub collecteur: String,
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
}

impl Default for Snmp {
    fn default() -> Self {
        Snmp {
            delai_ms: 2000,
            essais: 2,
            dossier_profils: None,
        }
    }
}

impl Snmp {
    pub fn delai(&self) -> Duration {
        Duration::from_millis(self.delai_ms.clamp(100, 30_000))
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
