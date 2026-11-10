//! Profils de compteurs par marque, en fichiers de configuration.
//!
//! Un profil dit quels OID additionner pour obtenir les compteurs total, noir et couleur. Les profils fournis
//! sont intégrés à l'exécutable ; d'autres peuvent être ajoutés dans un dossier, sans recompiler.

use std::path::Path;

use serde::Deserialize;
use tirage_snmp::Oid;

/// Profils de `profils/*.toml`, intégrés à la compilation (`build.rs`) : (nom du fichier, contenu).
pub const INTEGRES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/profils_integres.rs"));

/// Nom du profil de repli (Printer-MIB standard).
pub const REPLI: &str = "printer-mib";

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fichier {
    nom: String,
    #[serde(default)]
    description: String,
    entreprise: Option<u32>,
    #[serde(default)]
    compteurs: Compteurs<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compteurs<T> {
    #[serde(default = "Vec::new")]
    pub total: Vec<T>,
    #[serde(default = "Vec::new")]
    pub noir: Vec<T>,
    #[serde(default = "Vec::new")]
    pub couleur: Vec<T>,
    /// Compte pour zéro un OID absent ou négatif : selon le modèle, une fonction n'existe pas (copie sur une
    /// imprimante). Le compteur reste non publié si aucun OID ne répond.
    #[serde(default)]
    pub absents_toleres: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profil {
    pub nom: String,
    pub description: String,
    /// Numéro d'entreprise IANA : le profil s'applique si `sysObjectID` commence par 1.3.6.1.4.1.<entreprise>.
    pub entreprise: Option<u32>,
    /// OID à additionner ; une liste vide = compteur non fourni par ce profil.
    pub compteurs: Compteurs<Oid>,
}

impl Profil {
    pub fn lire(texte: &str) -> Result<Self, String> {
        let f: Fichier = toml::from_str(texte).map_err(|e| e.to_string())?;
        let conv = |v: &[String]| -> Result<Vec<Oid>, String> {
            v.iter().map(|s| s.parse::<Oid>().map_err(|e| e.0)).collect()
        };
        if f.nom.trim().is_empty() {
            return Err("profil sans nom".into());
        }
        Ok(Profil {
            nom: f.nom,
            description: f.description,
            entreprise: f.entreprise,
            compteurs: Compteurs {
                total: conv(&f.compteurs.total)?,
                noir: conv(&f.compteurs.noir)?,
                couleur: conv(&f.compteurs.couleur)?,
                absents_toleres: f.compteurs.absents_toleres,
            },
        })
    }
}

#[derive(Debug, Clone)]
pub struct Profils(Vec<Profil>);

impl Profils {
    /// Profils intégrés, plus ceux d'un dossier (un fichier du même nom de profil remplace l'intégré).
    pub fn charger(dossier: Option<&Path>) -> Result<Self, String> {
        let mut liste = Vec::new();
        for (fichier, texte) in INTEGRES {
            liste.push(Profil::lire(texte).map_err(|e| format!("profil intégré {fichier} : {e}"))?);
        }
        if let Some(d) = dossier {
            let entrees =
                std::fs::read_dir(d).map_err(|e| format!("dossier de profils {} : {e}", d.display()))?;
            let mut chemins: Vec<_> = entrees
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("toml"))
                .collect();
            chemins.sort();
            for p in chemins {
                let texte = std::fs::read_to_string(&p).map_err(|e| format!("{} : {e}", p.display()))?;
                let profil = Profil::lire(&texte).map_err(|e| format!("{} : {e}", p.display()))?;
                liste.retain(|x: &Profil| x.nom != profil.nom);
                liste.push(profil);
            }
        }
        Ok(Profils(liste))
    }

    pub fn par_nom(&self, nom: &str) -> Option<&Profil> {
        self.0.iter().find(|p| p.nom == nom)
    }

    pub fn repli(&self) -> &Profil {
        self.par_nom(REPLI).expect("le profil de repli est intégré")
    }

    /// Profil d'un appareil d'après son `sysObjectID`, à défaut le repli.
    pub fn choisir(&self, sys_object_id: Option<&Oid>) -> &Profil {
        sys_object_id
            .and_then(|id| {
                self.0.iter().find(|p| {
                    p.entreprise
                        .is_some_and(|e| id.sous(&Oid(vec![1, 3, 6, 1, 4, 1, e])))
                })
            })
            .unwrap_or_else(|| self.repli())
    }

    pub fn noms(&self) -> Vec<&str> {
        self.0.iter().map(|p| p.nom.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profils_integres_lisibles() {
        let p = Profils::charger(None).unwrap();
        assert!(p.par_nom("konica-minolta").is_some());
        assert_eq!(p.noms().len(), INTEGRES.len());
        assert_eq!(p.repli().compteurs.total.len(), 1);
    }

    #[test]
    fn choix_par_identifiant_d_appareil() {
        let p = Profils::charger(None).unwrap();
        let km: Oid = "1.3.6.1.4.1.18334.1.2.3".parse().unwrap();
        let autre: Oid = "1.3.6.1.4.1.99999.1".parse().unwrap();
        assert_eq!(p.choisir(Some(&km)).nom, "konica-minolta");
        assert_eq!(p.choisir(Some(&autre)).nom, REPLI);
        assert_eq!(p.choisir(None).nom, REPLI);
        // 1.3.6.1.4.1.183340 n'est pas sous 18334.
        let piege: Oid = "1.3.6.1.4.1.183340.1".parse().unwrap();
        assert_eq!(p.choisir(Some(&piege)).nom, REPLI);
    }

    #[test]
    fn dossier_ajoute_et_remplace() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("exemple.toml"),
            "nom = \"exemple\"\nentreprise = 99999\n[compteurs]\nnoir = [\"1.3.6.1.4.1.99999.1.1\", \"1.3.6.1.4.1.99999.1.2\"]\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("km.toml"),
            "nom = \"konica-minolta\"\nentreprise = 18334\n[compteurs]\ncouleur = [\"1.3.6.1.4.1.18334.9\"]\n",
        )
        .unwrap();
        std::fs::write(d.path().join("lisezmoi.txt"), "ignoré").unwrap();
        let p = Profils::charger(Some(d.path())).unwrap();
        assert_eq!(p.par_nom("exemple").unwrap().compteurs.noir.len(), 2);
        assert_eq!(p.par_nom("konica-minolta").unwrap().compteurs.couleur.len(), 1);
        assert_eq!(p.noms().len(), INTEGRES.len() + 1);
    }

    #[test]
    fn profil_invalide_refuse() {
        assert!(Profil::lire("nom = \"x\"\n[compteurs]\nnoir = [\"pas.un.oid\"]\n").is_err());
        assert!(Profil::lire("nom = \"x\"\nchamp_inconnu = 1\n").is_err());
        assert!(Profil::lire("nom = \" \"\n").is_err());
    }
}
