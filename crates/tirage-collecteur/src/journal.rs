//! Journal du collecteur : une ligne horodatée (UTC) par événement de fonctionnement.
//!
//! Minimisation : le journal ne contient **aucun travail** (ni utilisateur, ni document) — seulement le démarrage,
//! l'arrêt, les erreurs et les changements d'état de l'envoi. Taille bornée : au-delà de `TAILLE_MAX`, le fichier
//! courant devient `.1` (l'ancien `.1` est écrasé).

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::spouleur::temps::{iso_utc, maintenant_ms};

pub const TAILLE_MAX: u64 = 5 * 1024 * 1024;

pub struct Journal {
    chemin: Option<PathBuf>,
    taille_max: u64,
    echo: bool,
    fichier: Mutex<Option<File>>,
}

impl Journal {
    /// Journal dans un fichier (créé au besoin) ; `echo` recopie aussi sur la sortie d'erreur (mode console).
    pub fn fichier(chemin: &Path, echo: bool) -> std::io::Result<Self> {
        if let Some(d) = chemin.parent() {
            fs::create_dir_all(d)?;
        }
        let f = OpenOptions::new().create(true).append(true).open(chemin)?;
        Ok(Journal {
            chemin: Some(chemin.to_path_buf()),
            taille_max: TAILLE_MAX,
            echo,
            fichier: Mutex::new(Some(f)),
        })
    }

    /// Sortie d'erreur seule.
    pub fn console() -> Self {
        Journal {
            chemin: None,
            taille_max: TAILLE_MAX,
            echo: true,
            fichier: Mutex::new(None),
        }
    }

    pub fn avec_taille_max(mut self, octets: u64) -> Self {
        self.taille_max = octets;
        self
    }

    pub fn ecrire(&self, message: &str) {
        let ligne = format!("{} {message}\n", iso_utc(maintenant_ms()));
        if self.echo {
            eprint!("{ligne}");
        }
        let (Some(chemin), Ok(mut garde)) = (&self.chemin, self.fichier.lock()) else {
            return;
        };
        if let Some(f) = garde.as_mut() {
            // Une erreur d'écriture du journal ne doit jamais arrêter la collecte.
            let _ = f.write_all(ligne.as_bytes());
            if f.metadata().map(|m| m.len()).unwrap_or(0) >= self.taille_max {
                let ancien = chemin.with_extension("log.1");
                *garde = None;
                let _ = fs::rename(chemin, &ancien);
                *garde = OpenOptions::new().create(true).append(true).open(chemin).ok();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lignes_horodatees_et_rotation() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("journal").join("collecteur.log");
        let j = Journal::fichier(&p, false).unwrap().avec_taille_max(200);
        for i in 0..20 {
            j.ecrire(&format!("message {i}"));
        }
        let courant = fs::read_to_string(&p).unwrap();
        let ancien = fs::read_to_string(p.with_extension("log.1")).unwrap();
        assert!(courant.len() < 200 && ancien.len() >= 200);
        assert!(
            courant
                .lines()
                .all(|l| l.len() > 21 && l.as_bytes()[4] == b'-' && l.contains("Z message"))
        );
        assert!(courant.ends_with("message 19\n"));
    }
}
