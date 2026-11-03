//! File d'attente sur disque du collecteur (lot 1).
//!
//! Principe : chaque événement est écrit de façon atomique avant tout envoi, puis retiré seulement après
//! l'accusé de réception du serveur.

/// Nom du sous-dossier de la file, relatif au dossier de données du collecteur.
pub const DOSSIER_FILE: &str = "a-envoyer";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dossier_relatif() {
        assert!(!DOSSIER_FILE.contains('/') && !DOSSIER_FILE.contains('\\'));
    }
}
