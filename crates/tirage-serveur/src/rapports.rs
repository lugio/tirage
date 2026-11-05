//! Restitutions : agrégats par axe, coûts, palmarès, export CSV. Aucun nom de document n'y figure.
//!
//! Seuls les travaux au statut « imprimé » sont comptés. Une page au mode couleur inconnu est chiffrée au tarif noir :
//! le coût affiché est alors un minimum.

use std::collections::HashMap;
use std::fmt::Write as _;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use crate::base::LigneBrute;
use crate::config::ConfigCouts;

/// Axe d'un rapport. Liste fermée : c'est elle qui choisit l'expression SQL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axe {
    Utilisateur,
    Service,
    Cabinet,
    Copieur,
    Format,
    Application,
}

impl Axe {
    pub const TOUS: [Axe; 6] = [
        Axe::Utilisateur,
        Axe::Service,
        Axe::Cabinet,
        Axe::Copieur,
        Axe::Format,
        Axe::Application,
    ];

    pub fn du_chemin(s: &str) -> Option<Axe> {
        Axe::TOUS.into_iter().find(|a| a.chemin() == s)
    }

    pub fn chemin(self) -> &'static str {
        match self {
            Axe::Utilisateur => "utilisateur",
            Axe::Service => "service",
            Axe::Cabinet => "cabinet",
            Axe::Copieur => "copieur",
            Axe::Format => "format",
            Axe::Application => "application",
        }
    }

    pub fn titre(self) -> &'static str {
        match self {
            Axe::Utilisateur => "Utilisateur",
            Axe::Service => "Service",
            Axe::Cabinet => "Cabinet",
            Axe::Copieur => "Copieur",
            Axe::Format => "Format",
            Axe::Application => "Application",
        }
    }
}

/// Une ligne de rapport. Les coûts sont en cent-millièmes d'unité monétaire (entiers : pas d'erreur d'arrondi cumulée).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ligne {
    pub cle: String,
    pub nom: Option<String>,
    pub travaux: i64,
    pub pages: i64,
    pub noir: i64,
    pub couleur: i64,
    pub couleur_inconnue: i64,
    pub recto_verso: i64,
    pub feuilles: i64,
    pub cout: i64,
    /// Pages d'une file sans tarif, absentes du coût.
    pub sans_tarif: i64,
}

impl Ligne {
    pub fn lire_pages(&self) -> i64 {
        self.pages
    }

    pub fn lire_couleur(&self) -> i64 {
        self.couleur
    }

    pub fn lire_cout(&self) -> i64 {
        self.cout
    }

    fn ajouter(&mut self, b: &LigneBrute, couts: &ConfigCouts) {
        self.travaux += b.travaux;
        self.pages += b.pages;
        self.feuilles += b.feuilles;
        match b.couleur.as_str() {
            "couleur" => self.couleur += b.pages,
            "noir_et_blanc" => self.noir += b.pages,
            _ => self.couleur_inconnue += b.pages,
        }
        if matches!(b.recto_verso.as_str(), "bord_long" | "bord_court") {
            self.recto_verso += b.pages;
        }
        match couts.prix(&b.file, b.couleur == "couleur") {
            Some(p) => self.cout += p * b.pages,
            None => self.sans_tarif += b.pages,
        }
    }

    fn cumuler(&mut self, l: &Ligne) {
        self.travaux += l.travaux;
        self.pages += l.pages;
        self.noir += l.noir;
        self.couleur += l.couleur;
        self.couleur_inconnue += l.couleur_inconnue;
        self.recto_verso += l.recto_verso;
        self.feuilles += l.feuilles;
        self.cout += l.cout;
        self.sans_tarif += l.sans_tarif;
    }
}

/// Regroupe sans tenir compte de la casse ; trié par pages décroissantes, puis par clé.
pub fn construire(brutes: &[LigneBrute], couts: &ConfigCouts) -> Vec<Ligne> {
    let mut par_cle: HashMap<String, Ligne> = HashMap::new();
    for b in brutes {
        let l = par_cle.entry(b.cle.to_lowercase()).or_insert_with(|| Ligne {
            cle: b.cle.clone(),
            ..Ligne::default()
        });
        if l.nom.is_none() {
            l.nom.clone_from(&b.nom);
        }
        l.ajouter(b, couts);
    }
    let mut v: Vec<Ligne> = par_cle.into_values().collect();
    v.sort_by(|a, b| b.pages.cmp(&a.pages).then_with(|| a.cle.cmp(&b.cle)));
    v
}

pub fn total(lignes: &[Ligne]) -> Ligne {
    let mut t = Ligne {
        cle: "Total".into(),
        ..Ligne::default()
    };
    for l in lignes {
        t.cumuler(l);
    }
    t
}

/// Les `n` premiers selon `mesure` ; les lignes à zéro n'y figurent pas.
pub fn palmares(lignes: &[Ligne], mesure: fn(&Ligne) -> i64, n: usize) -> Vec<&Ligne> {
    let mut v: Vec<&Ligne> = lignes.iter().filter(|l| mesure(l) > 0).collect();
    v.sort_by(|a, b| mesure(b).cmp(&mesure(a)).then_with(|| a.cle.cmp(&b.cle)));
    v.truncate(n);
    v
}

/// Cent-millièmes → texte à deux décimales, séparateur virgule, arrondi au plus proche.
pub fn montant(cout: i64) -> String {
    let c = (cout + 500) / 1000;
    format!("{},{:02}", c / 100, c % 100)
}

/// Champ CSV : guillemets au besoin ; une valeur prise pour une formule par un tableur est neutralisée.
fn champ(s: &str) -> String {
    let neutre = if s.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        format!("'{s}")
    } else {
        s.to_owned()
    };
    if neutre.contains([';', '"', '\n', '\r']) {
        format!("\"{}\"", neutre.replace('"', "\"\""))
    } else {
        neutre
    }
}

/// Export pour tableur : UTF-8 avec BOM, point-virgule, virgule décimale.
pub fn csv(axe: Axe, lignes: &[Ligne], devise: &str) -> String {
    let mut s = String::from("\u{feff}");
    let _ = write!(s, "{}", champ(axe.titre()));
    if axe == Axe::Utilisateur {
        s.push_str(";Nom");
    }
    let _ = write!(
        s,
        ";Travaux;Pages;Pages N&B;Pages couleur;Pages mode inconnu;Pages recto-verso;Feuilles;Coût ({});Pages sans tarif\r\n",
        devise.replace([';', '"', '\r', '\n'], "")
    );
    for l in lignes {
        s.push_str(&champ(&l.cle));
        if axe == Axe::Utilisateur {
            s.push(';');
            s.push_str(&champ(l.nom.as_deref().unwrap_or("")));
        }
        let _ = write!(
            s,
            ";{};{};{};{};{};{};{};{};{}\r\n",
            l.travaux,
            l.pages,
            l.noir,
            l.couleur,
            l.couleur_inconnue,
            l.recto_verso,
            l.feuilles,
            montant(l.cout),
            l.sans_tarif
        );
    }
    s
}

/// Au-delà, l'évolution se lit par mois : des colonnes par jour ne seraient plus lisibles.
const JOURS_MAX_PAR_JOUR: i32 = 62;

/// Pages d'une tranche de temps, avec les mêmes règles de mode couleur que [`Ligne`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tranche {
    pub libelle: String,
    pub noir: i64,
    pub couleur: i64,
    pub inconnu: i64,
}

impl Tranche {
    fn ajouter(&mut self, mode: &str, pages: i64) {
        match mode {
            "couleur" => self.couleur += pages,
            "noir_et_blanc" => self.noir += pages,
            _ => self.inconnu += pages,
        }
    }
}

/// Évolution sur `[du, au]` : par jour, ou par mois si la période est longue ou contient des totaux archivés (ils ne
/// sont connus qu'au mois). Toutes les tranches sont présentes, y compris à zéro, pour un axe du temps régulier.
pub fn evolution(
    du: Date,
    au: Date,
    fuseau: &TimeZone,
    detail: &[(i64, String, i64)],
    archives: &[(String, String, i64)],
) -> Vec<Tranche> {
    let jours = du.until(au).map_or(0, |s| s.get_days());
    let par_mois = !archives.is_empty() || jours > JOURS_MAX_PAR_JOUR;
    let cle = |d: Date| {
        if par_mois {
            d.strftime("%Y-%m").to_string()
        } else {
            d.strftime("%Y-%m-%d").to_string()
        }
    };
    let mut tranches: Vec<(String, Tranche)> = Vec::new();
    let mut d = du;
    while d <= au {
        let k = cle(d);
        if tranches.last().is_none_or(|(c, _)| *c != k) {
            let libelle = if par_mois {
                d.strftime("%m/%Y")
            } else {
                d.strftime("%d/%m")
            }
            .to_string();
            tranches.push((
                k,
                Tranche {
                    libelle,
                    noir: 0,
                    couleur: 0,
                    inconnu: 0,
                },
            ));
        }
        match d.tomorrow() {
            Ok(s) => d = s,
            Err(_) => break,
        }
    }
    let index: HashMap<String, usize> = tranches
        .iter()
        .enumerate()
        .map(|(i, (k, _))| (k.clone(), i))
        .collect();
    let mut poser = |k: &str, mode: &str, pages: i64| {
        if let Some(i) = index.get(k) {
            tranches[*i].1.ajouter(mode, pages);
        }
    };
    for (ms, mode, pages) in detail {
        if let Ok(t) = Timestamp::from_millisecond(*ms) {
            poser(&cle(t.to_zoned(fuseau.clone()).date()), mode, *pages);
        }
    }
    for (mois, mode, pages) in archives {
        poser(mois, mode, *pages);
    }
    tranches.into_iter().map(|(_, t)| t).collect()
}

/// Pages par jour de la semaine (lundi d'abord) et par heure, dans le fuseau. Le détail vient par quart d'heure.
pub fn activite_horaire(detail: &[(i64, String, i64)], fuseau: &TimeZone) -> [[i64; 24]; 7] {
    let mut grille = [[0; 24]; 7];
    for (ms, _, pages) in detail {
        if let Ok(t) = Timestamp::from_millisecond(*ms) {
            let z = t.to_zoned(fuseau.clone());
            let jour = z.weekday().to_monday_zero_offset() as usize;
            if let Some(case) = grille
                .get_mut(jour)
                .and_then(|l| l.get_mut(usize::try_from(z.hour()).unwrap_or(0)))
            {
                *case += pages;
            }
        }
    }
    grille
}

/// Minutes ouvrées de `[depuis_ms, jusqu_ms[`, du lundi au vendredi, entre `ouverture` et `fermeture` (fin exclue).
/// Le calcul s'arrête à `plafond` : au-delà, le détail n'importe plus.
pub fn minutes_ouvrees(
    depuis_ms: i64,
    jusqu_ms: i64,
    fuseau: &TimeZone,
    (ouverture, fermeture): (u8, u8),
    plafond: i64,
) -> i64 {
    let date = |ms| {
        Timestamp::from_millisecond(ms)
            .ok()
            .map(|t| t.to_zoned(fuseau.clone()).date())
    };
    let (Some(mut d), Some(fin)) = (date(depuis_ms), date(jusqu_ms)) else {
        return 0;
    };
    let borne = |j: Date, h: u8| {
        let (heure, jour) = if h >= 24 {
            (0, j.tomorrow().ok()?)
        } else {
            (i8::try_from(h).ok()?, j)
        };
        Some(
            jour.at(heure, 0, 0, 0)
                .to_zoned(fuseau.clone())
                .ok()?
                .timestamp()
                .as_millisecond(),
        )
    };
    let mut total = 0;
    while d <= fin && total < plafond {
        if d.weekday().to_monday_zero_offset() < 5
            && let (Some(a), Some(b)) = (borne(d, ouverture), borne(d, fermeture))
        {
            total += (b.min(jusqu_ms) - a.max(depuis_ms)).max(0) / 60_000;
        }
        match d.tomorrow() {
            Ok(s) => d = s,
            Err(_) => break,
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigCouts, TarifFile};

    fn brute(cle: &str, file: &str, couleur: &str, rv: &str, pages: i64) -> LigneBrute {
        LigneBrute {
            cle: cle.into(),
            nom: None,
            file: file.into(),
            couleur: couleur.into(),
            recto_verso: rv.into(),
            travaux: 1,
            pages,
            feuilles: pages,
        }
    }

    fn couts() -> ConfigCouts {
        ConfigCouts {
            devise: "€".into(),
            noir: Some(0.01),
            couleur: Some(0.1),
            files: vec![TarifFile {
                file: "CHER".into(),
                noir: Some(0.02),
                couleur: Some(0.2),
            }],
        }
    }

    #[test]
    fn agregats_et_couts() {
        let b = [
            brute("a", "F", "noir_et_blanc", "recto", 100),
            brute("A", "F", "couleur", "bord_long", 10),
            brute("a", "CHER", "couleur", "bord_court", 10),
            brute("a", "F", "inconnu", "inconnu", 5),
            brute("b", "F", "noir_et_blanc", "recto", 1),
        ];
        let l = construire(&b, &couts());
        assert_eq!(l.len(), 2, "la casse ne sépare pas une personne");
        let a = &l[0];
        assert_eq!(
            (
                a.pages,
                a.noir,
                a.couleur,
                a.couleur_inconnue,
                a.recto_verso,
                a.travaux
            ),
            (125, 100, 20, 5, 20, 4)
        );
        // 100×0,01 + 10×0,10 + 10×0,20 + 5×0,01 (inconnu au tarif noir) = 4,05
        assert_eq!(montant(a.cout), "4,05");
        let t = total(&l);
        assert_eq!((t.pages, montant(t.cout).as_str()), (126, "4,06"));
    }

    #[test]
    fn file_sans_tarif_signalee_et_pas_comptee() {
        let c = ConfigCouts::default();
        let l = construire(&[brute("a", "F", "couleur", "recto", 7)], &c);
        assert_eq!((l[0].cout, l[0].sans_tarif), (0, 7));
    }

    #[test]
    fn palmares_borne_et_sans_zero() {
        let b = [
            brute("a", "F", "noir_et_blanc", "recto", 5),
            brute("b", "F", "couleur", "recto", 3),
            brute("c", "F", "couleur", "recto", 9),
        ];
        let l = construire(&b, &couts());
        let p = palmares(&l, |x| x.couleur, 1);
        assert_eq!(p[0].cle, "c");
        assert_eq!(palmares(&l, |x| x.couleur, 10).len(), 2);
    }

    #[test]
    fn montants() {
        assert_eq!(montant(0), "0,00");
        assert_eq!(montant(500), "0,01");
        assert_eq!(montant(499), "0,00");
        assert_eq!(montant(123_456_000), "1234,56");
    }

    #[test]
    fn csv_pour_tableur() {
        let mut l = construire(&[brute("=SOMME(A1);\"x\"", "F", "couleur", "recto", 2)], &couts());
        l[0].nom = Some("-danger".into());
        let c = csv(Axe::Utilisateur, &l, "€");
        assert!(c.starts_with('\u{feff}'));
        let lignes: Vec<&str> = c.split("\r\n").collect();
        assert!(lignes[0].ends_with("Coût (€);Pages sans tarif"));
        assert!(
            lignes[1].starts_with("\"'=SOMME(A1);\"\"x\"\"\";'-danger;1;2;0;2;0;0;2;0,20;0"),
            "{}",
            lignes[1]
        );
    }

    fn paris() -> TimeZone {
        TimeZone::get("Europe/Paris").unwrap()
    }

    fn date(a: i16, m: i8, j: i8) -> Date {
        Date::new(a, m, j).unwrap()
    }

    fn ms(texte: &str) -> i64 {
        texte.parse::<Timestamp>().unwrap().as_millisecond()
    }

    #[test]
    fn evolution_par_jour_dans_le_fuseau_avec_tous_les_jours() {
        let detail = vec![
            // 00:30 à Paris le 01/11, encore le 31/10 en UTC.
            (ms("2026-10-31T23:30:00Z"), "noir_et_blanc".to_owned(), 10),
            (ms("2026-11-03T08:00:00Z"), "couleur".to_owned(), 5),
            (ms("2026-11-03T09:00:00Z"), "inconnu".to_owned(), 2),
        ];
        let v = evolution(date(2026, 11, 1), date(2026, 11, 3), &paris(), &detail, &[]);
        assert_eq!(
            v.iter().map(|t| t.libelle.as_str()).collect::<Vec<_>>(),
            ["01/11", "02/11", "03/11"]
        );
        assert_eq!((v[0].noir, v[1].noir, v[2].couleur, v[2].inconnu), (10, 0, 5, 2));
    }

    #[test]
    fn activite_horaire_dans_le_fuseau() {
        // 03/11/2026, un mardi : 08:00 UTC = 09:00 à Paris. 01/11/2026, un dimanche : 23:30 UTC le 31/10 = 00:30.
        let detail = vec![
            (ms("2026-11-03T08:00:00Z"), "couleur".to_owned(), 5),
            (ms("2026-11-03T08:15:00Z"), "noir_et_blanc".to_owned(), 2),
            (ms("2026-10-31T23:30:00Z"), "noir_et_blanc".to_owned(), 7),
        ];
        let g = activite_horaire(&detail, &paris());
        assert_eq!(g[1][9], 7);
        assert_eq!(g[6][0], 7);
        assert_eq!(g.iter().flatten().sum::<i64>(), 14);
    }

    #[test]
    fn minutes_ouvrees_sans_week_end_ni_nuit() {
        let m = |a: &str, b: &str| minutes_ouvrees(ms(a), ms(b), &paris(), (8, 18), 100_000);
        // Paris en novembre = UTC+1. Mardi 03/11, de 09:00 à 10:30.
        assert_eq!(m("2026-11-03T08:00:00Z", "2026-11-03T09:30:00Z"), 90);
        // La nuit ne compte pas : mardi 17:30 à mercredi 08:30 = 30 + 30.
        assert_eq!(m("2026-11-03T16:30:00Z", "2026-11-04T07:30:00Z"), 60);
        // Le week-end non plus : vendredi 17:55 à lundi 08:05 = 5 + 5.
        assert_eq!(m("2026-11-06T16:55:00Z", "2026-11-09T07:05:00Z"), 10);
        // Tout un samedi : rien.
        assert_eq!(m("2026-11-07T08:00:00Z", "2026-11-07T16:00:00Z"), 0);
        // Le plafond arrête le parcours.
        assert!(minutes_ouvrees(0, ms("2026-11-03T08:00:00Z"), &paris(), (8, 18), 200) < 2000);
        // Fermeture à minuit.
        assert_eq!(
            minutes_ouvrees(
                ms("2026-11-03T22:00:00Z"),
                ms("2026-11-04T01:00:00Z"),
                &paris(),
                (8, 24),
                100_000
            ),
            60
        );
    }

    #[test]
    fn evolution_au_changement_d_heure() {
        // Le 25/10/2026 à Paris dure 25 heures : 22:30 UTC est encore le 25, 23:30 UTC déjà le 26.
        let detail = vec![
            (ms("2026-10-25T22:30:00Z"), "couleur".to_owned(), 1),
            (ms("2026-10-25T23:30:00Z"), "couleur".to_owned(), 2),
        ];
        let v = evolution(date(2026, 10, 25), date(2026, 10, 26), &paris(), &detail, &[]);
        assert_eq!((v[0].couleur, v[1].couleur), (1, 2));
    }

    #[test]
    fn evolution_par_mois_si_longue_ou_archivee() {
        let detail = vec![(ms("2026-03-10T12:00:00Z"), "couleur".to_owned(), 3)];
        let v = evolution(date(2026, 1, 15), date(2026, 4, 2), &paris(), &detail, &[]);
        assert_eq!(
            v.iter().map(|t| t.libelle.as_str()).collect::<Vec<_>>(),
            ["01/2026", "02/2026", "03/2026", "04/2026"]
        );
        assert_eq!(v[2].couleur, 3);

        // Période courte, mais un total archivé n'est connu qu'au mois : il ne doit pas tomber sur un jour inventé.
        let archives = vec![("2026-02".to_owned(), "noir_et_blanc".to_owned(), 7)];
        let v = evolution(date(2026, 2, 1), date(2026, 3, 3), &paris(), &[], &archives);
        assert_eq!(
            v.iter().map(|t| (t.libelle.as_str(), t.noir)).collect::<Vec<_>>(),
            [("02/2026", 7), ("03/2026", 0)]
        );
    }
}
