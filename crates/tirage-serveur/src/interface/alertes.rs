//! Lecture des alertes d'un copieur : traduit les messages de l'appareil, classe leur gravité, retire le bruit.
//!
//! Un relevé mêle les bits d'erreur standard (déjà en français) et les messages de l'appareil. La veille n'est pas
//! une panne, et le bit « intervention demandée » accompagne souvent un message plus précis : seuls ceux-ci restent.
//! Un message inconnu est gardé tel quel : il ne doit jamais disparaître parce qu'il n'est pas reconnu.

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Niveau {
    Urgent,
    Attention,
}

impl Niveau {
    pub fn libelle(self) -> &'static str {
        match self {
            Niveau::Urgent => "Urgent",
            Niveau::Attention => "À prévoir",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alerte {
    pub niveau: Niveau,
    pub texte: String,
}

fn alerte(niveau: Niveau, texte: impl Into<String>) -> Alerte {
    Alerte {
        niveau,
        texte: texte.into(),
    }
}

fn est_veille(a: &str) -> bool {
    a.eq_ignore_ascii_case("sleep") || a.eq_ignore_ascii_case("low power")
}

fn couleur(mot: &str) -> String {
    match mot.to_ascii_lowercase().as_str() {
        "black" => "noir".into(),
        "yellow" => "jaune".into(),
        autre => autre.to_owned(),
    }
}

fn bac(texte: &str) -> String {
    match texte.to_ascii_lowercase().strip_prefix("tray") {
        Some(n) if !n.is_empty() => n.to_owned(),
        _ => texte.to_owned(),
    }
}

/// Message précis de l'appareil, reconnu ou gardé tel quel.
fn message(a: &str) -> Alerte {
    let m = a.to_ascii_lowercase();
    let suite = |debut: &str| a.get(debut.len()..).unwrap_or("").trim().to_owned();
    if m.starts_with("toner near empty") {
        return alerte(
            Niveau::Attention,
            format!("Toner bientôt vide : {}", couleur(&suite("toner near empty"))),
        );
    }
    if m.starts_with("toner empty") {
        return alerte(
            Niveau::Urgent,
            format!("Toner vide : {}", couleur(&suite("toner empty"))),
        );
    }
    if m.starts_with("no paper") {
        return alerte(Niveau::Attention, format!("Bac {} vide", bac(&suite("no paper"))));
    }
    if m.starts_with("low paper") {
        return alerte(
            Niveau::Attention,
            format!("Bac {} presque vide", bac(&suite("low paper"))),
        );
    }
    if m.starts_with("wastetoner near full") {
        return alerte(Niveau::Attention, "Récupérateur de toner presque plein");
    }
    if m.starts_with("wastetoner full") {
        return alerte(Niveau::Urgent, "Récupérateur de toner plein");
    }
    if m.contains("jam") {
        return alerte(Niveau::Urgent, "Bourrage");
    }
    if m.contains("door") || m.contains("cover") {
        return alerte(Niveau::Urgent, "Porte ouverte");
    }
    alerte(Niveau::Attention, a)
}

fn majuscule(a: &str) -> String {
    let mut lettres = a.chars();
    let debut = lettres
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    debut + lettres.as_str()
}

/// Bit d'erreur standard : `None` s'il n'apporte rien de plus que les messages précis.
fn bit(a: &str, precis: &[&str]) -> Option<Alerte> {
    let a_message = |debuts: &[&str]| {
        precis.iter().any(|p| {
            let p = p.to_ascii_lowercase();
            debuts.iter().any(|d| p.starts_with(d))
        })
    };
    match a {
        "toner bas" if a_message(&["toner"]) => None,
        "papier bas" | "plus de papier" | "bac d'entrée vide" if a_message(&["no paper", "low paper"]) => {
            None
        }
        "bourrage" | "porte ouverte" | "plus de toner" => Some(alerte(Niveau::Urgent, majuscule(a))),
        _ => Some(alerte(Niveau::Attention, majuscule(a))),
    }
}

const BITS: [&str; 15] = [
    "papier bas",
    "plus de papier",
    "toner bas",
    "plus de toner",
    "porte ouverte",
    "bourrage",
    "hors ligne",
    "intervention demandée",
    "bac d'entrée absent",
    "bac de sortie absent",
    "consommable absent",
    "bac de sortie presque plein",
    "bac de sortie plein",
    "bac d'entrée vide",
    "maintenance préventive dépassée",
];

/// Alertes à montrer pour un relevé, les plus graves d'abord, sans doublon.
pub fn lire(alertes: &[String]) -> Vec<Alerte> {
    let precis: Vec<&str> = alertes
        .iter()
        .map(String::as_str)
        .filter(|a| !a.is_empty() && !est_veille(a) && !BITS.contains(a))
        .collect();
    // « Intervention demandée » seule compte : la veille et les messages précis l'allument aussi.
    let seule = alertes.len() == 1;
    let mut l: Vec<Alerte> = Vec::new();
    for a in alertes {
        let trouve = if a.is_empty() || est_veille(a) {
            None
        } else if a == "intervention demandée" {
            seule.then(|| alerte(Niveau::Attention, "Intervention demandée"))
        } else if BITS.contains(&a.as_str()) {
            bit(a, &precis)
        } else {
            Some(message(a))
        };
        if let Some(t) = trouve
            && !l.contains(&t)
        {
            l.push(t);
        }
    }
    l.sort_by_key(|a| a.niveau);
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    fn textes(alertes: &[&str]) -> Vec<String> {
        let brut: Vec<String> = alertes.iter().map(|s| (*s).to_owned()).collect();
        lire(&brut).into_iter().map(|a| a.texte).collect()
    }

    #[test]
    fn la_veille_n_est_pas_une_panne() {
        assert!(textes(&["intervention demandée", "Sleep"]).is_empty());
        assert!(textes(&["intervention demandée", "Low Power"]).is_empty());
    }

    #[test]
    fn intervention_seule_reste_signalee() {
        assert_eq!(textes(&["intervention demandée"]), ["Intervention demandée"]);
    }

    #[test]
    fn les_messages_precis_remplacent_les_bits_generaux() {
        assert_eq!(
            textes(&[
                "papier bas",
                "toner bas",
                "Toner Near Empty Black",
                "Low Paper Tray3"
            ]),
            ["Toner bientôt vide : noir", "Bac 3 presque vide"]
        );
        assert_eq!(
            textes(&["intervention demandée", "No Paper Tray1"]),
            ["Bac 1 vide"]
        );
        assert_eq!(
            textes(&[
                "toner bas",
                "WasteToner Near Full",
                "Toner Near Empty Cyan",
                "Sleep"
            ]),
            ["Récupérateur de toner presque plein", "Toner bientôt vide : cyan"]
        );
    }

    #[test]
    fn un_bit_sans_message_precis_est_garde() {
        assert_eq!(textes(&["toner bas"]), ["Toner bas"]);
    }

    #[test]
    fn le_plus_grave_d_abord() {
        let brut = vec!["Toner Near Empty Yellow".to_owned(), "bourrage".to_owned()];
        let l = lire(&brut);
        assert_eq!(l[0].niveau, Niveau::Urgent);
        assert_eq!(
            (l[0].texte.as_str(), l[1].texte.as_str()),
            ("Bourrage", "Toner bientôt vide : jaune")
        );
    }

    #[test]
    fn message_inconnu_garde_tel_quel() {
        assert_eq!(textes(&["Fuser Error C2557"]), ["Fuser Error C2557"]);
    }

    #[test]
    fn urgences_reconnues() {
        let brut: Vec<String> = [
            "Toner Empty Magenta",
            "WasteToner Full",
            "Paper Jam Tray 2",
            "Front Door Open",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let l = lire(&brut);
        assert!(l.iter().all(|a| a.niveau == Niveau::Urgent));
        assert_eq!(l[0].texte, "Toner vide : magenta");
    }
}
