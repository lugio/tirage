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
}
