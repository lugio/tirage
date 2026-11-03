//! File d'attente sur disque du collecteur.
//!
//! Principe : chaque événement est écrit de façon **atomique** (fichier temporaire, synchronisation, renommage)
//! avant tout envoi, puis retiré seulement après l'accusé de réception du serveur. Une coupure pendant l'écriture
//! laisse au pire un fichier temporaire, effacé à la réouverture : jamais un événement à moitié écrit.
//! Les doublons (envoi réussi mais accusé perdu) sont écartés côté serveur par la clé de l'événement.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Nom du sous-dossier de la file, relatif au dossier de données du collecteur.
pub const DOSSIER_FILE: &str = "a-envoyer";
/// Sous-dossier des éléments mis de côté (illisibles ou refusés par le serveur).
pub const DOSSIER_REBUT: &str = "rebut";

const EXT: &str = "evt";
const EXT_TMP: &str = "tmp";

#[derive(Debug)]
pub enum ErreurFile {
    Io(io::Error),
    /// La file a atteint sa capacité : le serveur est injoignable depuis longtemps.
    Pleine {
        capacite: usize,
    },
}

impl std::fmt::Display for ErreurFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErreurFile::Io(e) => write!(f, "file d'attente : {e}"),
            ErreurFile::Pleine { capacite } => write!(f, "file d'attente pleine ({capacite} éléments)"),
        }
    }
}

impl std::error::Error for ErreurFile {}

impl From<io::Error> for ErreurFile {
    fn from(e: io::Error) -> Self {
        ErreurFile::Io(e)
    }
}

/// Un élément en attente d'envoi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub id: u64,
    pub octets: Vec<u8>,
}

pub struct FileAttente {
    dossier: PathBuf,
    capacite: usize,
    suivant: u64,
    longueur: usize,
}

impl FileAttente {
    /// Ouvre (ou crée) la file sous `racine/a-envoyer`. Efface les écritures inachevées.
    pub fn ouvrir(racine: &Path, capacite: usize) -> Result<Self, ErreurFile> {
        let dossier = racine.join(DOSSIER_FILE);
        fs::create_dir_all(&dossier)?;
        let mut max = 0u64;
        let mut longueur = 0usize;
        for entree in fs::read_dir(&dossier)? {
            let chemin = entree?.path();
            match chemin.extension().and_then(|e| e.to_str()) {
                Some(EXT_TMP) => fs::remove_file(&chemin)?,
                Some(EXT) => {
                    if let Some(id) = id_du_chemin(&chemin) {
                        max = max.max(id);
                        longueur += 1;
                    }
                }
                _ => {}
            }
        }
        Ok(FileAttente {
            dossier,
            capacite,
            suivant: max + 1,
            longueur,
        })
    }

    pub fn longueur(&self) -> usize {
        self.longueur
    }

    pub fn est_vide(&self) -> bool {
        self.longueur == 0
    }

    /// Dépose un élément ; renvoie son identifiant. L'élément est sur disque au retour.
    pub fn deposer(&mut self, octets: &[u8]) -> Result<u64, ErreurFile> {
        if self.longueur >= self.capacite {
            return Err(ErreurFile::Pleine {
                capacite: self.capacite,
            });
        }
        let id = self.suivant;
        let tmp = self.dossier.join(format!("{id:020}.{EXT_TMP}"));
        {
            let mut f = File::create(&tmp)?;
            f.write_all(octets)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, self.chemin(id))?;
        synchroniser_dossier(&self.dossier)?;
        self.suivant += 1;
        self.longueur += 1;
        Ok(id)
    }

    /// Les `max` plus anciens éléments, dans l'ordre de dépôt.
    pub fn lot(&self, max: usize) -> Result<Vec<Element>, ErreurFile> {
        let mut ids: Vec<u64> = fs::read_dir(&self.dossier)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(EXT))
            .filter_map(|p| id_du_chemin(&p))
            .collect();
        ids.sort_unstable();
        ids.truncate(max);
        ids.into_iter()
            .map(|id| {
                Ok(Element {
                    id,
                    octets: fs::read(self.chemin(id))?,
                })
            })
            .collect()
    }

    /// Retire un élément acquitté par le serveur. Sans effet s'il n'existe plus.
    pub fn acquitter(&mut self, id: u64) -> Result<(), ErreurFile> {
        match fs::remove_file(self.chemin(id)) {
            Ok(()) => {
                self.longueur = self.longueur.saturating_sub(1);
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Met un élément de côté (illisible ou refusé) : il ne bloque plus la file, mais reste consultable.
    pub fn mettre_de_cote(&mut self, id: u64) -> Result<(), ErreurFile> {
        let rebut = self.dossier.parent().unwrap_or(&self.dossier).join(DOSSIER_REBUT);
        fs::create_dir_all(&rebut)?;
        match fs::rename(self.chemin(id), rebut.join(format!("{id:020}.{EXT}"))) {
            Ok(()) => {
                self.longueur = self.longueur.saturating_sub(1);
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    fn chemin(&self, id: u64) -> PathBuf {
        self.dossier.join(format!("{id:020}.{EXT}"))
    }
}

fn id_du_chemin(p: &Path) -> Option<u64> {
    p.file_stem()?.to_str()?.parse().ok()
}

#[cfg(unix)]
fn synchroniser_dossier(d: &Path) -> io::Result<()> {
    File::open(d)?.sync_all()
}

#[cfg(not(unix))]
fn synchroniser_dossier(_d: &Path) -> io::Result<()> {
    // Sous Windows (NTFS), le renommage est journalisé ; ouvrir un dossier pour le synchroniser n'est pas permis.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(d: &Path) -> FileAttente {
        FileAttente::ouvrir(d, 100).unwrap()
    }

    #[test]
    fn depot_ordre_et_acquittement() {
        let d = tempfile::tempdir().unwrap();
        let mut f = file(d.path());
        let a = f.deposer(b"a").unwrap();
        let b = f.deposer(b"b").unwrap();
        assert!(a < b);
        let lot = f.lot(10).unwrap();
        assert_eq!(
            lot.iter().map(|e| e.octets.clone()).collect::<Vec<_>>(),
            vec![b"a".to_vec(), b"b".to_vec()]
        );
        f.acquitter(a).unwrap();
        f.acquitter(a).unwrap(); // deux fois : sans effet
        assert_eq!(f.longueur(), 1);
        assert_eq!(f.lot(10).unwrap()[0].id, b);
    }

    #[test]
    fn rien_ne_se_perd_a_la_reouverture() {
        let d = tempfile::tempdir().unwrap();
        {
            let mut f = file(d.path());
            for i in 0..5u8 {
                f.deposer(&[i]).unwrap();
            }
            f.acquitter(1).unwrap();
        } // « arrêt » du collecteur
        let mut f = file(d.path());
        assert_eq!(f.longueur(), 4);
        let suivant = f.deposer(b"apres").unwrap();
        assert_eq!(
            suivant, 6,
            "la numérotation reprend après le plus grand id, sans écraser"
        );
        let ids: Vec<u64> = f.lot(10).unwrap().iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![2, 3, 4, 5, 6]);
    }

    #[test]
    fn ecriture_inachevee_effacee_jamais_livree() {
        let d = tempfile::tempdir().unwrap();
        {
            let mut f = file(d.path());
            f.deposer(b"complet").unwrap();
        }
        // Coupure simulée en pleine écriture : un temporaire reste.
        fs::write(
            d.path().join(DOSSIER_FILE).join("00000000000000000002.tmp"),
            b"moit",
        )
        .unwrap();
        let f = file(d.path());
        assert_eq!(f.longueur(), 1);
        assert_eq!(
            f.lot(10).unwrap(),
            vec![Element {
                id: 1,
                octets: b"complet".to_vec()
            }]
        );
        assert!(
            !d.path()
                .join(DOSSIER_FILE)
                .join("00000000000000000002.tmp")
                .exists()
        );
    }

    #[test]
    fn capacite_respectee() {
        let d = tempfile::tempdir().unwrap();
        let mut f = FileAttente::ouvrir(d.path(), 2).unwrap();
        f.deposer(b"1").unwrap();
        f.deposer(b"2").unwrap();
        assert!(matches!(f.deposer(b"3"), Err(ErreurFile::Pleine { capacite: 2 })));
        f.acquitter(1).unwrap();
        f.deposer(b"3").unwrap();
    }

    #[test]
    fn lot_borne() {
        let d = tempfile::tempdir().unwrap();
        let mut f = file(d.path());
        for _ in 0..7 {
            f.deposer(b"x").unwrap();
        }
        assert_eq!(f.lot(3).unwrap().len(), 3);
    }

    #[test]
    fn mise_de_cote() {
        let d = tempfile::tempdir().unwrap();
        let mut f = file(d.path());
        let id = f.deposer(b"illisible").unwrap();
        f.deposer(b"bon").unwrap();
        f.mettre_de_cote(id).unwrap();
        assert_eq!(f.longueur(), 1);
        assert_eq!(f.lot(10).unwrap()[0].octets, b"bon");
        assert!(
            d.path()
                .join(DOSSIER_REBUT)
                .join(format!("{id:020}.evt"))
                .exists()
        );
    }

    #[test]
    fn fichiers_etrangers_ignores() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join(DOSSIER_FILE)).unwrap();
        fs::write(d.path().join(DOSSIER_FILE).join("lisezmoi.txt"), b"x").unwrap();
        fs::write(d.path().join(DOSSIER_FILE).join("pasunnombre.evt"), b"x").unwrap();
        let f = file(d.path());
        assert_eq!(f.longueur(), 0);
        assert!(f.lot(10).unwrap().is_empty());
    }
}
