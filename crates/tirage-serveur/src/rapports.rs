//! Restitutions : agrégats par axe, coûts, palmarès, export CSV. Aucun nom de document n'y figure.
//!
//! Seuls les travaux au statut « imprimé » sont comptés. Une page au mode couleur inconnu est chiffrée au tarif noir :
//! le coût affiché est alors un minimum.

use std::collections::HashMap;
use std::fmt::Write as _;

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
}

impl Axe {
    pub const TOUS: [Axe; 5] = [
        Axe::Utilisateur,
        Axe::Service,
        Axe::Cabinet,
        Axe::Copieur,
        Axe::Format,
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
        }
    }

    pub fn titre(self) -> &'static str {
        match self {
            Axe::Utilisateur => "Utilisateur",
            Axe::Service => "Service",
            Axe::Cabinet => "Cabinet",
            Axe::Copieur => "Copieur",
            Axe::Format => "Format",
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
                noir: 0.02,
                couleur: 0.2,
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
}
