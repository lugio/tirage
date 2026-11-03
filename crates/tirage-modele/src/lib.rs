//! Types partagés entre le collecteur et le serveur.
//!
//! Le format d'échange est versionné : un serveur refuse un événement d'une version qu'il ne connaît pas
//! plutôt que de l'interpréter de travers.

/// Version du format d'échange collecteur → serveur.
pub const VERSION_FORMAT: u32 = 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_du_format_initiale() {
        assert_eq!(VERSION_FORMAT, 1);
    }
}
