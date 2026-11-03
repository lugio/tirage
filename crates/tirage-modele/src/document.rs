//! Nom du document : ce qu'on en garde dépend d'une politique choisie par l'organisation
//! (voir docs/architecture/0002-donnees-personnelles.md).

use serde::{Deserialize, Serialize};

/// Que garder du titre d'un travail ?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolitiqueNomDocument {
    /// Rien.
    Non,
    /// L'application d'origine seulement, déduite du titre ; le titre est jeté.
    #[default]
    Application,
    /// Le titre complet et l'application.
    Complet,
}

impl PolitiqueNomDocument {
    /// Renvoie `(nom_document, application)` à enregistrer pour un titre brut.
    pub fn appliquer(self, titre: Option<&str>) -> (Option<String>, Option<String>) {
        let titre = titre.map(str::trim).filter(|t| !t.is_empty());
        match self {
            PolitiqueNomDocument::Non => (None, None),
            PolitiqueNomDocument::Application => (None, titre.and_then(application_du_titre)),
            PolitiqueNomDocument::Complet => (titre.map(str::to_owned), titre.and_then(application_du_titre)),
        }
    }
}

/// Préfixes posés par des applications courantes (« Microsoft Word - rapport.docx »).
const PREFIXES: &[&str] = &[
    "Microsoft Word",
    "Microsoft Excel",
    "Microsoft PowerPoint",
    "Microsoft Outlook",
];

/// Extensions → application (ou famille).
const EXTENSIONS: &[(&str, &str)] = &[
    ("pdf", "PDF"),
    ("doc", "Microsoft Word"),
    ("docx", "Microsoft Word"),
    ("rtf", "Traitement de texte"),
    ("odt", "Traitement de texte"),
    ("xls", "Microsoft Excel"),
    ("xlsx", "Microsoft Excel"),
    ("xlsm", "Microsoft Excel"),
    ("csv", "Tableur"),
    ("ods", "Tableur"),
    ("ppt", "Microsoft PowerPoint"),
    ("pptx", "Microsoft PowerPoint"),
    ("odp", "Présentation"),
    ("msg", "Microsoft Outlook"),
    ("eml", "Messagerie"),
    ("txt", "Texte"),
    ("jpg", "Image"),
    ("jpeg", "Image"),
    ("png", "Image"),
    ("tif", "Image"),
    ("tiff", "Image"),
    ("htm", "Navigateur"),
    ("html", "Navigateur"),
];

/// Déduit l'application d'origine d'un titre de travail, sans rien garder du titre lui-même.
pub fn application_du_titre(titre: &str) -> Option<String> {
    let t = titre.trim();
    for p in PREFIXES {
        if let Some(reste) = t.strip_prefix(p)
            && (reste.starts_with(" - ") || reste.is_empty())
        {
            return Some((*p).to_owned());
        }
    }
    let minuscule = t.to_lowercase();
    if minuscule.starts_with("http://") || minuscule.starts_with("https://") {
        return Some("Navigateur".to_owned());
    }
    let (_, ext) = minuscule.rsplit_once('.')?;
    let ext = ext.trim();
    EXTENSIONS
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, a)| (*a).to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_office() {
        assert_eq!(
            application_du_titre("Microsoft Word - Rapport EXEMPLE.docx").as_deref(),
            Some("Microsoft Word")
        );
        assert_eq!(
            application_du_titre("Microsoft Excel - budget.xlsx").as_deref(),
            Some("Microsoft Excel")
        );
        // « Microsoft Wordpad » n'est pas Word
        assert_eq!(application_du_titre("Microsoft Wordpad"), None);
    }

    #[test]
    fn extensions() {
        assert_eq!(application_du_titre("facture-0001.PDF").as_deref(), Some("PDF"));
        assert_eq!(application_du_titre("notes.txt").as_deref(), Some("Texte"));
        assert_eq!(application_du_titre("archive.inconnue"), None);
        assert_eq!(application_du_titre("sans extension"), None);
    }

    #[test]
    fn navigateur() {
        assert_eq!(
            application_du_titre("https://example.org/page").as_deref(),
            Some("Navigateur")
        );
    }

    #[test]
    fn politiques() {
        let titre = Some("Microsoft Word - Dossier EXEMPLE.docx");
        assert_eq!(PolitiqueNomDocument::Non.appliquer(titre), (None, None));
        assert_eq!(
            PolitiqueNomDocument::Application.appliquer(titre),
            (None, Some("Microsoft Word".into()))
        );
        assert_eq!(
            PolitiqueNomDocument::Complet.appliquer(titre),
            (
                Some("Microsoft Word - Dossier EXEMPLE.docx".into()),
                Some("Microsoft Word".into())
            )
        );
        assert_eq!(PolitiqueNomDocument::Complet.appliquer(Some("   ")), (None, None));
        assert_eq!(PolitiqueNomDocument::default(), PolitiqueNomDocument::Application);
    }
}
