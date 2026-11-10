//! Composants HTML partagés par les pages de l'interface : gabarit, navigation, tableau, tuiles, onglets.
//!
//! Tout texte reçu ici est échappé. Seules exceptions, explicites : le `corps` de [`page`], déjà assemblé à partir de ces
//! composants, et [`Cellule::Html`].

use std::fmt::Write as _;

use crate::session::Session;

pub const FEUILLE_DE_STYLE: &str = include_str!("style.css");
/// Logo de l'outil, servi sur /logo.svg (en-tête et icône d'onglet). Copie conforme de `docs/images/icone.svg`.
pub const LOGO: &str = include_str!("logo.svg");

/// Rubrique de la navigation à signaler comme active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rubrique {
    Accueil,
    Palmares,
    Rapports,
    Copieurs,
    Tarifs,
    Journal,
    Aucune,
}

const NAVIGATION: [(Rubrique, &str, &str); 6] = [
    (Rubrique::Accueil, "/", "Accueil"),
    (Rubrique::Palmares, "/palmares", "Palmarès"),
    (Rubrique::Rapports, "/rapports/service", "Rapports"),
    (Rubrique::Copieurs, "/copieurs", "Copieurs"),
    (Rubrique::Tarifs, "/tarifs", "Tarifs"),
    (Rubrique::Journal, "/journal", "Journal"),
];

pub fn echapper(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => r.push_str("&amp;"),
            '<' => r.push_str("&lt;"),
            '>' => r.push_str("&gt;"),
            '"' => r.push_str("&quot;"),
            '\'' => r.push_str("&#39;"),
            _ => r.push(c),
        }
    }
    r
}

/// Encodage d'un identifiant dans un chemin d'URL.
pub fn encoder_chemin(s: &str) -> String {
    let mut r = String::with_capacity(s.len());
    for o in s.bytes() {
        if o.is_ascii_alphanumeric() || matches!(o, b'-' | b'.' | b'_' | b'~') {
            r.push(char::from(o));
        } else {
            let _ = write!(r, "%{o:02X}");
        }
    }
    r
}

fn navigation(rubrique: Rubrique, qui: &str) -> String {
    let mut h = String::from("<nav aria-label=\"Navigation principale\">");
    for (r, href, libelle) in NAVIGATION {
        let actif = if r == rubrique {
            " aria-current=\"page\""
        } else {
            ""
        };
        let _ = write!(h, "<a href=\"{href}\"{actif}>{libelle}</a>");
    }
    let _ = write!(
        h,
        "</nav><div class=\"compte\"><span>{}</span><form method=\"post\" action=\"/deconnexion\">\
         <button>Se déconnecter</button></form></div>",
        echapper(qui)
    );
    h
}

/// Adresse du code source, affichée au pied de chaque page (AGPL-3.0, article 13) : une version modifiée et
/// servie à d'autres doit pointer vers SON propre code source.
pub const SOURCE: &str = "https://github.com/lugio/tirage";

/// Page complète. Sans session, l'en-tête ne porte que le nom de l'outil.
pub fn page(rubrique: Rubrique, titre: &str, corps: &str, session: Option<&Session>) -> String {
    let nav = session.map_or(String::new(), |s| {
        navigation(rubrique, s.nom.as_deref().unwrap_or(&s.login))
    });
    format!(
        "<!doctype html><html lang=\"fr\"><head><meta charset=\"utf-8\"><title>{t} — Tirage</title>\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <link rel=\"stylesheet\" href=\"/style.css\"><link rel=\"icon\" href=\"/logo.svg\" type=\"image/svg+xml\"></head><body>\
         <a class=\"evitement\" href=\"#contenu\">Aller au contenu</a><header><span class=\"marque\"><img src=\"/logo.svg\" alt=\"\" width=\"28\" height=\"28\">Tirage</span>{nav}</header>\
         <main id=\"contenu\"><h1>{t}</h1>{corps}</main>\
         <footer class=\"pied\">Tirage {v} · <a href=\"{SOURCE}\">Code source</a> · licence AGPL-3.0</footer></body></html>",
        t = echapper(titre),
        v = env!("CARGO_PKG_VERSION")
    )
}

pub fn erreur(message: &str) -> String {
    format!("<p class=\"erreur\">{}</p>", echapper(message))
}

pub fn note(message: &str) -> String {
    format!("<p class=\"note\">{}</p>", echapper(message))
}

/// Rangée de chiffres clés : (valeur, libellé).
pub fn tuiles(valeurs: &[(String, &str)]) -> String {
    let mut h = String::from("<div class=\"tuiles\">");
    for (valeur, libelle) in valeurs {
        let _ = write!(
            h,
            "<div class=\"tuile\"><b>{}</b><span>{}</span></div>",
            echapper(valeur),
            echapper(libelle)
        );
    }
    h.push_str("</div>");
    h
}

/// Liens de même niveau, le lien actif étant signalé : (adresse, libellé, actif).
pub fn onglets(liens: &[(String, &str, bool)]) -> String {
    let mut h = String::from("<div class=\"onglets\">");
    for (href, libelle, actif) in liens {
        let a = if *actif { " aria-current=\"true\"" } else { "" };
        let _ = write!(h, "<a href=\"{}\"{a}>{}</a>", echapper(href), echapper(libelle));
    }
    h.push_str("</div>");
    h
}

/// Durée lisible, en français.
pub fn duree(ms: i64) -> String {
    let s = (ms.max(0) + 500) / 1000;
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min {} s", s / 60, s % 60),
        _ if s % 3600 / 60 == 0 => format!("{} h", s / 3600),
        _ => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

/// Taille lisible, en français.
pub fn taille(octets: i64) -> String {
    const KO: i64 = 1024;
    const MO: i64 = 1024 * KO;
    if octets < KO {
        return format!("{} o", octets.max(0));
    }
    if octets < MO {
        return format!("{} Ko", (octets + KO / 2) / KO);
    }
    let dixiemes = (octets * 10 + MO / 2) / MO;
    format!("{},{} Mo", dixiemes / 10, dixiemes % 10)
}

pub fn bouton_lien(href: &str, libelle: &str) -> String {
    format!(
        "<p><a class=\"bouton\" href=\"{}\">{}</a></p>",
        echapper(href),
        echapper(libelle)
    )
}

/// Bloc titré d'une page.
pub fn carte(titre: &str, corps: &str) -> String {
    format!(
        "<section class=\"carte\"><h2>{}</h2>{corps}</section>",
        echapper(titre)
    )
}

/// Nombre avec séparateur de milliers.
pub fn entier(n: i64) -> String {
    let brut = n.unsigned_abs().to_string();
    let mut r = String::new();
    for (i, c) in brut.chars().enumerate() {
        if i > 0 && (brut.len() - i).is_multiple_of(3) {
            r.push('\u{202f}');
        }
        r.push(c);
    }
    if n < 0 { format!("-{r}") } else { r }
}

/// Colonne d'un [`Tableau`] : les nombres sont alignés à droite.
pub struct Colonne {
    titre: String,
    nombre: bool,
}

impl Colonne {
    pub fn texte(titre: &str) -> Self {
        Colonne {
            titre: titre.to_owned(),
            nombre: false,
        }
    }

    pub fn nombre(titre: &str) -> Self {
        Colonne {
            titre: titre.to_owned(),
            nombre: true,
        }
    }
}

pub enum Cellule {
    Texte(String),
    Nombre(String),
    Lien {
        href: String,
        texte: String,
    },
    /// HTML déjà assemblé par un composant de ce module.
    Html(String),
    Vide,
}

impl Cellule {
    pub fn texte(v: &str) -> Self {
        Cellule::Texte(v.to_owned())
    }

    /// Lien vers la fiche d'une personne.
    pub fn utilisateur(identifiant: &str) -> Self {
        Cellule::Lien {
            href: format!("/utilisateurs/{}", encoder_chemin(identifiant)),
            texte: identifiant.to_owned(),
        }
    }

    fn ecrire(&self, h: &mut String) {
        let _ = match self {
            Cellule::Texte(v) => write!(h, "<td>{}</td>", echapper(v)),
            Cellule::Nombre(v) => write!(h, "<td class=\"n\">{}</td>", echapper(v)),
            Cellule::Lien { href, texte } => {
                write!(
                    h,
                    "<td><a href=\"{}\">{}</a></td>",
                    echapper(href),
                    echapper(texte)
                )
            }
            Cellule::Html(v) => write!(h, "<td>{v}</td>"),
            Cellule::Vide => write!(h, "<td></td>"),
        };
    }
}

/// Tableau de données, avec défilement horizontal s'il est plus large que l'écran.
pub struct Tableau {
    html: String,
    lignes: usize,
    entete: bool,
    a_un_lien: bool,
}

impl Tableau {
    pub fn nouveau(colonnes: &[Colonne]) -> Self {
        let mut html = String::from("<div class=\"defile\"><table><thead><tr>");
        for c in colonnes {
            let classe = if c.nombre { " class=\"n\"" } else { "" };
            let _ = write!(html, "<th{classe}>{}</th>", echapper(&c.titre));
        }
        html.push_str("</tr></thead><tbody>");
        Tableau {
            html,
            lignes: 0,
            entete: true,
            a_un_lien: false,
        }
    }

    /// Classement sans en-tête : le titre de la section suffit.
    pub fn sans_entete() -> Self {
        Tableau {
            html: String::from("<div class=\"defile\"><table><tbody>"),
            lignes: 0,
            entete: false,
            a_un_lien: false,
        }
    }

    pub fn ligne(&mut self, cellules: impl IntoIterator<Item = Cellule>) {
        self.ajouter("<tr>", cellules);
    }

    pub fn total(&mut self, cellules: impl IntoIterator<Item = Cellule>) {
        self.ajouter("<tr class=\"total\">", cellules);
    }

    fn ajouter(&mut self, ouverture: &str, cellules: impl IntoIterator<Item = Cellule>) {
        self.html.push_str(ouverture);
        for c in cellules {
            self.a_un_lien |= matches!(c, Cellule::Lien { .. });
            c.ecrire(&mut self.html);
        }
        self.html.push_str("</tr>");
        self.lignes += 1;
    }

    pub fn fin(mut self) -> String {
        self.html.push_str("</tbody></table></div>");
        // Sans lien à l'intérieur, le clavier ne pourrait pas faire défiler un tableau trop large.
        if self.entete && !self.a_un_lien {
            self.html = self.html.replacen(
                "<div class=\"defile\">",
                "<div class=\"defile\" tabindex=\"0\" role=\"region\" aria-label=\"Tableau, défilement horizontal\">",
                1,
            );
        }
        self.html
    }

    /// Le tableau, ou le message s'il n'a aucune ligne.
    pub fn fin_ou(self, message: &str) -> String {
        if self.lignes == 0 {
            note(message)
        } else {
            self.fin()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chaque_page_donne_le_lien_du_code_source() {
        let h = page(Rubrique::Accueil, "Connexion", "", None);
        assert!(
            h.contains(&format!("<a href=\"{SOURCE}\">Code source</a>")),
            "{h}"
        );
        assert!(h.contains(env!("CARGO_PKG_VERSION")), "{h}");
    }

    #[test]
    fn le_tableau_echappe_et_aligne() {
        let mut t = Tableau::nouveau(&[Colonne::texte("N&B"), Colonne::nombre("Pages")]);
        t.ligne([Cellule::texte("<b>x</b>"), Cellule::Nombre("6".into())]);
        let h = t.fin();
        assert!(h.contains("<th>N&amp;B</th><th class=\"n\">Pages</th>"), "{h}");
        assert!(
            h.contains("<td>&lt;b&gt;x&lt;/b&gt;</td><td class=\"n\">6</td>"),
            "{h}"
        );
    }

    #[test]
    fn le_lien_d_une_personne_est_encode_et_echappe() {
        let mut h = String::new();
        Cellule::utilisateur("a\"><b").ecrire(&mut h);
        assert_eq!(
            h,
            "<td><a href=\"/utilisateurs/a%22%3E%3Cb\">a&quot;&gt;&lt;b</a></td>"
        );
    }

    #[test]
    fn un_tableau_sans_lien_defile_au_clavier() {
        let mut sans = Tableau::nouveau(&[Colonne::texte("A")]);
        sans.ligne([Cellule::texte("x")]);
        assert!(sans.fin().contains("tabindex=\"0\""));
        let mut avec = Tableau::nouveau(&[Colonne::texte("A")]);
        avec.ligne([Cellule::utilisateur("x")]);
        assert!(!avec.fin().contains("tabindex"));
    }

    #[test]
    fn un_tableau_vide_laisse_place_au_message() {
        let t = Tableau::nouveau(&[Colonne::texte("Quand")]);
        assert_eq!(t.fin_ou("Rien."), "<p class=\"note\">Rien.</p>");
    }

    #[test]
    fn la_navigation_signale_la_rubrique_active_et_echappe_le_nom() {
        let h = navigation(Rubrique::Palmares, "<Nom>");
        assert!(h.contains("<a href=\"/palmares\" aria-current=\"page\">"), "{h}");
        assert!(h.contains("<a href=\"/\">"), "{h}");
        assert!(h.contains("&lt;Nom&gt;"), "{h}");
        assert!(!page(Rubrique::Aucune, "T", "", None).contains("<nav"));
    }

    #[test]
    fn les_onglets_echappent_l_adresse() {
        let h = onglets(&[("/r?du=1&au=2".to_owned(), "Service", true)]);
        assert!(
            h.contains("href=\"/r?du=1&amp;au=2\" aria-current=\"true\""),
            "{h}"
        );
    }
}
