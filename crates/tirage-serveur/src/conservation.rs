//! Conservation : au-delà de la durée configurée, un travail ne survit qu'en totaux mensuels sans nom, et le journal
//! des consultations est purgé. Sauvegarde cohérente de la base, serveur en marche.

use std::path::Path;
use std::sync::Mutex;

use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};

use crate::base::{Base, BilanPurge};
use crate::config::ConfigConservation;

/// Instant `mois` mois avant maintenant, dans le fuseau donné.
pub fn limite_ms(maintenant_ms: i64, mois: u32, fuseau: &TimeZone) -> Option<i64> {
    Timestamp::from_millisecond(maintenant_ms)
        .ok()?
        .to_zoned(fuseau.clone())
        .checked_sub(i64::from(mois).months())
        .ok()
        .map(|z| z.timestamp().as_millisecond())
}

/// Mois d'un instant dans le fuseau donné : « AAAA-MM ».
pub fn mois_de(ms: i64, fuseau: &TimeZone) -> String {
    Timestamp::from_millisecond(ms).map_or_else(
        |_| "0000-00".to_owned(),
        |t| t.to_zoned(fuseau.clone()).strftime("%Y-%m").to_string(),
    )
}

pub fn appliquer(
    base: &Mutex<Base>,
    c: &ConfigConservation,
    fuseau: &TimeZone,
    maintenant_ms: i64,
) -> Result<BilanPurge, String> {
    let detail = limite_ms(maintenant_ms, c.detail_mois, fuseau).ok_or("date de conservation invalide")?;
    let journal = limite_ms(maintenant_ms, c.journal_mois, fuseau).ok_or("date de conservation invalide")?;
    base.lock()
        .map_err(|_| "base indisponible".to_owned())?
        .purger(detail, journal, |ms| mois_de(ms, fuseau))
        .map_err(|e| e.to_string())
}

/// Copie cohérente de la base dans un fichier neuf, lisible par son seul propriétaire. Un fichier existant n'est jamais
/// écrasé. La copie contient des données personnelles : elle se protège et se purge comme la base.
pub fn sauvegarder(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.is_file() {
        return Err(format!("{} : base introuvable", source.display()));
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(destination)
        .map_err(|e| format!("{} : {e}", destination.display()))?;
    let copie = Base::ouvrir(source)
        .and_then(|b| b.copier_vers(destination))
        .map_err(|e| e.to_string());
    if copie.is_err() {
        let _ = std::fs::remove_file(destination);
    }
    copie
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::tests::travail;
    use crate::rapports::Axe;
    use tirage_modele::{Evenement, Statut};

    /// 2026-11-15 12:00 UTC.
    const MAINTENANT: i64 = 1_794_744_000_000;
    const JOUR: i64 = 86_400_000;

    fn paris() -> TimeZone {
        TimeZone::get("Europe/Paris").unwrap()
    }

    fn conf() -> ConfigConservation {
        ConfigConservation::default()
    }

    fn ancien(n: u32, u: &str, age_jours: i64) -> Evenement {
        let mut t = travail(n);
        t.utilisateur = u.into();
        t.envoye_ms = MAINTENANT - age_jours * JOUR;
        t.nom_document = Some("document-fictif.pdf".into());
        Evenement::Travail(t)
    }

    #[test]
    fn dates() {
        let z = paris();
        assert_eq!(
            mois_de(1_793_489_400_000, &z),
            "2026-11",
            "00:30 à Paris, encore octobre en UTC"
        );
        let l = limite_ms(MAINTENANT, 12, &z).unwrap();
        assert_eq!(mois_de(l, &z), "2025-11");
        assert_eq!(MAINTENANT - l, 365 * JOUR);
    }

    #[test]
    fn purge_a_date_simulee() {
        let base = Mutex::new(Base::en_memoire().unwrap());
        {
            let mut b = base.lock().unwrap();
            b.enregistrer(&ancien(1, "parti.exemple", 400), 0).unwrap();
            b.enregistrer(&ancien(2, "parti.exemple", 401), 0).unwrap();
            b.enregistrer(&ancien(3, "reste.exemple", 400), 0).unwrap();
            b.enregistrer(&ancien(4, "reste.exemple", 10), 0).unwrap();
            let mut annule = travail(5);
            annule.statut = Statut::Annule;
            annule.envoye_ms = MAINTENANT - 400 * JOUR;
            b.enregistrer(&Evenement::Travail(annule), 0).unwrap();
            b.personne_trouvee("parti.exemple", Some("Parti"), Some("Atelier"), Some("Est"), 0)
                .unwrap();
            b.personne_trouvee("reste.exemple", Some("Reste"), None, None, 0)
                .unwrap();
            b.consigner(
                MAINTENANT - 400 * JOUR,
                "lecteur",
                "detail_utilisateur",
                Some("parti.exemple"),
            )
            .unwrap();
            b.consigner(MAINTENANT - JOUR, "lecteur", "tableau", None)
                .unwrap();
        }
        let bilan = appliquer(&base, &conf(), &paris(), MAINTENANT).unwrap();
        assert_eq!((bilan.travaux, bilan.personnes, bilan.consultations), (4, 1, 1));

        let b = base.lock().unwrap();
        // Le détail récent reste, le reste a disparu, y compris la fiche de la personne sans travail restant.
        assert_eq!(b.nombre_travaux().unwrap(), 1);
        assert!(b.travaux_de("parti.exemple", 0, i64::MAX, 10).unwrap().is_empty());
        assert_eq!(b.personne("parti.exemple").unwrap(), None);
        assert!(b.personne("reste.exemple").unwrap().is_some());
        assert_eq!(b.consultations(10).unwrap().len(), 1);

        // Totaux sans nom : l'annulé n'y entre pas, le service d'alors est gardé.
        let mois = mois_de(MAINTENANT - 400 * JOUR, &paris());
        let a = b.totaux_archives(Axe::Service, &mois, &mois).unwrap();
        let pages = |n: &str| -> i64 { a.iter().filter(|l| l.cle == n).map(|l| l.pages).sum() };
        assert_eq!((pages("Atelier"), pages("Non rattaché")), (12, 6));
        assert!(
            b.totaux_archives(Axe::Utilisateur, &mois, &mois)
                .unwrap()
                .is_empty()
        );
        drop(b);

        // Deuxième passage : rien à refaire, et les totaux ne doublent pas.
        let bilan = appliquer(&base, &conf(), &paris(), MAINTENANT).unwrap();
        assert_eq!(bilan.travaux, 0);
        let b = base.lock().unwrap();
        let a = b.totaux_archives(Axe::Copieur, &mois, &mois).unwrap();
        assert_eq!(a.iter().map(|l| l.pages).sum::<i64>(), 18);
    }

    #[test]
    fn les_totaux_d_un_mois_s_additionnent_d_une_purge_a_l_autre() {
        let base = Mutex::new(Base::en_memoire().unwrap());
        base.lock()
            .unwrap()
            .enregistrer(&ancien(1, "a.exemple", 400), 0)
            .unwrap();
        appliquer(&base, &conf(), &paris(), MAINTENANT).unwrap();
        base.lock()
            .unwrap()
            .enregistrer(&ancien(2, "a.exemple", 400), 0)
            .unwrap();
        appliquer(&base, &conf(), &paris(), MAINTENANT).unwrap();
        let mois = mois_de(MAINTENANT - 400 * JOUR, &paris());
        let b = base.lock().unwrap();
        let a = b.totaux_archives(Axe::Copieur, &mois, &mois).unwrap();
        assert_eq!((a.len(), a[0].pages, a[0].travaux), (1, 12, 2));
    }

    #[test]
    fn plus_aucune_trace_dans_les_fichiers_de_la_base() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t.sqlite");
        let base = Mutex::new(Base::ouvrir(&p).unwrap());
        {
            let mut b = base.lock().unwrap();
            for n in 0..200 {
                b.enregistrer(&ancien(n, "personne-fictive", 400), 0).unwrap();
            }
            b.personne_trouvee("personne-fictive", Some("Nom-Fictif"), Some("Atelier"), None, 0)
                .unwrap();
        }
        appliquer(&base, &conf(), &paris(), MAINTENANT).unwrap();
        for f in std::fs::read_dir(d.path()).unwrap() {
            let f = f.unwrap();
            let o = std::fs::read(f.path()).unwrap();
            for mot in ["personne-fictive", "Nom-Fictif", "document-fictif"] {
                assert!(
                    !o.windows(mot.len()).any(|w| w == mot.as_bytes()),
                    "« {mot} » encore dans {:?}",
                    f.file_name()
                );
            }
        }
    }

    #[test]
    fn sauvegarde_coherente_et_sans_ecrasement() {
        let d = tempfile::tempdir().unwrap();
        let source = d.path().join("t.sqlite");
        let cible = d.path().join("copie.sqlite");
        let mut b = Base::ouvrir(&source).unwrap();
        b.enregistrer(&Evenement::Travail(travail(1)), 0).unwrap();
        // Base ouverte en écriture en même temps : la copie passe.
        sauvegarder(&source, &cible).unwrap();
        assert_eq!(Base::ouvrir(&cible).unwrap().nombre_travaux().unwrap(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(&cible).unwrap().permissions().mode();
            assert_eq!(m & 0o777, 0o600);
        }
        // Destination existante : refus, et elle reste intacte.
        std::fs::write(d.path().join("existe"), "x").unwrap();
        assert!(sauvegarder(&source, &d.path().join("existe")).is_err());
        assert_eq!(std::fs::read_to_string(d.path().join("existe")).unwrap(), "x");
        // Pas de base : pas de base vide créée par erreur.
        let absente = d.path().join("absente.sqlite");
        assert!(sauvegarder(&absente, &d.path().join("c2")).is_err());
        assert!(!absente.exists() && !d.path().join("c2").exists());
    }
}
