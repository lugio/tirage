//! Graphiques en SVG, générés côté serveur, sans script : la politique CSP de l'interface n'autorise aucun script.
//!
//! Les positions horizontales sont en pourcentage : le graphique suit la largeur de la page sans que le texte change
//! de taille. Les couleurs viennent de `style.css` (classes de série), avec leurs valeurs claires et sombres. Le survol
//! passe par `<title>`, et les valeurs restent lisibles dans un tableau voisin.

use std::fmt::Write as _;

use super::gabarit::{echapper, entier};

const HAUTEUR: i64 = 220;
const BASE: i64 = 190;
const HAUT: i64 = 14;
/// En dessous, les colonnes restent centrées et fines plutôt que d'occuper toute la largeur.
const CASES_MIN: usize = 12;
const MARGE: f64 = 2.0;
const ETIQUETTES_MAX: usize = 7;
/// En dessous de cette part, le pourcentage ne tient pas dans son segment : il reste dans le survol.
const PART_ETIQUETEE: i64 = 8;

#[derive(Debug, Clone, Copy)]
pub struct Serie {
    pub nom: &'static str,
    pub classe: &'static str,
}

/// Une colonne : un libellé et une valeur par série.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    pub libelle: String,
    pub valeurs: Vec<i64>,
}

pub fn legende(series: &[Serie]) -> String {
    let mut h = String::from("<ul class=\"legende\">");
    for s in series {
        let _ = write!(
            h,
            "<li><i class=\"pastille {}\"></i>{}</li>",
            s.classe,
            echapper(s.nom)
        );
    }
    h.push_str("</ul>");
    h
}

/// Pas de graduation « rond » (1, 2 ou 5 × 10ⁿ), pour quatre intervalles au plus.
fn pas_de_graduation(max: i64) -> i64 {
    let mut p = 1;
    loop {
        for m in [1, 2, 5] {
            if p * m * 4 >= max {
                return p * m;
            }
        }
        p *= 10;
    }
}

fn graduation(v: i64) -> String {
    if v >= 10_000 {
        format!("{}\u{202f}k", entier(v / 1000))
    } else {
        entier(v)
    }
}

fn pourcent(v: f64) -> String {
    format!("{v:.3}%")
}

/// Colonnes empilées dans le temps. `None` si tout est à zéro : un graphique vide n'apprend rien.
pub fn colonnes_empilees(
    description: &str,
    unite: &str,
    series: &[Serie],
    points: &[Point],
) -> Option<String> {
    let max = points
        .iter()
        .map(|p| p.valeurs.iter().sum::<i64>())
        .max()
        .unwrap_or(0);
    if max <= 0 {
        return None;
    }
    let pas = pas_de_graduation(max);
    let sommet = (max + pas - 1) / pas * pas;
    let echelle = |v: i64| v * (BASE - HAUT) / sommet;
    let mut h = format!(
        "<svg class=\"graphique\" width=\"100%\" height=\"{HAUTEUR}\" role=\"img\" aria-label=\"{}\">",
        echapper(description)
    );
    let mut graduations = String::new();
    let mut v = 0;
    while v <= sommet {
        let y = BASE - echelle(v);
        let _ = write!(
            h,
            "<line class=\"grille\" x1=\"0\" x2=\"100%\" y1=\"{y}\" y2=\"{y}\"/>"
        );
        // Écrites après les colonnes, avec un liseré : elles restent lisibles même par-dessus une colonne.
        let _ = write!(
            graduations,
            "<text class=\"axe\" x=\"0\" y=\"{}\">{}</text>",
            y - 3,
            graduation(v)
        );
        v += pas;
    }
    let cases = points.len().max(CASES_MIN);
    let case = (100.0 - 2.0 * MARGE) / cases as f64;
    let depart = MARGE + (cases - points.len()) as f64 * case / 2.0;
    for (i, p) in points.iter().enumerate() {
        let x = depart + i as f64 * case;
        let total: i64 = p.valeurs.iter().sum();
        let mut detail = format!("{} : {} {unite}", p.libelle, entier(total));
        for (s, v) in series.iter().zip(&p.valeurs) {
            let _ = write!(detail, " · {} {}", s.nom, entier(*v));
        }
        let _ = write!(
            h,
            "<g><title>{}</title><rect class=\"cible\" x=\"{}\" y=\"{HAUT}\" width=\"{}\" height=\"{}\"/>",
            echapper(&detail),
            pourcent(x),
            pourcent(case),
            BASE - HAUT
        );
        let (bx, bl) = (pourcent(x + case * 0.19), pourcent(case * 0.62));
        let mut bas = BASE;
        let dernier = series.iter().zip(&p.valeurs).rposition(|(_, v)| *v > 0);
        for (n, (s, v)) in series.iter().zip(&p.valeurs).enumerate() {
            if *v <= 0 {
                continue;
            }
            let c = s.classe;
            let hauteur = echelle(*v).max(1);
            let y = bas - hauteur;
            if Some(n) == dernier {
                // Bout arrondi, pied carré : le second rectangle recouvre les arrondis du bas.
                let _ = write!(
                    h,
                    "<rect class=\"{c}\" x=\"{bx}\" y=\"{y}\" width=\"{bl}\" height=\"{hauteur}\" rx=\"3\"/>"
                );
                if hauteur > 3 {
                    let _ = write!(
                        h,
                        "<rect class=\"{c}\" x=\"{bx}\" y=\"{}\" width=\"{bl}\" height=\"{}\"/>",
                        y + 3,
                        hauteur - 3
                    );
                }
            } else {
                let _ = write!(
                    h,
                    "<rect class=\"{c}\" x=\"{bx}\" y=\"{y}\" width=\"{bl}\" height=\"{hauteur}\"/>"
                );
            }
            // Deux pixels d'air entre deux segments empilés.
            bas = y - 2;
        }
        h.push_str("</g>");
    }
    h.push_str(&graduations);
    let tous_les = points.len().div_ceil(ETIQUETTES_MAX).max(1);
    for (i, p) in points.iter().enumerate().step_by(tous_les) {
        let _ = write!(
            h,
            "<text class=\"axe milieu\" x=\"{}\" y=\"{}\">{}</text>",
            pourcent(depart + (i as f64 + 0.5) * case),
            HAUTEUR - 8,
            echapper(&p.libelle)
        );
    }
    h.push_str("</svg>");
    Some(h)
}

/// Un tout découpé en parts. `None` si le total est nul.
pub fn segments(description: &str, parts: &[(Serie, i64)]) -> Option<String> {
    let total: i64 = parts.iter().map(|(_, v)| *v).sum();
    if total <= 0 {
        return None;
    }
    let mut h = format!(
        "<svg class=\"segments\" width=\"100%\" height=\"28\" role=\"img\" aria-label=\"{}\">",
        echapper(description)
    );
    let mut cumul = 0;
    for (s, v) in parts.iter().filter(|(_, v)| *v > 0) {
        let x = cumul as f64 * 100.0 / total as f64;
        let l = *v as f64 * 100.0 / total as f64;
        let part = (v * 100 + total / 2) / total;
        let _ = write!(
            h,
            "<g><title>{} : {} pages, {part} %</title><rect class=\"{}\" x=\"{}\" y=\"0\" width=\"{}\" height=\"28\" rx=\"4\"/>",
            echapper(s.nom),
            entier(*v),
            s.classe,
            pourcent(x),
            pourcent(l)
        );
        if part >= PART_ETIQUETEE {
            let _ = write!(
                h,
                "<text class=\"dans\" x=\"{}\" y=\"19\">{part} %</text>",
                pourcent(x + 0.8)
            );
        }
        h.push_str("</g>");
        cumul += v;
    }
    h.push_str("</svg>");
    Some(h)
}

/// Barre d'un classement, rapportée au maximum de la liste. Le chiffre est dans la colonne voisine.
pub fn barre(valeur: i64, max: i64, libelle: &str) -> String {
    let l = if max > 0 {
        valeur as f64 * 100.0 / max as f64
    } else {
        0.0
    };
    format!(
        "<svg class=\"barre\" width=\"100%\" height=\"12\" role=\"img\" aria-label=\"{t}\"><title>{t}</title>\
         <rect class=\"piste\" width=\"100%\" height=\"12\" rx=\"4\"/><rect class=\"noir\" width=\"{}\" height=\"12\" rx=\"4\"/></svg>",
        pourcent(l),
        t = echapper(libelle)
    )
}

/// Part d'un tout pour une seule mesure, dans une couleur qui n'est celle d'aucune série.
pub fn jauge(valeur: i64, max: i64, libelle: &str) -> String {
    barre(valeur, max, libelle).replace("class=\"noir\"", "class=\"jauge\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERIES: [Serie; 2] = [
        Serie {
            nom: "N&B",
            classe: "noir",
        },
        Serie {
            nom: "Couleur",
            classe: "couleur",
        },
    ];

    fn point(libelle: &str, valeurs: &[i64]) -> Point {
        Point {
            libelle: libelle.to_owned(),
            valeurs: valeurs.to_vec(),
        }
    }

    #[test]
    fn graduations_rondes_jamais_sous_le_maximum() {
        assert_eq!(pas_de_graduation(1), 1);
        assert_eq!(pas_de_graduation(7), 2);
        assert_eq!(pas_de_graduation(161), 50);
        assert_eq!(pas_de_graduation(1999), 500);
        for max in 1..5000 {
            assert!(pas_de_graduation(max) * 4 >= max, "{max}");
        }
    }

    #[test]
    fn rien_a_montrer_pas_de_graphique() {
        assert_eq!(
            colonnes_empilees("d", "pages", &SERIES, &[point("a", &[0, 0])]),
            None
        );
        assert_eq!(colonnes_empilees("d", "pages", &SERIES, &[]), None);
        assert_eq!(segments("d", &[(SERIES[0], 0)]), None);
    }

    #[test]
    fn colonnes_dans_le_cadre_et_textes_echappes() {
        let h = colonnes_empilees(
            "d<",
            "pages",
            &SERIES,
            &[
                point("<j1>", &[100, 61]),
                point("j2", &[0, 1]),
                point("j3", &[0, 0]),
            ],
        )
        .unwrap();
        assert!(h.contains("aria-label=\"d&lt;\""), "{h}");
        assert!(
            h.contains("<title>&lt;j1&gt; : 161 pages · N&amp;B 100 · Couleur 61</title>"),
            "{h}"
        );
        assert!(!h.contains("<j1>"));
        for morceau in h.split("<rect ").skip(1) {
            let y: i64 = morceau
                .split("y=\"")
                .nth(1)
                .unwrap()
                .split('"')
                .next()
                .unwrap()
                .parse()
                .unwrap();
            assert!((0..=BASE).contains(&y), "{morceau}");
        }
        // Une valeur minuscule reste visible.
        assert!(h.contains("height=\"1\"") || h.contains("height=\"2\""), "{h}");
    }

    #[test]
    fn les_positions_restent_dans_la_largeur() {
        let points: Vec<Point> = (0..366).map(|i| point(&i.to_string(), &[i, 1])).collect();
        let h = colonnes_empilees("d", "pages", &SERIES, &points).unwrap();
        for morceau in h.split(" x=\"").skip(1) {
            let v = morceau.split('"').next().unwrap();
            if let Some(p) = v.strip_suffix('%') {
                assert!((0.0..=100.0).contains(&p.parse::<f64>().unwrap()), "{v}");
            }
        }
        assert!(h.matches("axe milieu").count() <= ETIQUETTES_MAX);
    }

    #[test]
    fn une_jauge_n_emprunte_pas_la_couleur_d_une_serie() {
        let h = jauge(1, 2, "d");
        assert!(
            h.contains("class=\"jauge\"") && !h.contains("class=\"noir\""),
            "{h}"
        );
        assert!(h.contains("width=\"50.000%\""), "{h}");
    }

    #[test]
    fn une_part_trop_petite_n_a_pas_d_etiquette() {
        let h = segments("d", &[(SERIES[0], 97), (SERIES[1], 3)]).unwrap();
        assert!(h.contains(">97 %</text>"), "{h}");
        assert!(!h.contains(">3 %</text>"), "{h}");
        assert!(h.contains("<title>Couleur : 3 pages, 3 %</title>"), "{h}");
    }
}
